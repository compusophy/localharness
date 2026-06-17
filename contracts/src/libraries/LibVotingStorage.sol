// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the VOTING / DAO facet — Rung 4 of the
///      coordination ladder (design/agent-coordination.md), the apex: a
///      guild whose treasury is spent BY VOTE rather than by admin fiat. A
///      member PROPOSES a treasury spend, members VOTE, and a passed measure
///      EXECUTES from the guild's own treasury. Diamond storage pattern —
///      fresh slot, no collision with the registry / TBA / team / guild /
///      bounty / invite / schedule / credits storage already cut into the
///      diamond. Add new fields ONLY at the end of the struct, and ONLY at
///      the end of `Proposal` (diamond storage layout is positional and
///      immutable — the "append-only rule").
///
///      WHAT IT GOVERNS. The MVP measure is the concrete DAO action that
///      makes a guild member-governed: a TREASURY SPEND
///      (`to`, `amount`, `memo`) out of `GuildFacet`'s `guildBalance`
///      escrow. On a pass, `execute` debits the SAME `LibGuildStorage`
///      ledger and transfers the credits token — i.e. it routes through the
///      VERY SAME CEI-safe treasury-debit core as `GuildFacet.spendTreasury`
///      / `GuildFacet._spend` (single source of accounting truth), but
///      gated on a vote instead of the Admin role. Generic
///      arbitrary-measure execution (an opaque `target`/`data` call from the
///      treasury TBA) is the documented follow-up — the seam is the measure
///      shape, not this storage.
///
///      THE RECURSIVE PROPERTY (Part 4 of the design doc — "turtles all the
///      way down"). A voter is an `address` and NOTHING here assumes it is
///      an EOA. A guild's own TBA is a contract account that can be a member
///      of another guild (proven in GuildFacet), so a member-DAO's TBA can
///      cast a vote in a parent DAO. The nested "the member-DAO opens its
///      OWN proposal to decide how to vote, then its treasury TBA executes
///      `parentDao.vote(...)`" auto-resolution is the next layer — NOT built
///      here; this facet just sees an `address` voting. The discipline that
///      keeps that door open: keep `voted` / membership keyed on `address`,
///      never on EOA-ness.
library LibVotingStorage {
    bytes32 constant POSITION = keccak256("localharness.voting.storage.v1");

    // --- Governance constants (documented; sane MVP defaults) -----------
    //
    //  QUORUM (participation gate): at least `ceil(memberCount / 2)` of the
    //  guild's CURRENT members must have voted (for OR against) for a
    //  proposal to be eligible to pass. Computed as
    //  `(memberCount + 1) / 2` (integer ceil-of-half). For a 1-member guild
    //  this is 1 — the sole member voting meets quorum (the divide-by-zero
    //  / degenerate case is handled by `_quorum` returning 1 when
    //  memberCount is 0 OR 1, so a vote is always required and a
    //  zero-member guild can never pass anything).
    //
    //  THRESHOLD (approval gate): STRICT majority of cast votes —
    //  `forVotes * 2 > forVotes + againstVotes` (i.e. for > against). A tie
    //  FAILS (no majority). One-member-one-vote in the MVP (weight = 1),
    //  so `forVotes + againstVotes` is exactly the number of distinct
    //  members who voted.
    //
    //  QUORUM IS SNAPSHOTTED AT PROPOSE (the governance-robustness fix). The
    //  denominator is the guild's `memberCount` AT THE MOMENT the proposal is
    //  created, frozen into `Proposal.snapshotMemberCount`. It is NOT re-read
    //  against the live membership at vote/execute. This closes a
    //  membership-churn class: previously members could LEAVE between propose
    //  and execute to shrink the live `memberCount` so a thin minority met the
    //  smaller quorum and drained the treasury (a voter could even vote FOR
    //  then leave — their vote stayed counted while the denominator dropped),
    //  OR a griefer could FLOOD sybil members to inflate the live quorum above
    //  the honest cast votes and sink a legitimately-passing measure. With the
    //  snapshot the bar is fixed when voting OPENS. (THRESHOLD is purely the
    //  cast tally — `for > against` — so it is unaffected by membership
    //  changes regardless; the only churn lever was the quorum denominator.)
    //  Membership/weight at vote time still come from GuildFacet (`roleOf` —
    //  you must be a CURRENT member to cast a ballot); only the quorum
    //  DENOMINATOR is the snapshot.

    // --- Bounds (anti-grief). The voting period is the one window the
    //     contract MUST bound: a 0/sub-minute period is a flash-vote trap,
    //     an unbounded period locks a proposal (and the implied treasury
    //     intent) open forever. Mirror BountyFacet's TTL discipline. ------
    uint64 internal constant MIN_VOTING_PERIOD = 1 hours;
    uint64 internal constant MAX_VOTING_PERIOD = 30 days;

    /// Hard ceiling on the stored measure `memo` byte length (gas-per-byte;
    /// a pointer/short note is the intended payload, like BountyFacet's task
    /// cap). Stops an unbounded blob from being escrowed into one SSTORE
    /// chain.
    uint256 internal constant MAX_MEMO_BYTES = 4096;

    /// Proposal lifecycle (the ABI-pinned enum — Active=0 … Expired=4).
    ///   Active   (0) — open for voting; `now <= deadline`.
    ///   Passed   (1) — terminal-via-execute marker is `Executed`; `Passed`
    ///                  is NOT a stored resting state (a passed proposal is
    ///                  executed directly). Kept in the enum for the
    ///                  `tallyOf` projection / future timelock seam.
    ///   Failed   (2) — deadline passed, quorum not met OR majority against;
    ///                  no spend. Terminal.
    ///   Executed (3) — passed AND the treasury spend was performed.
    ///                  Terminal; idempotent (can't execute twice).
    ///   Expired  (4) — reserved (deadline-passed-but-unresolved marker for
    ///                  a future GC sweep). Unused by the MVP `execute`
    ///                  (which goes straight Active → Failed/Executed).
    enum VStatus {
        Active, // 0
        Passed, // 1 (transient classification; not a stored resting state)
        Failed, // 2 — terminal: did not pass; no spend
        Executed, // 3 — terminal: passed + spent
        Expired // 4 — reserved
    }

    /// One proposal record, keyed by a monotonic `uint256 id`. Scalars
    /// packed to minimise cold SSTOREs. The measure is a treasury spend:
    /// `to` + `amount` (+ the separate `memo` bytes mapping).
    struct Proposal {
        uint256 guildId; // the guild whose treasury this measure spends
        address proposer; // a member at propose-time; the record's author
        address to; // the spend recipient (may be a contract — a member-guild TBA)
        uint256 amount; // $LH (18-dec wei) to spend from the guild treasury
        uint64 deadline; // unix seconds; voting closes here, execute opens after
        VStatus status; // Active | Failed | Executed (Passed/Expired reserved)
        uint256 forVotes; // weight voting support (== count of for-voters, MVP weight 1)
        uint256 againstVotes; // weight voting against
        // --- APPEND-ONLY additions below this line ----------------------
        /// The guild's `memberCount` AT PROPOSE TIME — the FROZEN quorum
        /// denominator (`quorum = ceil(snapshotMemberCount / 2)`). Read by
        /// `_passed` / `tallyOf` instead of the live member count so
        /// membership churn between propose and execute can't move the bar
        /// (the governance-robustness fix). A 0/1-member snapshot still maps
        /// to quorum 1 via `_quorum` (the divide-by-zero guard), so a vote is
        /// always required.
        uint64 snapshotMemberCount;
        /// Count of FOR ballots cast by ADMINs (the privilege-escalation fix).
        /// A passing 1m1v treasury spend MUST include >= 1 Admin FOR vote, so a
        /// rogue Officer that floods sybil Members can't move the treasury
        /// without Admin consent (restoring the `spendTreasury` Admin-gate
        /// invariant the bare-majority path silently dropped). Tallied live at
        /// `vote` (Admin-role read on the FOR path), checked by `_passed`.
        /// APPEND-ONLY: a pre-fix in-flight proposal reads 0 here, so it can no
        /// longer execute until re-proposed — fail-safe, never a silent spend.
        uint256 forAdminVotes;
    }

    struct Storage {
        /// proposalId -> proposal record. Monotonic id from `nextProposalId`
        /// (ids start at 1; 0 = no proposal). A non-zero `proposer` means the
        /// id is live (the unknown-proposal guard).
        mapping(uint256 => Proposal) proposals;
        /// proposalId -> the opaque measure `memo` bytes (a note / pointer).
        /// Stored separately because bytes don't pack into the scalar slots
        /// and on-chain bytes storage is the gas-hungry path.
        mapping(uint256 => bytes) memo;
        /// proposalId -> voter address -> has voted (one-member-one-vote;
        /// the double-vote guard). KEYED ON ADDRESS, never on EOA-ness — a
        /// voter may be a contract (a member-guild's TBA), the recursive
        /// property (Part 4 of the design doc).
        mapping(uint256 => mapping(address => bool)) voted;
        /// Monotonic proposal id counter (ids start at 1; 0 = no proposal).
        uint256 nextProposalId;
        /// guildId -> the proposal ids opened against it (for `proposalsOf`).
        /// Append-only, so an id's position is stable and the paginated
        /// `proposalsOf` cursor stays valid across calls.
        mapping(uint256 => uint256[]) proposalsOfGuild;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
