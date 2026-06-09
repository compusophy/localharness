// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibBountyStorage} from "../libraries/LibBountyStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// The TBA-resolution surface BountyFacet reaches for the payout. In
/// production this is a SELF-call: the facet runs via the diamond's
/// delegatecall, so `address(this)` is the diamond and routes to
/// `TbaFacet.tokenBoundAccount` (same diamond, same on-chain reads). Kept
/// as an explicit interface so the dependency is legible and the test
/// harness can satisfy it by implementing the one selector.
interface ITbaResolver {
    function tokenBoundAccount(uint256 tokenId) external view returns (address);
}

/// @title BountyFacet
/// @notice The agent-economy DEMAND primitive — Rung 1 of the coordination
///         ladder (design/agent-coordination.md). An agent POSTS a task and
///         ESCROWS a `$LH` reward; another agent CLAIMS it, does the work,
///         and SUBMITS a result; the poster ACCEPTS → the reward settles to
///         the worker's agent TBA. The first one→many coordination
///         marketplace, the demand engine the whole ladder needs.
///
///         REUSED PRIMITIVE — it is `InviteFacet`'s escrow state-machine
///         with a richer release condition (§Rung 1): "accept by the poster
///         confirming the submitted result" instead of "accept by knowing a
///         code." Escrow mechanics (`transferFrom` poster→diamond on post,
///         CEI status-flips before every payout/refund) are copied verbatim;
///         only the lifecycle states differ.
///
///         LIFECYCLE (the ABI-pinned Status enum):
///           postBounty   → Open    (poster escrows rewardWei)
///           claimBounty  → Claimed (a worker reserves it; records the
///                                    claimant identity, payout-bound)
///           submitResult → Submitted (worker commits the result bytes)
///           acceptResult → Paid    (poster confirms → reward → worker TBA)
///         plus two refund exits:
///           cancelBounty   (poster, while Open only) → Cancelled (refund)
///           reclaimExpired (anyone, after expiry)    → Reclaimed (refund)
///
///         CEI ON EVERY `$LH` MOVE (escrow / payout / refund). Each
///         function commits the terminal/next status + the per-poster
///         active-count BEFORE the external token transfer, so a hostile
///         re-entrant token re-reads a non-Open / non-Submitted status and
///         reverts — no double-payout, no double-refund, no escrow drain.
///         Proven by the reentrant-token probe in the test suite.
///
///         THE CLAIM-SQUAT / TRUST NOTE (the genuinely hard part —
///         trust=0 "poster accepts," testnet, `$LH`-credit). The MVP trust
///         model is "the poster is the oracle": only the poster can
///         `acceptResult`. Honest failure modes, both bounded here:
///           • Worker claims and never delivers → the poster's escrow is
///             NOT trapped: `reclaimExpired` refunds 100% after the TTL,
///             permissionlessly. The claim soft-locks the bounty but the
///             deadline is the hard release.
///           • CLAIM-SQUAT / claim-as-someone-else → claiming records the
///             caller-supplied `claimantTokenId`, and the payout is BOUND
///             to THAT identity's TBA. So claiming a bounty "as" another
///             agent just pays THAT agent — there is no theft vector, only
///             a (benign) gift. A squatter who claims to block others still
///             can't redirect the reward to themselves, and the poster can
///             let the bounty expire and re-post. Documented, not "fixed,"
///             because it is not a vulnerability — it is the design.
///         The `trust=1` staked-validator and `trust=2` ERC-8004
///         reputation modes are additive cuts later (mainnet-gated); the
///         seam is the per-bounty acceptance gate, not this facet's shape.
///
///         GAS / STORAGE: task + result PROSE is the caller's problem — the
///         intended payload is a hash or off-chain pointer (CLAUDE.md
///         ~7.6k gas/byte). The facet caps both at MAX_TASK_BYTES /
///         MAX_RESULT_BYTES so an unbounded blob can't be escrowed into one
///         SSTORE chain, but does NOT mandate a hash — short inline specs
///         are allowed under the cap.
///
///         CUTTING IT (diamond owner; mirror script/AddInviteFacet):
///         deploy + diamondCut Add the 10 selectors in
///         script/AddBountyFacet.s.sol. No post-cut config — the credits
///         token is read from the shared CreditsFacet storage slot and the
///         TBA resolver is the diamond itself.
contract BountyFacet {
    // --- Events (indexed for off-chain harvest / the discovery board) ---

    event BountyPosted(
        uint256 indexed id, address indexed poster, uint128 rewardWei, uint64 expiry
    );
    event BountyClaimed(uint256 indexed id, uint256 indexed claimantTokenId, address indexed claimant);
    event ResultSubmitted(uint256 indexed id, uint256 indexed claimantTokenId);
    event ResultAccepted(
        uint256 indexed id, uint256 indexed claimantTokenId, address worker, uint128 rewardWei
    );
    event BountyCancelled(uint256 indexed id, address indexed poster, uint128 rewardWei);
    event BountyReclaimed(uint256 indexed id, address indexed poster, uint128 rewardWei);

    // --- Errors ---------------------------------------------------------

    error NotConfigured(); // credits token unset
    error ZeroReward();
    error BadTtl(); // ttl < MIN_TTL || ttl > MAX_TTL
    error TaskTooLarge(); // task bytes over MAX_TASK_BYTES
    error ResultTooLarge(); // result bytes over MAX_RESULT_BYTES
    error TooManyActiveBounties(); // poster already at MAX_ACTIVE_PER_POSTER
    error UnknownBounty(); // no such id
    error NotOpen(); // claim/cancel on a non-Open bounty
    error NotClaimed(); // submit on a non-Claimed bounty
    error NotSubmitted(); // accept on a non-Submitted bounty
    error Expired(); // claim after expiry
    error NotYetExpired(); // reclaim before expiry
    error NotReclaimable(); // reclaim on a terminal (already-settled) bounty
    error NotPoster(); // poster-only gate
    error UnknownClaimant(); // claimantTokenId is not a registered identity
    error TbaUnresolved(); // the claimant's TBA resolved to address(0)

    // --- Post (permissionless; poster escrows their own $LH) ------------

    /// Post a bounty: escrow `rewardWei` `$LH` (`transferFrom`
    /// poster→diamond; approve the diamond first — the bundle batches
    /// approve + postBounty into one sponsored tx, exactly like
    /// `createInvite` / `scheduleJob`), store the `task` spec, set
    /// `expiry = now + ttlSeconds`, status Open. Returns the new id.
    ///
    /// Rejects a zero reward, a ttl outside [MIN_TTL, MAX_TTL], an
    /// over-cap task blob, and a poster already at MAX_ACTIVE_PER_POSTER
    /// live bounties.
    ///
    /// CEI: the WHOLE bounty record + the active-count bump + the index
    /// writes land BEFORE the external `transferFrom`, so a failed pull
    /// reverts the whole tx and leaves NO ghost bounty (and no consumed
    /// id, since the counter increment reverts with it).
    function postBounty(bytes calldata task, uint128 rewardWei, uint64 ttlSeconds)
        external
        returns (uint256 bountyId)
    {
        if (rewardWei == 0) revert ZeroReward();
        if (ttlSeconds < LibBountyStorage.MIN_TTL || ttlSeconds > LibBountyStorage.MAX_TTL) {
            revert BadTtl();
        }
        if (task.length > LibBountyStorage.MAX_TASK_BYTES) revert TaskTooLarge();

        LibBountyStorage.Storage storage s = LibBountyStorage.load();
        if (s.activeOf[msg.sender] >= LibBountyStorage.MAX_ACTIVE_PER_POSTER) {
            revert TooManyActiveBounties();
        }

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // Commit all state BEFORE the escrow pull (CEI).
        s.activeOf[msg.sender] += 1;
        bountyId = ++s.nextBountyId; // ids start at 1
        uint64 expiry = uint64(block.timestamp) + ttlSeconds;
        s.bounties[bountyId] = LibBountyStorage.Bounty({
            poster: msg.sender,
            expiry: expiry,
            status: LibBountyStorage.Status.Open,
            rewardWei: rewardWei,
            claimantTokenId: 0
        });
        s.task[bountyId] = task;
        s.bountyIds.push(bountyId);
        s.bountiesOfPoster[msg.sender].push(bountyId);

        // CEI: escrow LAST. A failed pull reverts everything above with it.
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), rewardWei),
            "bounty: escrow failed"
        );

        emit BountyPosted(bountyId, msg.sender, rewardWei, expiry);
    }

    // --- Claim (a worker reserves the task; first-come soft-lock) -------

    /// Claim an Open, unexpired bounty, recording `claimantTokenId` — the
    /// agent identity whose TBA the reward will settle to on acceptance.
    /// First-come reserve: once claimed, no one else can claim until the
    /// bounty expires and is reclaimed/re-posted.
    ///
    /// PAYOUT IS BOUND TO `claimantTokenId`'s TBA. Claiming "as" another
    /// identity therefore just pays THAT identity — there is no theft
    /// vector (only a benign gift). See the contract-level claim-squat /
    /// trust note. `claimantTokenId` MUST be a registered identity (its
    /// tokenId has an owner) so a bogus id can't strand the payout.
    function claimBounty(uint256 bountyId, uint256 claimantTokenId) external {
        LibBountyStorage.Storage storage s = LibBountyStorage.load();
        LibBountyStorage.Bounty storage b = s.bounties[bountyId];

        if (b.poster == address(0)) revert UnknownBounty();
        if (b.status != LibBountyStorage.Status.Open) revert NotOpen();
        if (block.timestamp > b.expiry) revert Expired();
        // The claimant identity must exist (a registered tokenId). Without
        // this, acceptResult would later resolve a TBA for a phantom id.
        if (LibRegistryStorage.load().ownerOfId[claimantTokenId] == address(0)) {
            revert UnknownClaimant();
        }

        b.status = LibBountyStorage.Status.Claimed;
        b.claimantTokenId = claimantTokenId;

        emit BountyClaimed(bountyId, claimantTokenId, msg.sender);
    }

    // --- Submit (the worker commits the result) -------------------------

    /// Submit a result for a Claimed bounty. Stores the `result` bytes
    /// (hash / pointer, gas-bounded like the task) and flips Claimed →
    /// Submitted. PERMISSIONLESS to call (anyone may push the result for
    /// the claimed identity — the payout is still bound to
    /// `claimantTokenId`'s TBA, so there's no benefit to submitting on
    /// someone else's behalf beyond helping them). The poster's accept is
    /// the only gate that releases money.
    function submitResult(uint256 bountyId, bytes calldata result) external {
        if (result.length > LibBountyStorage.MAX_RESULT_BYTES) revert ResultTooLarge();

        LibBountyStorage.Storage storage s = LibBountyStorage.load();
        LibBountyStorage.Bounty storage b = s.bounties[bountyId];

        if (b.poster == address(0)) revert UnknownBounty();
        if (b.status != LibBountyStorage.Status.Claimed) revert NotClaimed();

        b.status = LibBountyStorage.Status.Submitted;
        s.result[bountyId] = result;

        emit ResultSubmitted(bountyId, b.claimantTokenId);
    }

    // --- Accept (poster confirms → reward settles to the worker TBA) ----

    /// Accept a Submitted bounty. POSTER-ONLY. Pays `rewardWei` to the
    /// token-bound account of `claimantTokenId` (the worker's agent
    /// wallet, resolved via the diamond's `TbaFacet.tokenBoundAccount`)
    /// and flips Submitted → Paid.
    ///
    /// CEI: status → Paid + the active-count decrement land BEFORE the
    /// payout `transfer`, so a re-entrant token re-reads `status != Submitted`
    /// and reverts (no double-payout). The reward field is read into a
    /// local before the status flip and the transfer is the last action.
    function acceptResult(uint256 bountyId) external {
        LibBountyStorage.Storage storage s = LibBountyStorage.load();
        LibBountyStorage.Bounty storage b = s.bounties[bountyId];

        if (b.poster == address(0)) revert UnknownBounty();
        if (msg.sender != b.poster) revert NotPoster();
        if (b.status != LibBountyStorage.Status.Submitted) revert NotSubmitted();

        uint128 reward = b.rewardWei;
        uint256 claimantTokenId = b.claimantTokenId;
        address poster = b.poster;

        // Resolve the worker's TBA via the diamond (self-call to TbaFacet).
        // Done BEFORE the status flip so a resolution revert leaves the
        // bounty Submitted (the poster can retry once the TBA is set), and
        // a zero-address resolution is rejected (never burn the reward).
        address workerTba = ITbaResolver(address(this)).tokenBoundAccount(claimantTokenId);
        if (workerTba == address(0)) revert TbaUnresolved();

        // CEI: terminal state + active-count BEFORE the payout.
        b.status = LibBountyStorage.Status.Paid;
        s.activeOf[poster] -= 1;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(workerTba, reward), "bounty: payout failed");

        emit ResultAccepted(bountyId, claimantTokenId, workerTba, reward);
    }

    // --- Cancel (poster aborts an unclaimed bounty; full refund) --------

    /// Cancel an Open (not-yet-claimed) bounty. POSTER-ONLY. Refunds the
    /// full `rewardWei` to the poster and flips Open → Cancelled. Only
    /// valid while Open — once a worker has claimed, the poster can't pull
    /// the reward out from under them (they wait for the deadline +
    /// `reclaimExpired` instead).
    ///
    /// CEI: status → Cancelled + active-count decrement BEFORE the refund.
    function cancelBounty(uint256 bountyId) external {
        LibBountyStorage.Storage storage s = LibBountyStorage.load();
        LibBountyStorage.Bounty storage b = s.bounties[bountyId];

        if (b.poster == address(0)) revert UnknownBounty();
        if (msg.sender != b.poster) revert NotPoster();
        if (b.status != LibBountyStorage.Status.Open) revert NotOpen();

        uint128 reward = b.rewardWei;
        address poster = b.poster;

        b.status = LibBountyStorage.Status.Cancelled;
        s.activeOf[poster] -= 1;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(poster, reward), "bounty: refund failed");

        emit BountyCancelled(bountyId, poster, reward);
    }

    // --- Reclaim (anyone pokes an expired bounty; refunds the poster) ---

    /// Reclaim an EXPIRED bounty whose work was never accepted. PERMISSION-
    /// LESS to call (anyone can poke it), but the refund ALWAYS goes to the
    /// POSTER, never `msg.sender` — a third-party caller gains nothing. Valid
    /// only when `now > expiry` AND the status is still in {Open, Claimed,
    /// Submitted} (i.e. the escrow hasn't already settled). Flips → Reclaimed.
    /// This is the worker-griefs-poster hard stop: a claimant who never
    /// delivers (or a result the poster won't accept) can't trap the escrow
    /// past the deadline.
    ///
    /// CEI: status → Reclaimed + active-count decrement BEFORE the refund.
    function reclaimExpired(uint256 bountyId) external {
        LibBountyStorage.Storage storage s = LibBountyStorage.load();
        LibBountyStorage.Bounty storage b = s.bounties[bountyId];

        if (b.poster == address(0)) revert UnknownBounty();
        // Only the non-terminal (escrow-still-locked) states are reclaimable.
        LibBountyStorage.Status st = b.status;
        if (
            st != LibBountyStorage.Status.Open && st != LibBountyStorage.Status.Claimed
                && st != LibBountyStorage.Status.Submitted
        ) {
            revert NotReclaimable();
        }
        if (block.timestamp <= b.expiry) revert NotYetExpired();

        uint128 reward = b.rewardWei;
        address poster = b.poster;

        b.status = LibBountyStorage.Status.Reclaimed;
        s.activeOf[poster] -= 1;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(poster, reward), "bounty: reclaim failed");

        emit BountyReclaimed(bountyId, poster, reward);
    }

    // --- Views (the discovery surface) ----------------------------------

    /// Full bounty record by id. Returns zeros for an unknown id
    /// (poster == address(0)).
    function getBounty(uint256 bountyId)
        external
        view
        returns (
            address poster,
            uint128 rewardWei,
            uint64 expiry,
            uint8 status,
            uint256 claimantTokenId
        )
    {
        LibBountyStorage.Bounty storage b = LibBountyStorage.load().bounties[bountyId];
        return (b.poster, b.rewardWei, b.expiry, uint8(b.status), b.claimantTokenId);
    }

    /// The task spec bytes (hash / pointer / short prose) for a bounty.
    function bountyTaskOf(uint256 bountyId) external view returns (bytes memory) {
        return LibBountyStorage.load().task[bountyId];
    }

    /// The submitted result bytes for a bounty (empty until submitResult).
    function resultOf(uint256 bountyId) external view returns (bytes memory) {
        return LibBountyStorage.load().result[bountyId];
    }

    /// Paginated scan of OPEN, unexpired bounties: returns up to `limit`
    /// ids of bounties with status Open AND `expiry >= now`, scanning
    /// `bountyIds` after position `startAfter` (a 0-based INDEX into
    /// `bountyIds`, NOT a bounty id). Callers page with the returned
    /// `nextCursor` (the index scanned up to) until it comes back empty or
    /// short. Index-window paging, identical to ScheduleFacet's `jobsDue`:
    /// the cursor is index-based + bountyIds is append-only, so pagination
    /// is stable across blocks. A flat scan for the MVP; a status-bucketed
    /// index is the scale path.
    function openBounties(uint256 startAfter, uint256 limit)
        external
        view
        returns (uint256[] memory ids, uint256 nextCursor)
    {
        LibBountyStorage.Storage storage s = LibBountyStorage.load();
        uint256 total = s.bountyIds.length;
        if (startAfter >= total || limit == 0) {
            return (new uint256[](0), total);
        }
        uint256 nowTs = block.timestamp;
        // First pass: count matches in the [startAfter, scanned-limit)
        // window so the result array is sized exactly (view = free gas).
        uint256 scanned = 0;
        uint256 matches = 0;
        uint256 i = startAfter;
        while (i < total && scanned < limit) {
            LibBountyStorage.Bounty storage b = s.bounties[s.bountyIds[i]];
            if (b.status == LibBountyStorage.Status.Open && b.expiry >= nowTs) {
                matches++;
            }
            i++;
            scanned++;
        }
        nextCursor = i;
        ids = new uint256[](matches);
        uint256 k = 0;
        uint256 m = startAfter;
        uint256 scanned2 = 0;
        while (m < i && scanned2 < limit) {
            uint256 bid = s.bountyIds[m];
            LibBountyStorage.Bounty storage b = s.bounties[bid];
            if (b.status == LibBountyStorage.Status.Open && b.expiry >= nowTs) {
                ids[k++] = bid;
            }
            m++;
            scanned2++;
        }
    }

    /// Every bounty id a given poster has posted (live + terminal).
    function bountiesOf(address poster) external view returns (uint256[] memory) {
        return LibBountyStorage.load().bountiesOfPoster[poster];
    }

    /// Total bounties ever posted (== highest bounty id; ids are monotonic).
    function bountyCount() external view returns (uint256) {
        return LibBountyStorage.load().nextBountyId;
    }

    /// The count of a poster's currently-LIVE (Open/Claimed/Submitted)
    /// bounties — the anti-sybil cap key. Read it to know how many slots
    /// remain before MAX_ACTIVE_PER_POSTER.
    function activeBountyCountOf(address poster) external view returns (uint256) {
        return LibBountyStorage.load().activeOf[poster];
    }
}
