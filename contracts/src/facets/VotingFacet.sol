// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {GuildFacet} from "./GuildFacet.sol";
import {LibGuildStorage} from "../libraries/LibGuildStorage.sol";
import {LibVotingStorage} from "../libraries/LibVotingStorage.sol";

/// @title VotingFacet
/// @notice The DAO apex — Rung 4 of the coordination ladder
///         (design/agent-coordination.md). Turns a `GuildFacet` guild from
///         ADMIN-controlled into MEMBER-GOVERNED: a member PROPOSES a
///         treasury spend, members VOTE one-member-one-vote, and a passed
///         measure EXECUTES from the guild's own treasury. The standing
///         organism the lower rungs feed (a DAO votes on which bounties to
///         fund out of the guild treasury that a party then claims).
///
///         WHAT IT GOVERNS (the MVP measure). The concrete DAO action is a
///         TREASURY SPEND — `(guildId, to, amount, memo)`. Generic
///         arbitrary-measure execution (an opaque `target`/`data` call from
///         the treasury TBA) is the documented follow-up; the seam is the
///         measure shape, not this facet's plumbing.
///
///         HOW EXECUTE REUSES `GuildFacet._spend` (single source of
///         accounting truth — the whole reason `spendTreasury` was factored
///         through an internal `_spend`). VotingFacet INHERITS GuildFacet,
///         so a passed `execute` calls the inherited `_spendCore(gs, guildId,
///         to, amount, memo)` DIRECTLY — the exact same CEI-safe debit core
///         `GuildFacet.spendTreasury` reaches (`spendTreasury` → `_spend`
///         (calldata) → `_spendCore` (memory); `execute` → `_spendCore`
///         (memory), since it has no calldata bytes arg): validate → debit
///         `LibGuildStorage.guildBalance[guildId]` → external token transfer
///         LAST. Same storage slot, same ledger, same reentrancy guarantee;
///         the ONLY difference from the Admin path is the gate (a passed
///         vote, not the Admin role). On the live diamond VotingFacet is its
///         own deployed contract whose bytecode INCLUDES `_spend` (via
///         inheritance) and which reads/writes the SHARED `LibGuildStorage`
///         slot — exactly the cross-facet storage sharing the diamond
///         provides (the same technique `GuildFacet.createGuild` uses to
///         replicate `register`'s writes against the shared
///         `LibRegistryStorage` slot). The inherited GuildFacet externals
///         are NOT re-registered in the cut (the diamond already routes them
///         to the live GuildFacet); only VotingFacet's OWN selectors are
///         cut in (see script/AddVotingFacet.s.sol).
///
///         QUORUM / THRESHOLD:
///           • QUORUM = `ceil(snapshotMemberCount / 2)` distinct members must
///             have voted (for OR against). The denominator is SNAPSHOTTED at
///             PROPOSE (`Proposal.snapshotMemberCount`), NOT re-read live at
///             execute — so members can't join/leave between propose and
///             execute to move the bar (the governance-robustness fix; a
///             leaver's already-cast vote no longer shrinks the denominator
///             under it, and sybil-flooding can't inflate it). DIVIDE-BY-ZERO
///             / degenerate guard: `_quorum` returns 1 for a 0- or 1-member
///             snapshot, so a vote is ALWAYS required and a zero-member guild
///             can never pass. Treasury affordability IS still re-checked live
///             at execute (the balance can change while the vote runs).
///           • THRESHOLD = STRICT majority of cast votes (`for > against`);
///             a tie FAILS. One-member-one-vote (weight 1) in the MVP. (Purely
///             the cast tally — unaffected by churn regardless.)
///         A proposal that misses quorum OR fails the threshold by its
///         deadline goes Active → Failed with NO spend (idempotent).
///
///         CEI + REENTRANCY-SAFE on execute: the proposal status flips to a
///         terminal `Executed` BEFORE `_spend` performs the external token
///         transfer, AND `_spend` itself debits the treasury ledger before
///         its transfer — so a hostile re-entrant token that calls `execute`
///         again re-reads `status != Active` and reverts (no double-spend),
///         and even if it slipped past, the already-debited `guildBalance`
///         reverts the second `_spend` on `InsufficientTreasury`. Two CEI
///         barriers, proven by the reentrant-token probe.
///
///         THE RECURSIVE PROPERTY (Part 4 — "turtles all the way down"). A
///         voter is an `address`; NOTHING here gates it to an EOA. A guild's
///         own TBA is a contract account, so a member-DAO's TBA can be a
///         guild member (proven in GuildFacet) and therefore cast a vote
///         here. The nested "member-DAO opens its OWN proposal to decide how
///         to vote, then its treasury TBA executes `parentDao.vote(...)`"
///         auto-resolution is the NEXT layer — NOT built; this facet just
///         sees an address voting (proven by the contract-member-votes
///         test).
///
///         CUTTING IT (diamond owner; mirror script/AddGuildFacet):
///         deploy + diamondCut Add the 8 selectors in
///         script/AddVotingFacet.s.sol. No post-cut config — membership +
///         treasury are read from the shared LibGuildStorage slot and the
///         credits token from the shared CreditsFacet slot. GuildFacet must
///         already be cut (it is, at 0xfE806FD0…).
///
///         SELECTOR NOTE: `propose` / `vote` / `execute` / `getProposal` /
///         `proposalsOf` / `hasVoted` / `tallyOf` / `proposalCount` were all
///         checked collision-free against the LIVE diamond's 148 selectors —
///         no generic-name clash (unlike Bounty's `taskOf`/Guild's
///         `membersOf`), so no `gov`/`proposal` prefix was needed.
contract VotingFacet is GuildFacet {
    // --- Events ---------------------------------------------------------

    event ProposalCreated(
        uint256 indexed proposalId,
        uint256 indexed guildId,
        address indexed proposer,
        address to,
        uint256 amount,
        uint64 deadline
    );
    event VoteCast(uint256 indexed proposalId, address indexed voter, bool support, uint256 weight);
    event ProposalExecuted(uint256 indexed proposalId, address indexed to, uint256 amount);
    event ProposalFailed(uint256 indexed proposalId);

    // --- Errors ---------------------------------------------------------

    error UnknownProposal(); // no such proposalId
    error NotGuildMember(); // proposer/voter is not a member of the guild
    error ZeroProposalAmount(); // propose with amount == 0
    error ZeroProposalRecipient(); // propose to address(0)
    error BadVotingPeriod(); // period outside [MIN, MAX]
    error MemoTooLarge(); // memo bytes over MAX_MEMO_BYTES
    error AmountExceedsTreasury(); // propose/execute amount > guild treasury
    error VotingClosed(); // vote after the deadline
    error AlreadyVoted(); // a member voting twice on one proposal
    error ProposalNotActive(); // vote/execute on a non-Active (terminal) proposal
    error VotingNotEnded(); // execute before the deadline

    // --- Propose (a guild member proposes a treasury spend) -------------

    /// A guild MEMBER proposes spending `amount` of the guild treasury to
    /// `to`, opening a vote that closes at `now + votingPeriod`. Returns the
    /// new `proposalId`.
    ///
    /// Reverts if: the guild doesn't exist (`UnknownGuild`), the caller is
    /// NOT a member (`NotGuildMember`), `amount == 0`
    /// (`ZeroProposalAmount`), `to == address(0)` (`ZeroProposalRecipient`),
    /// `votingPeriod` is outside [MIN_VOTING_PERIOD, MAX_VOTING_PERIOD]
    /// (`BadVotingPeriod`), `memo` exceeds the cap (`MemoTooLarge`), or
    /// `amount` already exceeds the live treasury balance
    /// (`AmountExceedsTreasury` — a fail-fast; the balance is re-checked at
    /// execute time too, since it can change while the vote runs).
    ///
    /// `to` MAY be a contract (an EOA, a worker's TBA, or a member-guild's
    /// address) — the spend is not gated to EOAs (the recursive money flow).
    function propose(
        uint256 guildId,
        address to,
        uint256 amount,
        bytes calldata memo,
        uint64 votingPeriod
    ) external returns (uint256 proposalId) {
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        if (!gs.guilds[guildId].exists) revert UnknownGuild();
        if (gs.roleOf[guildId][msg.sender] == LibGuildStorage.Role.None) revert NotGuildMember();
        if (amount == 0) revert ZeroProposalAmount();
        if (to == address(0)) revert ZeroProposalRecipient();
        if (
            votingPeriod < LibVotingStorage.MIN_VOTING_PERIOD
                || votingPeriod > LibVotingStorage.MAX_VOTING_PERIOD
        ) revert BadVotingPeriod();
        if (memo.length > LibVotingStorage.MAX_MEMO_BYTES) revert MemoTooLarge();
        // Fail-fast: a proposal whose amount already can't be afforded is
        // pointless. (It is RE-checked at execute, because fundGuild /
        // spendTreasury can change the balance while the vote runs.)
        if (amount > gs.guildBalance[guildId]) revert AmountExceedsTreasury();

        LibVotingStorage.Storage storage vs = LibVotingStorage.load();
        proposalId = ++vs.nextProposalId; // ids start at 1
        uint64 deadline = uint64(block.timestamp) + votingPeriod;
        // Snapshot the quorum denominator NOW (the governance-robustness fix):
        // freeze the guild's member count at propose so churn between propose
        // and execute can't shrink/inflate the quorum bar. See
        // LibVotingStorage's QUORUM IS SNAPSHOTTED note.
        vs.proposals[proposalId] = LibVotingStorage.Proposal({
            guildId: guildId,
            proposer: msg.sender,
            to: to,
            amount: amount,
            deadline: deadline,
            status: LibVotingStorage.VStatus.Active,
            forVotes: 0,
            againstVotes: 0,
            snapshotMemberCount: gs.guilds[guildId].memberCount
        });
        if (memo.length != 0) vs.memo[proposalId] = memo;
        vs.proposalsOfGuild[guildId].push(proposalId);

        emit ProposalCreated(proposalId, guildId, msg.sender, to, amount, deadline);
    }

    // --- Vote (one member, one ballot) ----------------------------------

    /// Cast ONE vote on an Active proposal. `support == true` adds to
    /// `forVotes`, false to `againstVotes` (weight 1 — one-member-one-vote
    /// MVP). VOTING ELIGIBILITY is read live from GuildFacet's storage — you
    /// must be a CURRENT member to cast a ballot (a member who joined after
    /// the proposal opened may vote; one who left may not). The QUORUM
    /// DENOMINATOR, by contrast, is the member count snapshotted at propose
    /// (`Proposal.snapshotMemberCount`), so the participation bar is fixed
    /// when voting opens even as the live roster changes (the
    /// governance-robustness fix).
    ///
    /// Reverts if: the proposal doesn't exist (`UnknownProposal`), it is not
    /// Active (`ProposalNotActive` — already executed/failed), voting has
    /// closed (`VotingClosed`, `now > deadline`), the caller is not a guild
    /// member (`NotGuildMember`), or the caller already voted
    /// (`AlreadyVoted`). The voter MAY be a contract (a member-guild TBA) —
    /// the recursive property.
    function vote(uint256 proposalId, bool support) external {
        LibVotingStorage.Storage storage vs = LibVotingStorage.load();
        LibVotingStorage.Proposal storage p = vs.proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        if (p.status != LibVotingStorage.VStatus.Active) revert ProposalNotActive();
        if (block.timestamp > p.deadline) revert VotingClosed();

        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        if (gs.roleOf[p.guildId][msg.sender] == LibGuildStorage.Role.None) revert NotGuildMember();
        if (vs.voted[proposalId][msg.sender]) revert AlreadyVoted();

        vs.voted[proposalId][msg.sender] = true;
        uint256 weight = 1; // one-member-one-vote (MVP)
        if (support) {
            p.forVotes += weight;
        } else {
            p.againstVotes += weight;
        }

        emit VoteCast(proposalId, msg.sender, support, weight);
    }

    // --- Execute (after the deadline; spend if passed) ------------------

    /// Resolve a proposal after its voting period ends. PERMISSIONLESS (the
    /// outcome is deterministic from the on-chain tally — anyone may poke
    /// it). If the proposal PASSED (quorum met AND strict majority for), it
    /// EXECUTES: debits the guild treasury and transfers `amount` `$LH` to
    /// `to`, via the inherited `GuildFacet._spend` (single source of
    /// accounting truth). Otherwise it FAILS with no spend. Either way the
    /// proposal becomes terminal — IDEMPOTENT (a second `execute` reverts
    /// `ProposalNotActive`).
    ///
    /// Reverts if: the proposal doesn't exist (`UnknownProposal`), it is not
    /// Active (`ProposalNotActive` — already resolved), or the deadline has
    /// not passed (`VotingNotEnded`). On the PASS path, `amount` is
    /// re-checked against the LIVE treasury (`AmountExceedsTreasury`) — the
    /// balance can change while the vote runs; an unaffordable passed
    /// measure reverts (and stays Active so it can be retried once the
    /// treasury is refunded), rather than silently failing.
    ///
    /// CEI: the status flips to `Executed` BEFORE `_spend`'s external
    /// transfer (first barrier), and `_spend` debits `guildBalance` before
    /// its own transfer (second barrier) — a re-entrant `execute` reverts on
    /// `ProposalNotActive` and a re-entrant `_spend` on `InsufficientTreasury`.
    function execute(uint256 proposalId) external {
        LibVotingStorage.Storage storage vs = LibVotingStorage.load();
        LibVotingStorage.Proposal storage p = vs.proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        if (p.status != LibVotingStorage.VStatus.Active) revert ProposalNotActive();
        if (block.timestamp <= p.deadline) revert VotingNotEnded();

        LibGuildStorage.Storage storage gs = LibGuildStorage.load();

        if (_passed(gs, p)) {
            // Re-check affordability against the LIVE treasury (it can have
            // changed since propose). Leave the proposal Active on shortfall
            // so it can be retried after a refund, rather than burning it.
            if (p.amount > gs.guildBalance[p.guildId]) revert AmountExceedsTreasury();

            // CEI barrier 1: terminal status BEFORE the external spend.
            p.status = LibVotingStorage.VStatus.Executed;

            // Reuse GuildFacet's internal treasury-debit core — the SAME
            // CEI-safe path `spendTreasury` uses (single accounting source).
            // `_spend` (the Admin path) is `bytes calldata`; `execute` has no
            // calldata bytes argument, so it calls the shared `_spendCore`
            // (`bytes memory`) that `_spend` itself forwards to — identical
            // ledger debit + transfer, the proposal memo carried into the
            // GuildFacet TreasurySpent event. `_spendCore` reverts on a zero
            // amount / zero recipient / insufficient treasury; propose barred
            // the first two and we re-checked the third above.
            _spendCore(gs, p.guildId, p.to, p.amount, vs.memo[proposalId]);

            emit ProposalExecuted(proposalId, p.to, p.amount);
        } else {
            p.status = LibVotingStorage.VStatus.Failed;
            emit ProposalFailed(proposalId);
        }
    }

    // --- Views ----------------------------------------------------------

    /// Full proposal record by id. Reverts `UnknownProposal` for an unknown
    /// id (so a caller can't confuse "id 0 / never created" with a real
    /// zeroed proposal). `status` is the raw VStatus (0=Active … 3=Executed).
    function getProposal(uint256 proposalId)
        external
        view
        returns (
            uint256 guildId,
            address proposer,
            address to,
            uint256 amount,
            uint64 deadline,
            uint8 status,
            uint256 forVotes,
            uint256 againstVotes
        )
    {
        LibVotingStorage.Proposal storage p = LibVotingStorage.load().proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        return (
            p.guildId,
            p.proposer,
            p.to,
            p.amount,
            p.deadline,
            uint8(p.status),
            p.forVotes,
            p.againstVotes
        );
    }

    /// The opaque measure `memo` bytes for a proposal (empty if none).
    function proposalMemoOf(uint256 proposalId) external view returns (bytes memory) {
        return LibVotingStorage.load().memo[proposalId];
    }

    /// Paginated scan of a guild's proposal ids: returns up to `limit` ids
    /// from `proposalsOfGuild[guildId]` starting at index `startAfter` (a
    /// 0-based INDEX into the per-guild list, NOT a proposal id), plus the
    /// `nextCursor` to page with. Index-window paging, identical to
    /// BountyFacet's `openBounties` / ScheduleFacet's `jobsDue`: the list is
    /// append-only so cursors stay stable across blocks.
    function proposalsOf(uint256 guildId, uint256 startAfter, uint256 limit)
        external
        view
        returns (uint256[] memory ids, uint256 nextCursor)
    {
        uint256[] storage all = LibVotingStorage.load().proposalsOfGuild[guildId];
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
    function hasVoted(uint256 proposalId, address voter) external view returns (bool) {
        return LibVotingStorage.load().voted[proposalId][voter];
    }

    /// The current tally for a proposal: `forVotes`, `againstVotes`, the
    /// live `quorum` requirement (distinct members that must vote), the
    /// `votesCast` so far (for + against), and `passing` — whether it WOULD
    /// pass right now (quorum met AND strict majority for). A read-only
    /// projection of `_passed` against the live membership; the actual
    /// outcome is fixed at `execute`.
    function tallyOf(uint256 proposalId)
        external
        view
        returns (
            uint256 forVotes,
            uint256 againstVotes,
            uint256 quorum,
            uint256 votesCast,
            bool passing
        )
    {
        LibVotingStorage.Proposal storage p = LibVotingStorage.load().proposals[proposalId];
        if (p.proposer == address(0)) revert UnknownProposal();
        LibGuildStorage.Storage storage gs = LibGuildStorage.load();
        forVotes = p.forVotes;
        againstVotes = p.againstVotes;
        // Quorum is the SNAPSHOT taken at propose (frozen denominator), not
        // the live member count — so the view agrees with the eventual
        // execute outcome regardless of membership churn.
        quorum = _quorum(p.snapshotMemberCount);
        votesCast = forVotes + againstVotes;
        passing = _passed(gs, p);
    }

    /// Total proposals ever created (== highest proposal id; ids monotonic).
    function proposalCount() external view returns (uint256) {
        return LibVotingStorage.load().nextProposalId;
    }

    // --- internals ------------------------------------------------------

    /// The quorum (minimum distinct voters) for a guild of `memberCount`
    /// members: `ceil(memberCount / 2)`. DIVIDE-BY-ZERO / degenerate guard —
    /// returns 1 for a 0- or 1-member guild, so a vote is ALWAYS required
    /// (a zero-member guild's `votesCast` is 0 < 1 = quorum → can never
    /// pass; a 1-member guild needs that one member to vote).
    function _quorum(uint64 memberCount) internal pure returns (uint256) {
        if (memberCount <= 1) return 1;
        return (uint256(memberCount) + 1) / 2; // ceil-of-half
    }

    /// Whether a proposal passes: quorum met (distinct votes cast >=
    /// ceil(SNAPSHOT members / 2)) AND a STRICT majority for (for > against; a
    /// tie fails). The quorum denominator is the member count SNAPSHOTTED at
    /// propose (`p.snapshotMemberCount`), NOT the live count — membership
    /// churn between propose and execute can't move the bar (the
    /// governance-robustness fix). `gs` is retained in the signature for the
    /// caller's storage-handle convention / future weight upgrades but the
    /// quorum no longer reads from it.
    function _passed(LibGuildStorage.Storage storage gs, LibVotingStorage.Proposal storage p)
        internal
        view
        returns (bool)
    {
        gs; // silence unused-parameter (weight is 1; quorum is the snapshot)
        uint256 votesCast = p.forVotes + p.againstVotes;
        uint256 quorum = _quorum(p.snapshotMemberCount);
        if (votesCast < quorum) return false; // quorum not met
        return p.forVotes > p.againstVotes; // strict majority for
    }
}
