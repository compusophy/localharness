// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {GuildFacet} from "./GuildFacet.sol";
import {LibGuildStorage} from "../libraries/LibGuildStorage.sol";
import {LibWeightedVotingStorage} from "../libraries/LibWeightedVotingStorage.sol";

/// @title WeightedVotingFacet
/// @notice A SHARE-WEIGHTED governance board (a cap table) that runs IN
///         PARALLEL to `VotingFacet` over the SAME guild treasury — never
///         touching it. Where `VotingFacet` is one-member-one-vote
///         (`weight = 1`, quorum = `ceil(memberCount/2)`), this facet tallies
///         admin-assigned SHARES and quorums on MORE THAN HALF of a
///         total-shares snapshot. Same membership gate
///         (`LibGuildStorage.roleOf`), same treasury, same `_spendCore`
///         payout, same snapshot-prevents-churn discipline — only the WEIGHT
///         BASIS differs. A guild can run all three boards (admin fiat
///         `spendTreasury`, 1m1v `VotingFacet`, cap-table `WeightedVotingFacet`)
///         over one treasury, never colliding (isolated storage slots; shared
///         `LibGuildStorage`).
///
///         WEIGHT BASIS. A per-`(guildId, member)` share count (default 0) set
///         ONLY by a guild Admin via `setShares` (gated exactly like
///         `GuildFacet.setRole`). `setShares` maintains `totalShares[guildId]`
///         delta-style (`total = total - old + new`) so the live denominator is
///         O(1) and never drifts. A member's shares are INDEPENDENT of role: a
///         Member with 60 shares outvotes an Admin with 10. Role gates WHO can
///         vote (a current member) and WHO can set shares (Admin); shares
///         decide HOW MUCH the vote weighs. `setShares` on a None-role address
///         is allowed (seats their cap-table entry; they still can't vote until
///         they're a member).
///
///         HOW EXECUTE REUSES `GuildFacet._spendCore` — the single source of
///         treasury-accounting truth. This facet INHERITS GuildFacet, so a
///         passed `executeWeighted` calls the inherited `_spendCore(gs,
///         guildId, to, amount, memo)` against the SHARED `LibGuildStorage`
///         slot — the EXACT CEI-safe debit `GuildFacet.spendTreasury` and
///         `VotingFacet.execute` both reach. The ONLY difference from the Admin
///         path is the gate (a passed WEIGHTED vote, not the Admin role). On
///         the live diamond this is its own deployed contract whose bytecode
///         INCLUDES `_spendCore` (via inheritance); the inherited GuildFacet
///         externals are NOT re-registered (the diamond already routes them to
///         the live GuildFacet) — only THIS facet's OWN selectors are cut in
///         (see script/AddWeightedVotingFacet.s.sol).
///
///         QUORUM / THRESHOLD (the weighted formula):
///           • QUORUM = `2 * (forShares + againstShares) > snapshotTotalShares`
///             — "more than half of the total-shares snapshot must have voted
///             (for OR against)". The division-free form avoids the
///             divide-by-zero a `snapshot/2` would need a guard for; a
///             zero-total guild can never pass (0 > 0 is false). The denominator
///             is SNAPSHOTTED at PROPOSE (`WProposal.snapshotTotalShares`), NOT
///             re-read live — so an Admin re-weighting the cap table mid-vote
///             can't shrink/inflate the bar (the share-weighted churn fix).
///           • THRESHOLD = STRICT majority of cast SHARES (`forShares >
///             againstShares`); a tie FAILS.
///         A proposal that misses quorum OR fails the threshold by its deadline
///         goes Active → Failed with NO spend (idempotent).
///
///         CEI + REENTRANCY-SAFE on execute: the status flips to terminal
///         `Executed` BEFORE `_spendCore`'s external transfer (barrier 1), and
///         `_spendCore` debits `guildBalance` before its own transfer
///         (barrier 2) — a re-entrant `executeWeighted` reverts on
///         `ProposalNotActive` and a re-entrant `_spendCore` on
///         `InsufficientTreasury`.
///
///         SELECTOR HYGIENE (the `bountyTaskOf` / `guildMembersOf` lesson).
///         EVERY external is `weighted`/`Weighted`/`shares`-named so NONE
///         clashes with `VotingFacet`'s `propose`/`vote`/`execute`/`getProposal`/
///         `tallyOf`/`proposalsOf`/`hasVoted`/`proposalMemoOf`/`proposalCount`
///         already on the live diamond. `setShares`/`sharesOf`/`totalSharesOf`
///         are new names — verify collision-free against the live loupe before
///         the cut.
contract WeightedVotingFacet is GuildFacet {
    // --- Events ---------------------------------------------------------
    //
    // DISTINCT names from VotingFacet's so indexers never confuse the two
    // boards. Events are not part of diamond selector routing, so re-declaring
    // GuildFacet's events here (via inheritance) cannot collide.

    event SharesSet(uint256 indexed guildId, address indexed member, uint256 shares, address indexed by);
    event WeightedProposalCreated(
        uint256 indexed proposalId,
        uint256 indexed guildId,
        address indexed proposer,
        address to,
        uint256 amount,
        uint64 deadline,
        uint256 snapshotTotalShares
    );
    event WeightedVoteCast(uint256 indexed proposalId, address indexed voter, bool support, uint256 weight);
    event WeightedProposalExecuted(uint256 indexed proposalId, address indexed to, uint256 amount);
    event WeightedProposalFailed(uint256 indexed proposalId);

    // --- Errors ---------------------------------------------------------
    //
    // Declared locally (some names match VotingFacet's, but a Solidity error
    // is identified by its 4-byte selector from the signature — declaring an
    // identically-named error in a separate contract is a no-op for routing;
    // each facet just needs its own to revert with). `NotAdmin` /
    // `UnknownGuild` are INHERITED from GuildFacet (the shares + guild gates).

    error UnknownProposal(); // no such proposalId
    error NotGuildMember(); // proposer/voter is not a member of the guild
    error ZeroProposalAmount(); // propose with amount == 0
    error ZeroProposalRecipient(); // propose to address(0)
    error BadVotingPeriod(); // period outside [MIN, MAX]
    error MemoTooLarge(); // memo bytes over MAX_MEMO_BYTES
    error AmountExceedsTreasury(); // propose/execute amount > guild treasury
    error VotingClosed(); // vote after the deadline
    error AlreadyVoted(); // a voter voting twice on one proposal
    error ProposalNotActive(); // vote/execute on a non-Active (terminal) proposal
    error VotingNotEnded(); // execute before the deadline
    error NoVotingPower(); // a member with 0 shares attempting to vote
    error SharesLockedDuringVote(); // setShares while a weighted proposal is open

    // --- Shares (Admin-gated cap table) ---------------------------------

    /// Admin-only: set `member`'s share weight in the guild's cap table.
    /// Gated exactly like `GuildFacet.setRole` (`roleOf[guildId][msg.sender]
    /// == Admin`). Maintains `totalShares[guildId]` delta-style so the live
    /// total is O(1). Setting a member to 0 revokes; re-assigning overwrites.
    /// Allowed on a None-role address (seats their entry; they still can't vote
    /// until they're a member). `member` MAY be a contract (a sub-guild TBA).
    ///
    /// Reverts if the guild doesn't exist (`UnknownGuild`) or the caller is not
    /// an Admin (`NotAdmin`). No-op (no event, no write) if the value is
    /// unchanged.
    function setShares(uint256 guildId, address member, uint256 shares) external {
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        if (!gs.guilds[guildId].exists) revert UnknownGuild();
        if (gs.roleOf[guildId][msg.sender] != LibGuildStorage.Role.Admin) revert NotAdmin();

        LibWeightedVotingStorage.Storage storage ws = LibWeightedVotingStorage.load();
        // The cap table is FROZEN while a weighted proposal is open: each ballot's
        // weight is the live share count but the quorum denominator is snapshotted
        // at propose, so re-weighting mid-vote could blow past the frozen quorum.
        // `shareLockUntil` is the latest open proposal's deadline; auto-releases.
        if (block.timestamp < ws.shareLockUntil[guildId]) revert SharesLockedDuringVote();
        uint256 old = ws.shares[guildId][member];
        if (old == shares) return; // no-op
        // O(1) running total. Under Solidity 0.8 checked arithmetic this never
        // underflows: `old` is the exact prior value, so `total - old >= 0`
        // (invariant: totalShares == sum of shares[*]).
        ws.totalShares[guildId] = ws.totalShares[guildId] - old + shares;
        ws.shares[guildId][member] = shares;
        emit SharesSet(guildId, member, shares, msg.sender);
    }

    // --- Propose (a guild member opens a share-weighted measure) --------

    /// A guild MEMBER proposes spending `amount` of the guild treasury to
    /// `to`, opening a SHARE-WEIGHTED vote that closes at `now + period`.
    /// Returns the new `proposalId`.
    ///
    /// NOTE the signature differs from `VotingFacet.propose`: `period` is
    /// `uint256` and `memo` is a `string` sitting LAST. Validation otherwise
    /// mirrors `propose`: guild exists (`UnknownGuild`), caller is a CURRENT
    /// member (`NotGuildMember`), `amount != 0` (`ZeroProposalAmount`), `to !=
    /// address(0)` (`ZeroProposalRecipient`), `period` within
    /// [MIN_VOTING_PERIOD, MAX_VOTING_PERIOD] (`BadVotingPeriod`),
    /// `bytes(memo).length <= MAX_MEMO_BYTES` (`MemoTooLarge`), and `amount <=
    /// guildBalance` (`AmountExceedsTreasury` — a fail-fast; re-checked at
    /// execute).
    ///
    /// The quorum denominator is SNAPSHOTTED here: `ws.totalShares[guildId]`
    /// frozen into `snapshotTotalShares` (the share-weighted analogue of
    /// `VotingFacet`'s `snapshotMemberCount`). `to` MAY be a contract.
    function proposeWeighted(
        uint256 guildId,
        address to,
        uint256 amount,
        uint256 period,
        string calldata memo
    ) external returns (uint256 proposalId) {
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        if (!gs.guilds[guildId].exists) revert UnknownGuild();
        if (gs.roleOf[guildId][msg.sender] == LibGuildStorage.Role.None) revert NotGuildMember();
        if (amount == 0) revert ZeroProposalAmount();
        if (to == address(0)) revert ZeroProposalRecipient();
        if (
            period < LibWeightedVotingStorage.MIN_VOTING_PERIOD
                || period > LibWeightedVotingStorage.MAX_VOTING_PERIOD
        ) revert BadVotingPeriod();
        if (bytes(memo).length > LibWeightedVotingStorage.MAX_MEMO_BYTES) revert MemoTooLarge();
        if (amount > gs.guildBalance[guildId]) revert AmountExceedsTreasury();

        LibWeightedVotingStorage.Storage storage ws = LibWeightedVotingStorage.load();
        proposalId = ++ws.nextProposalId; // ids start at 1
        uint64 deadline = uint64(block.timestamp) + uint64(period);
        // FREEZE the cap table for this proposal's voting window: setShares
        // reverts until the latest open proposal's deadline, so the live per-ballot
        // weight can't be inflated past the snapshotted quorum denominator.
        if (deadline > ws.shareLockUntil[guildId]) ws.shareLockUntil[guildId] = deadline;
        uint256 snapshot = ws.totalShares[guildId];
        ws.proposals[proposalId] = LibWeightedVotingStorage.WProposal({
            guildId: guildId,
            proposer: msg.sender,
            to: to,
            amount: amount,
            deadline: deadline,
            status: LibWeightedVotingStorage.WStatus.Active,
            forShares: 0,
            againstShares: 0,
            snapshotTotalShares: snapshot
        });
        if (bytes(memo).length != 0) ws.memo[proposalId] = bytes(memo);
        ws.proposalsOfGuild[guildId].push(proposalId);

        emit WeightedProposalCreated(proposalId, guildId, msg.sender, to, amount, deadline, snapshot);
    }

    // --- Vote (one ballot per voter; weight == the voter's shares) ------

    /// Cast ONE share-weighted ballot on an Active proposal. The voter's
    /// CURRENT shares are added to `forShares` / `againstShares` (not 1). A
    /// member with 0 shares is rejected (`NoVotingPower`) — so a zero-weight
    /// ballot can't occupy the double-vote slot without moving the tally, and
    /// quorum-by-distinct-voters can't be gamed by zero-share members.
    ///
    /// Membership is read LIVE from `LibGuildStorage` (a member who left can't
    /// vote). Shares are read live too, BUT the cap table is FROZEN for the whole
    /// voting window — `setShares` reverts `SharesLockedDuringVote` until the
    /// latest open proposal's deadline (`shareLockUntil`) — so a live read here
    /// can't be inflated past the propose-time `snapshotTotalShares` denominator.
    /// (For 1m1v `VotingFacet` weight is the constant 1, so it needs no such lock;
    /// only the weighted board, where weight itself is the lever, does.)
    ///
    /// Reverts if: the proposal doesn't exist (`UnknownProposal`), it is not
    /// Active (`ProposalNotActive`), voting has closed (`VotingClosed`), the
    /// caller is not a guild member (`NotGuildMember`), the caller has 0 shares
    /// (`NoVotingPower`), or the caller already voted (`AlreadyVoted`).
    function voteWeighted(uint256 proposalId, bool support) external {
        LibWeightedVotingStorage.Storage storage ws = LibWeightedVotingStorage.load();
        LibWeightedVotingStorage.WProposal storage p = ws.proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        if (p.status != LibWeightedVotingStorage.WStatus.Active) revert ProposalNotActive();
        if (block.timestamp > p.deadline) revert VotingClosed();

        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        if (gs.roleOf[p.guildId][msg.sender] == LibGuildStorage.Role.None) revert NotGuildMember();
        if (ws.voted[proposalId][msg.sender]) revert AlreadyVoted();

        uint256 weight = ws.shares[p.guildId][msg.sender];
        if (weight == 0) revert NoVotingPower();

        ws.voted[proposalId][msg.sender] = true;
        if (support) {
            p.forShares += weight;
        } else {
            p.againstShares += weight;
        }

        emit WeightedVoteCast(proposalId, msg.sender, support, weight);
    }

    // --- Execute (after the deadline; spend if passed) ------------------

    /// Resolve a proposal after its voting period ends. PERMISSIONLESS (the
    /// outcome is deterministic from the on-chain tally). On a PASS (quorum met
    /// AND strict majority of cast shares for) it EXECUTES: debits the guild
    /// treasury and transfers `amount` `$LH` to `to`, via the inherited
    /// `GuildFacet._spendCore` (single accounting source). Otherwise it FAILS
    /// with no spend. Either way terminal — IDEMPOTENT.
    ///
    /// Reverts if: the proposal doesn't exist (`UnknownProposal`), it is not
    /// Active (`ProposalNotActive`), or the deadline has not passed
    /// (`VotingNotEnded`). On the PASS path, `amount` is re-checked against the
    /// LIVE treasury (`AmountExceedsTreasury`) — leaving the proposal Active on
    /// a shortfall so it can be retried once the treasury is refunded.
    ///
    /// CEI: the status flips to `Executed` BEFORE `_spendCore`'s external
    /// transfer (barrier 1); `_spendCore` debits `guildBalance` before its own
    /// transfer (barrier 2).
    function executeWeighted(uint256 proposalId) external {
        LibWeightedVotingStorage.Storage storage ws = LibWeightedVotingStorage.load();
        LibWeightedVotingStorage.WProposal storage p = ws.proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        if (p.status != LibWeightedVotingStorage.WStatus.Active) revert ProposalNotActive();
        if (block.timestamp <= p.deadline) revert VotingNotEnded();

        LibGuildStorage.Storage storage gs = LibGuildStorage.load();

        if (_passedWeighted(p)) {
            // Re-check affordability against the LIVE treasury (it can change
            // while the vote runs). Leave Active on shortfall so it can retry.
            if (p.amount > gs.guildBalance[p.guildId]) revert AmountExceedsTreasury();

            // CEI barrier 1: terminal status BEFORE the external spend.
            p.status = LibWeightedVotingStorage.WStatus.Executed;

            // Reuse GuildFacet's internal treasury-debit core — the SAME
            // CEI-safe path `spendTreasury` / `VotingFacet.execute` use. The
            // measure memo is carried into GuildFacet's TreasurySpent event.
            _spendCore(gs, p.guildId, p.to, p.amount, ws.memo[proposalId]);

            emit WeightedProposalExecuted(proposalId, p.to, p.amount);
        } else {
            p.status = LibWeightedVotingStorage.WStatus.Failed;
            emit WeightedProposalFailed(proposalId);
        }
    }

    // --- Views ----------------------------------------------------------

    /// A member's share weight in the guild's cap table (0 default).
    function sharesOf(uint256 guildId, address member) external view returns (uint256) {
        return LibWeightedVotingStorage.load().shares[guildId][member];
    }

    /// The guild's live total shares (the quorum denominator source). Mirrors
    /// `treasuryBalanceOf`; the CLI needs it to show "X of Y shares".
    function totalSharesOf(uint256 guildId) external view returns (uint256) {
        return LibWeightedVotingStorage.load().totalShares[guildId];
    }

    /// Full weighted-proposal record by id. Reverts `UnknownProposal` for an
    /// unknown id. Returns `snapshotTotalShares` so the caller can compute
    /// quorum without a second call. `status` is the raw WStatus
    /// (0=Active … 4=Expired).
    function weightedProposal(uint256 proposalId)
        external
        view
        returns (
            uint256 guildId,
            address proposer,
            address to,
            uint256 amount,
            uint64 deadline,
            uint8 status,
            uint256 forShares,
            uint256 againstShares,
            uint256 snapshotTotalShares
        )
    {
        LibWeightedVotingStorage.WProposal storage p = LibWeightedVotingStorage.load().proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        return (
            p.guildId,
            p.proposer,
            p.to,
            p.amount,
            p.deadline,
            uint8(p.status),
            p.forShares,
            p.againstShares,
            p.snapshotTotalShares
        );
    }

    /// The opaque measure `memo` bytes for a proposal (empty if none).
    function weightedProposalMemoOf(uint256 proposalId) external view returns (bytes memory) {
        return LibWeightedVotingStorage.load().memo[proposalId];
    }

    /// Paginated scan of a guild's weighted-proposal ids: up to `limit` ids
    /// from `proposalsOfGuild[guildId]` starting at INDEX `startAfter` (a
    /// 0-based index into the per-guild list, NOT a proposal id), plus the
    /// `nextCursor`. Index-window paging, identical to `VotingFacet.proposalsOf`
    /// (the list is append-only so cursors stay stable across blocks).
    function weightedProposalsOf(uint256 guildId, uint256 startAfter, uint256 limit)
        external
        view
        returns (uint256[] memory ids, uint256 nextCursor)
    {
        uint256[] storage all = LibWeightedVotingStorage.load().proposalsOfGuild[guildId];
        uint256 total = all.length;
        if (startAfter >= total || limit == 0) {
            return (new uint256[](0), total);
        }
        uint256 end = startAfter + limit;
        if (end > total) end = total;
        uint256 n = end - startAfter;
        ids = new uint256[](n);
        for (uint256 i = 0; i < n; i++) {
            ids[i] = all[startAfter + i];
        }
        nextCursor = end;
    }

    /// True iff `voter` has cast a ballot on `proposalId`.
    function hasVotedWeighted(uint256 proposalId, address voter) external view returns (bool) {
        return LibWeightedVotingStorage.load().voted[proposalId][voter];
    }

    /// The current share-weighted tally: `forShares`, `againstShares`, the
    /// `quorumShares` DISPLAY value (`snapshotTotalShares / 2 + 1` — "shares
    /// needed"; the on-chain test is the division-free `2 * cast > snapshot`),
    /// the `castShares` so far (`for + against`), and `passing` — whether it
    /// WOULD pass right now (quorum met AND strict majority for). Reverts
    /// `UnknownProposal` for an unknown id.
    function weightedTallyOf(uint256 proposalId)
        external
        view
        returns (
            uint256 forShares,
            uint256 againstShares,
            uint256 quorumShares,
            uint256 castShares,
            bool passing
        )
    {
        LibWeightedVotingStorage.WProposal storage p = LibWeightedVotingStorage.load().proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        forShares = p.forShares;
        againstShares = p.againstShares;
        castShares = forShares + againstShares;
        // DISPLAY quorum = strictly-more-than-half = floor(snapshot/2)+1. (The
        // executable test is `2*cast > snapshot`, which is division-free and
        // exactly equivalent: `cast >= floor(snapshot/2)+1`.)
        quorumShares = p.snapshotTotalShares / 2 + 1;
        passing = _passedWeighted(p);
    }

    /// Total weighted proposals ever created (== highest id; ids monotonic).
    function weightedProposalCount() external view returns (uint256) {
        return LibWeightedVotingStorage.load().nextProposalId;
    }

    // --- internals ------------------------------------------------------

    /// Whether a weighted proposal passes:
    ///   • QUORUM: `2 * (forShares + againstShares) > snapshotTotalShares` —
    ///     strictly more than half of the snapshot total shares voted. The
    ///     division-free form is divide-by-zero-safe; a 0-snapshot can never
    ///     pass (0 > 0 is false). The denominator is the propose-time snapshot,
    ///     so admin re-weighting mid-vote can't move the bar.
    ///   • THRESHOLD: STRICT majority of cast SHARES (`for > against`; a tie
    ///     fails).
    /// `2 * cast` overflow: `cast <= snapshotTotalShares`; realistic cap tables
    /// are far below 2^255 (the same implicit bound `VotingFacet` makes on its
    /// vote sums).
    function _passedWeighted(LibWeightedVotingStorage.WProposal storage p)
        internal
        view
        returns (bool)
    {
        uint256 cast = p.forShares + p.againstShares;
        if (2 * cast <= p.snapshotTotalShares) return false; // quorum not met
        return p.forShares > p.againstShares; // strict majority for
    }
}
