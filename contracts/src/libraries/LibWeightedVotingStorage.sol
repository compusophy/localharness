// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the WEIGHTED-VOTING facet — a SHARE-WEIGHTED
///      governance board that COEXISTS with (and never touches) the
///      one-member-one-vote `VotingFacet` (Rung 4 of the coordination ladder,
///      design/agent-coordination.md). Where `VotingFacet` tallies HEADCOUNT
///      (weight = 1, quorum = ceil(memberCount/2)), this facet tallies
///      admin-assigned SHARES (a cap table) and quorums on MORE THAN HALF of a
///      total-shares snapshot. Both facets read the SAME `LibGuildStorage`
///      slot (membership + treasury) but each owns an isolated voting-storage
///      slot, so a guild can run an admin-fiat spend (`spendTreasury`), a 1m1v
///      DAO (`VotingFacet`), AND a cap-table board (`WeightedVotingFacet`)
///      over one treasury, never colliding.
///
///      Diamond storage pattern — a FRESH slot
///      (`localharness.weightedvoting.storage.v1`), no collision with the
///      registry / TBA / team / guild / bounty / invite / schedule / credits /
///      voting storage already cut into the diamond. Add new fields ONLY at
///      the end of the struct, and ONLY at the end of `WProposal` (diamond
///      storage layout is positional and immutable — the "append-only rule").
///
///      WEIGHT BASIS. A per-`(guildId, member)` share count
///      (`shares[guildId][member]`, default 0) set ONLY by a guild Admin via
///      `setShares`. A running `totalShares[guildId]` is maintained delta-style
///      so the live denominator is O(1) and never drifts
///      (invariant: `totalShares == sum of shares[*]`). A member's shares are
///      INDEPENDENT of role: role gates WHO can vote (a current member) and WHO
///      can set shares (Admin); shares decide HOW MUCH a vote weighs.
///
///      THE RECURSIVE PROPERTY (Part 4 — "turtles all the way down"). A voter
///      / share-holder is an `address`, NEVER gated to an EOA — a sub-guild's
///      TBA can hold shares and cast a weighted ballot in a parent guild's
///      board. Keep `voted` / `shares` keyed on `address`.
library LibWeightedVotingStorage {
    bytes32 constant POSITION = keccak256("localharness.weightedvoting.storage.v1");

    // --- Bounds (anti-grief) — MIRROR LibVotingStorage so the two boards
    //     share one voting-period discipline (a 0/sub-minute period is a
    //     flash-vote trap; an unbounded period locks a proposal — and its
    //     implied treasury intent — open forever). -----------------------
    uint64 internal constant MIN_VOTING_PERIOD = 1 hours;
    uint64 internal constant MAX_VOTING_PERIOD = 30 days;

    /// Hard ceiling on the stored measure `memo` byte length (gas-per-byte; a
    /// pointer/short note is the intended payload). Mirrors LibVotingStorage.
    uint256 internal constant MAX_MEMO_BYTES = 4096;

    /// Proposal lifecycle (the ABI-pinned enum — Active=0 … Expired=4),
    /// MIRRORING LibVotingStorage.VStatus so the two boards decode identically.
    ///   Active   (0) — open for voting; `now <= deadline`.
    ///   Passed   (1) — transient classification; NOT a stored resting state
    ///                  (a passed proposal is executed directly to `Executed`).
    ///   Failed   (2) — deadline passed, quorum not met OR majority against;
    ///                  no spend. Terminal.
    ///   Executed (3) — passed AND the treasury spend was performed. Terminal;
    ///                  idempotent.
    ///   Expired  (4) — reserved (future GC sweep marker). Unused by `execute`.
    enum WStatus {
        Active, // 0
        Passed, // 1 (transient classification; not a stored resting state)
        Failed, // 2 — terminal: did not pass; no spend
        Executed, // 3 — terminal: passed + spent
        Expired // 4 — reserved
    }

    /// One weighted-proposal record, keyed by a monotonic `uint256 id`. The
    /// measure is a treasury spend: `to` + `amount` (+ the separate `memo`
    /// bytes mapping). Tallies are SUMS OF SHARES, not headcounts.
    struct WProposal {
        uint256 guildId; // the guild whose treasury this measure spends
        address proposer; // a member at propose-time; the record's author
        address to; // the spend recipient (may be a contract — a member-guild TBA)
        uint256 amount; // $LH (18-dec wei) to spend from the guild treasury
        uint64 deadline; // unix seconds; voting closes here, execute opens after
        WStatus status; // Active | Failed | Executed (Passed/Expired reserved)
        uint256 forShares; // SUM of for-voters' shares (NOT headcount)
        uint256 againstShares; // SUM of against-voters' shares
        // --- APPEND-ONLY additions below this line ----------------------
        /// The guild's `totalShares` AT PROPOSE TIME — the FROZEN quorum
        /// DENOMINATOR (quorum test = `2 * cast > snapshotTotalShares`). Read
        /// by `_passedWeighted` / `weightedTallyOf` instead of the live total
        /// so an Admin re-weighting the cap table between propose and execute
        /// can't shrink/inflate the bar (the share-weighted churn fix). A
        /// 0-total snapshot can never pass (0 > 0 is false).
        uint256 snapshotTotalShares;
    }

    struct Storage {
        /// Cap table: guildId -> member address -> share count (default 0).
        /// KEYED ON ADDRESS, never on EOA-ness (the recursive property).
        mapping(uint256 => mapping(address => uint256)) shares;
        /// guildId -> live SUM of all members' shares. Maintained O(1) via the
        /// `setShares` delta (`total = total - old + new`); the quorum
        /// denominator source (snapshotted at propose).
        mapping(uint256 => uint256) totalShares;
        /// proposalId -> proposal record. Monotonic id from `nextProposalId`
        /// (ids start at 1; 0 = no proposal). A non-zero `proposer` means the
        /// id is live (the unknown-proposal guard).
        mapping(uint256 => WProposal) proposals;
        /// proposalId -> the opaque measure `memo` bytes (a note / pointer).
        /// Stored separately (bytes don't pack into the scalar slots; on-chain
        /// bytes storage is the gas-hungry path).
        mapping(uint256 => bytes) memo;
        /// proposalId -> voter address -> has voted (the double-vote guard).
        /// KEYED ON ADDRESS — a voter may be a contract (a member-guild's TBA).
        mapping(uint256 => mapping(address => bool)) voted;
        /// Monotonic proposal id counter (ids start at 1; 0 = no proposal).
        uint256 nextProposalId;
        /// guildId -> the proposal ids opened against it (for
        /// `weightedProposalsOf`). Append-only, so an id's position is stable
        /// and the paginated cursor stays valid across calls.
        mapping(uint256 => uint256[]) proposalsOfGuild;
        /// guildId -> unix seconds until which `setShares` is LOCKED — the
        /// LATEST open weighted-proposal deadline. The quorum DENOMINATOR is
        /// snapshotted at propose, but each ballot's WEIGHT is the live share
        /// count; without this, an Admin could re-weight a friendly voter AFTER
        /// propose and blow past the frozen quorum (a snapshot bypass). Setting
        /// this to a proposal's deadline freezes the cap table for the whole
        /// voting window, so cast weights stay consistent with the snapshot.
        /// Auto-releases when the deadline passes (time-based, no sweep needed).
        mapping(uint256 => uint64) shareLockUntil;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
