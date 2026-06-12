// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibPartyStorage} from "../libraries/LibPartyStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// The TBA-resolution surface PartyFacet reaches for the member payouts. In
/// production this is a SELF-call: the facet runs via the diamond's
/// delegatecall, so `address(this)` is the diamond and routes to
/// `TbaFacet.tokenBoundAccount` (same diamond, same on-chain reads). Kept
/// as an explicit interface so the dependency is legible and the test
/// harness can satisfy it by implementing the one selector.
interface ITbaResolver {
    function tokenBoundAccount(uint256 tokenId) external view returns (address);
}

/// @title PartyFacet
/// @notice Agent PARTIES (ad-hoc squads / "raids") — Rung 2 of the
///         coordination ladder (design/shipped/agent-coordination.md): an
///         EPHEMERAL squad formed around ONE objective (often a bounty) with
///         a PRE-AGREED reward split, that settles and dissolves. The
///         lightweight counterpart of a guild: no roles, no standing
///         treasury, no identity NFT — one pot, one split, one outcome.
///
///         LIFECYCLE (the ABI-pinned Status enum):
///           formParty    → Forming  (creator proposes members + sharesBps
///                                     summing to 10000; creator-owned seats
///                                     auto-consent)
///           joinParty    → Forming/Active (each member identity's OWNER
///                                     consents their seats; full consent →
///                                     Active — the design's "consent over
///                                     the money, not just membership":
///                                     shares are fixed at formParty, so
///                                     consenting IS signing the split)
///           fundParty    → escrow   (anyone pulls their own `$LH` into the
///                                     pot, Forming or Active, pre-expiry)
///           completeParty→ Completed (CREATOR settles: the pot splits to
///                                     each member's TBA by shares, the
///                                     remainder to the LAST member so the
///                                     split conserves the escrow EXACTLY)
///         plus the refund exit:
///           disbandParty (creator any time; ANYONE once expired)
///                        → Disbanded (every funder refunded their exact
///                                     contribution — the design's
///                                     "abort+reclaim for MVP")
///
///         REUSED PRIMITIVES (the design's framing): BountyFacet's escrow +
///         TBA payout (`transferFrom` funder→diamond, payout bound to an
///         IDENTITY's TBA), GuildFacet's consent-gated membership (no one is
///         conscripted), InviteFacet's disjoint settle/refund windows
///         (complete requires `now <= expiry`; permissionless disband
///         requires `now > expiry`).
///
///         CEI ON EVERY `$LH` MOVE (escrow / split / refund). Each function
///         commits the terminal/next status + the per-creator active-count
///         BEFORE any external token transfer, so a hostile re-entrant token
///         re-reads a non-live status and reverts — no double-split, no
///         double-refund, no escrow drain. Proven by the reentrant-token
///         probes in the test suite.
///
///         THE TRUST MODEL (MVP, mirroring BountyFacet's poster-is-oracle):
///         the CREATOR is the settlement oracle — only they can
///         `completeParty`. Honest failure modes, both bounded:
///           • Creator never settles → funders are NOT trapped: after the
///             TTL anyone can `disbandParty` and every funder is refunded
///             100%. The deadline is the hard release.
///           • Members never consent → the party never turns Active, the
///             pot can never split; disband/expiry refunds the funders.
///         A member-quorum or unanimous-member complete is the documented
///         upgrade seam (the internal `_settle` is the single split path a
///         future gate would call), not this facet's MVP shape.
///
///         SELECTOR NOTE: every view is `party`-prefixed (`partyMembersOf`,
///         NOT `membersOf` — TeamFacet owns `membersOf(uint256)`;
///         `partyCount`, not a bare `count`) — the BountyFacet
///         `bountyTaskOf`-vs-`taskOf` lesson: a diamond can't share a
///         selector across facets.
///
///         CUTTING IT (diamond owner; mirror script/AddBountyFacet): deploy
///         + diamondCut Add the 15 selectors in script/AddPartyFacet.s.sol.
///         No post-cut config — the credits token is read from the shared
///         CreditsFacet storage slot, the member-TBA resolver is the diamond
///         itself (TbaFacet must already be cut, which it is on the live
///         diamond).
contract PartyFacet {
    // --- Events (indexed for off-chain harvest / the squad board) --------

    event PartyFormed(
        uint256 indexed partyId, address indexed creator, uint64 expiry, uint256 memberCount
    );
    event PartyJoined(uint256 indexed partyId, uint256 indexed memberTokenId, address indexed owner);
    event PartyActivated(uint256 indexed partyId);
    event PartyFunded(uint256 indexed partyId, address indexed funder, uint128 amount);
    event PartyMemberPaid(
        uint256 indexed partyId, uint256 indexed memberTokenId, address tba, uint256 amount
    );
    event PartyCompleted(uint256 indexed partyId, uint128 escrowWei);
    event PartyDisbanded(uint256 indexed partyId, uint128 refundedWei);

    // --- Errors -----------------------------------------------------------

    error NotConfigured(); // credits token unset
    error UnknownParty(); // no such id
    error BadMembers(); // empty / over-cap / length-mismatched member list
    error BadShares(); // a zero share, or the sum != 10000 bps
    error DuplicateMember(); // the same tokenId listed twice
    error UnknownMember(); // a member tokenId is not a registered identity
    error BadTtl(); // ttl < MIN_TTL || ttl > MAX_TTL
    error TooManyActiveParties(); // creator already at MAX_ACTIVE_PER_CREATOR
    error TooManyFunders(); // distinct-funder cap reached
    error NotForming(); // join on a non-Forming party
    error NotActive(); // complete on a non-Active party
    error NotLive(); // fund/disband on a terminal party
    error NotCreator(); // creator-only gate (completeParty)
    error NotDisbandable(); // non-creator disband before expiry
    error NothingToConsent(); // joinParty by an address owning no unconsented seat
    error Expired(); // join/fund/complete after expiry
    error ZeroAmount(); // fund of 0
    error TbaUnresolved(); // a member's TBA resolved to address(0)

    // --- Form (permissionless; the creator proposes the squad + split) ---

    /// Propose a party: `memberTokenIds[i]` gets `sharesBps[i]` of the pot
    /// (basis points; the vector MUST sum to exactly 10000 with no zero
    /// share — a zero-share seat is a freeloading consent veto). Every
    /// member must be a REGISTERED identity (its TBA is the payout target),
    /// listed once. `expiry = now + ttlSeconds` bounds the whole lifecycle.
    /// Returns the new partyId. Status starts Forming; seats whose tokenId
    /// the CREATOR owns auto-consent (forming is consenting), so a party of
    /// only the creator's own agents starts Active immediately.
    ///
    /// Shares are FIXED here, before any consent: a member who joins is
    /// consenting to this exact split (the design's "encode shares only
    /// after all members accept" inverted into the equivalent safe form —
    /// accept happens AFTER the shares are pinned, so nothing can be
    /// re-split under a consenting member).
    function formParty(
        uint256[] calldata memberTokenIds,
        uint16[] calldata sharesBps,
        uint64 ttlSeconds
    ) external returns (uint256 partyId) {
        if (
            memberTokenIds.length == 0
                || memberTokenIds.length > LibPartyStorage.MAX_PARTY_MEMBERS
                || sharesBps.length != memberTokenIds.length
        ) revert BadMembers();
        if (ttlSeconds < LibPartyStorage.MIN_TTL || ttlSeconds > LibPartyStorage.MAX_TTL) {
            revert BadTtl();
        }
        _validateMembersAndShares(memberTokenIds, sharesBps);

        LibPartyStorage.Storage storage s = LibPartyStorage.load();
        if (s.activeOf[msg.sender] >= LibPartyStorage.MAX_ACTIVE_PER_CREATOR) {
            revert TooManyActiveParties();
        }

        s.activeOf[msg.sender] += 1;
        partyId = ++s.nextPartyId; // ids start at 1
        uint64 expiry = uint64(block.timestamp) + ttlSeconds;
        {
            LibPartyStorage.Party storage p = s.parties[partyId];
            p.creator = msg.sender;
            p.expiry = expiry;
            p.status = LibPartyStorage.Status.Forming;
        }
        s.members[partyId] = memberTokenIds;
        s.shares[partyId] = sharesBps;
        s.partyIds.push(partyId);
        s.partiesOfCreator[msg.sender].push(partyId);

        emit PartyFormed(partyId, msg.sender, expiry, memberTokenIds.length);
        _autoConsentCreatorSeats(s, partyId, memberTokenIds);
    }

    /// formParty's validation half (split out for stack depth): every share
    /// nonzero and summing to exactly 10000 bps; every member a registered
    /// identity, listed once (O(n^2) dup scan — n <= 16, cheaper than a
    /// transient mapping).
    function _validateMembersAndShares(
        uint256[] calldata memberTokenIds,
        uint16[] calldata sharesBps
    ) internal view {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        uint256 sum = 0;
        for (uint256 i = 0; i < memberTokenIds.length; i++) {
            if (sharesBps[i] == 0) revert BadShares();
            sum += sharesBps[i];
            if (rs.ownerOfId[memberTokenIds[i]] == address(0)) revert UnknownMember();
            for (uint256 j = 0; j < i; j++) {
                if (memberTokenIds[j] == memberTokenIds[i]) revert DuplicateMember();
            }
        }
        if (sum != LibPartyStorage.TOTAL_SHARES_BPS) revert BadShares();
    }

    /// formParty's auto-consent half (split out for stack depth): seats
    /// whose tokenId the CREATOR owns consent at formation (the creator
    /// formed the split — they obviously agree to it). A fully
    /// creator-owned party goes straight to Active.
    function _autoConsentCreatorSeats(
        LibPartyStorage.Storage storage s,
        uint256 partyId,
        uint256[] calldata memberTokenIds
    ) internal {
        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        uint16 accepted = 0;
        for (uint256 i = 0; i < memberTokenIds.length; i++) {
            if (rs.ownerOfId[memberTokenIds[i]] == msg.sender) {
                s.consented[partyId][memberTokenIds[i]] = true;
                accepted += 1;
                emit PartyJoined(partyId, memberTokenIds[i], msg.sender);
            }
        }
        LibPartyStorage.Party storage p = s.parties[partyId];
        p.acceptedCount = accepted;
        if (accepted == memberTokenIds.length) {
            p.status = LibPartyStorage.Status.Active;
            emit PartyActivated(partyId);
        }
    }

    // --- Join (the consent half: each seat's OWNER signs the split) ------

    /// Consent to membership: marks consented every member seat whose
    /// tokenId the CALLER currently owns (one call covers all your seats —
    /// an owner of several listed agents consents them all). Reverts
    /// `NothingToConsent` if the caller owns no unconsented seat — a
    /// stranger can't "join" a party they weren't named in. When the last
    /// seat consents the party turns Active (fundable + completable).
    /// Forming-only and pre-expiry: an expired proposal is refund-only.
    function joinParty(uint256 partyId) external {
        LibPartyStorage.Storage storage s = LibPartyStorage.load();
        LibPartyStorage.Party storage p = s.parties[partyId];
        if (p.creator == address(0)) revert UnknownParty();
        if (p.status != LibPartyStorage.Status.Forming) revert NotForming();
        if (block.timestamp > p.expiry) revert Expired();

        LibRegistryStorage.Storage storage rs = LibRegistryStorage.load();
        uint256[] storage mem = s.members[partyId];
        uint256 n = mem.length;
        uint16 newlyConsented = 0;
        for (uint256 i = 0; i < n; i++) {
            uint256 tokenId = mem[i];
            if (!s.consented[partyId][tokenId] && rs.ownerOfId[tokenId] == msg.sender) {
                s.consented[partyId][tokenId] = true;
                newlyConsented += 1;
                emit PartyJoined(partyId, tokenId, msg.sender);
            }
        }
        if (newlyConsented == 0) revert NothingToConsent();

        p.acceptedCount += newlyConsented;
        if (p.acceptedCount == n) {
            p.status = LibPartyStorage.Status.Active;
            emit PartyActivated(partyId);
        }
    }

    // --- Fund (anyone escrows their own $LH into the pot) ----------------

    /// Fund the party's pot: pull `amount` `$LH` (`transferFrom`
    /// funder→diamond; approve the diamond first — the bundle batches
    /// approve + fundParty into one sponsored tx, exactly like `postBounty`
    /// / `fundGuild`) and record the contribution per funder (the disband
    /// refund key). PERMISSIONLESS — the creator, a member's owner, or an
    /// interested third party (a bounty poster staking a side-pot) may all
    /// fund. Allowed while Forming OR Active (funding a proposal is safe:
    /// if it never forms, disband/expiry refunds you exactly), but only
    /// pre-expiry — an expired party is refund-only.
    ///
    /// CEI: the escrow + funder ledger land BEFORE the external pull, so a
    /// failed pull (under-allowance / under-balance) reverts the whole tx —
    /// no ghost contribution.
    function fundParty(uint256 partyId, uint128 amount) external {
        if (amount == 0) revert ZeroAmount();
        LibPartyStorage.Storage storage s = LibPartyStorage.load();
        LibPartyStorage.Party storage p = s.parties[partyId];
        if (p.creator == address(0)) revert UnknownParty();
        if (
            p.status != LibPartyStorage.Status.Forming
                && p.status != LibPartyStorage.Status.Active
        ) revert NotLive();
        if (block.timestamp > p.expiry) revert Expired();

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // First-time funders take an enumerable slot (the refund loop's
        // bound); repeat contributions just grow their ledger entry.
        if (s.fundedBy[partyId][msg.sender] == 0) {
            if (s.funders[partyId].length >= LibPartyStorage.MAX_FUNDERS) {
                revert TooManyFunders();
            }
            s.funders[partyId].push(msg.sender);
        }

        // CEI: ledger BEFORE the pull. A revert in transferFrom unwinds it.
        s.fundedBy[partyId][msg.sender] += amount;
        p.escrowWei += amount;
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), amount),
            "party: fund failed"
        );

        emit PartyFunded(partyId, msg.sender, amount);
    }

    // --- Complete (creator settles: the pot splits to member TBAs) -------

    /// Settle the party. CREATOR-ONLY (the MVP oracle, mirroring
    /// BountyFacet's poster-accepts). Requires Active (every member
    /// consented — no split without full consent over the money) and
    /// `now <= expiry` (past the deadline the refund window owns the
    /// escrow; the windows are disjoint, the InviteFacet discipline).
    ///
    /// The split: member `i` receives `escrow * sharesBps[i] / 10000`,
    /// except the LAST member, who receives the REMAINDER — so the payouts
    /// sum to the escrow EXACTLY (no rounding dust stranded in the
    /// diamond). Each payout settles to the member identity's TBA (resolved
    /// via the diamond's TbaFacet, the BountyFacet payout path). ALL TBAs
    /// are resolved + zero-checked BEFORE the status flip, so a resolution
    /// failure leaves the party Active (the creator deploys the TBA and
    /// retries) and a zero-address can never eat a share.
    ///
    /// CEI: status → Completed + the active-count decrement land BEFORE the
    /// payout transfers, so a re-entrant token re-reads `status != Active`
    /// and reverts (no double-split). A zero-escrow complete is a pure
    /// dissolution (no transfers).
    function completeParty(uint256 partyId) external {
        LibPartyStorage.Storage storage s = LibPartyStorage.load();
        LibPartyStorage.Party storage p = s.parties[partyId];
        if (p.creator == address(0)) revert UnknownParty();
        if (msg.sender != p.creator) revert NotCreator();
        if (p.status != LibPartyStorage.Status.Active) revert NotActive();
        if (block.timestamp > p.expiry) revert Expired();

        uint256[] storage mem = s.members[partyId];
        uint16[] storage bps = s.shares[partyId];
        uint256 n = mem.length;
        uint128 escrow = p.escrowWei;

        // Resolve every member TBA BEFORE the status flip (a revert here
        // leaves the party Active; a zero address is never paid).
        address[] memory tbas = new address[](n);
        if (escrow > 0) {
            for (uint256 i = 0; i < n; i++) {
                address tba = ITbaResolver(address(this)).tokenBoundAccount(mem[i]);
                if (tba == address(0)) revert TbaUnresolved();
                tbas[i] = tba;
            }
        }

        // CEI: terminal state + active-count BEFORE the payouts.
        p.status = LibPartyStorage.Status.Completed;
        s.activeOf[p.creator] -= 1;

        if (escrow > 0) {
            address token = LibCreditsStorage.load().creditsToken;
            if (token == address(0)) revert NotConfigured();
            uint256 paid = 0;
            for (uint256 i = 0; i < n; i++) {
                // Last member takes the remainder — the split sums to the
                // escrow exactly (conservation by construction).
                uint256 cut = i == n - 1
                    ? uint256(escrow) - paid
                    : (uint256(escrow) * bps[i]) / LibPartyStorage.TOTAL_SHARES_BPS;
                paid += cut;
                if (cut > 0) {
                    require(IERC20Min(token).transfer(tbas[i], cut), "party: payout failed");
                    emit PartyMemberPaid(partyId, mem[i], tbas[i], cut);
                }
            }
        }

        emit PartyCompleted(partyId, escrow);
    }

    // --- Disband (refund exit: creator any time, anyone after expiry) ----

    /// Dissolve the party and refund EVERY funder their exact contribution.
    /// The CREATOR may disband a live (Forming/Active) party at any time
    /// (abort); after expiry ANYONE may poke it (permissionless, the
    /// `reclaimExpired` precedent) — but the refunds ALWAYS go to the
    /// FUNDERS, never `msg.sender`, so a third-party caller gains nothing.
    /// This is the funder-griefs hard stop: a creator who never settles (or
    /// a squad that never consents) can't trap the pot past the deadline.
    ///
    /// CEI: status → Disbanded + the active-count decrement land BEFORE the
    /// refund transfers, so a re-entrant token re-reads a terminal status
    /// and reverts (no double-refund). The refund loop is bounded by
    /// MAX_FUNDERS; each funder gets `fundedBy[partyId][funder]` — summing
    /// to the escrow exactly (the ledger IS the escrow decomposition).
    function disbandParty(uint256 partyId) external {
        LibPartyStorage.Storage storage s = LibPartyStorage.load();
        LibPartyStorage.Party storage p = s.parties[partyId];
        if (p.creator == address(0)) revert UnknownParty();
        if (
            p.status != LibPartyStorage.Status.Forming
                && p.status != LibPartyStorage.Status.Active
        ) revert NotLive();
        if (msg.sender != p.creator && block.timestamp <= p.expiry) revert NotDisbandable();

        uint128 escrow = p.escrowWei;

        // CEI: terminal state + active-count BEFORE the refunds.
        p.status = LibPartyStorage.Status.Disbanded;
        s.activeOf[p.creator] -= 1;

        if (escrow > 0) {
            address token = LibCreditsStorage.load().creditsToken;
            if (token == address(0)) revert NotConfigured();
            address[] storage fs = s.funders[partyId];
            for (uint256 i = 0; i < fs.length; i++) {
                uint128 amt = s.fundedBy[partyId][fs[i]];
                if (amt > 0) {
                    require(IERC20Min(token).transfer(fs[i], amt), "party: refund failed");
                }
            }
        }

        emit PartyDisbanded(partyId, escrow);
    }

    // --- Views (the squad-board surface; all `party`-prefixed) -----------

    /// Full party record by id. Returns zeros for an unknown id
    /// (creator == address(0)).
    function getParty(uint256 partyId)
        external
        view
        returns (
            address creator,
            uint64 expiry,
            uint8 status,
            uint128 escrowWei,
            uint256 memberCount,
            uint256 acceptedCount
        )
    {
        LibPartyStorage.Storage storage s = LibPartyStorage.load();
        LibPartyStorage.Party storage p = s.parties[partyId];
        return (
            p.creator,
            p.expiry,
            uint8(p.status),
            p.escrowWei,
            s.members[partyId].length,
            p.acceptedCount
        );
    }

    /// The member identity tokenIds (parallel to `partySharesOf`).
    function partyMembersOf(uint256 partyId) external view returns (uint256[] memory) {
        return LibPartyStorage.load().members[partyId];
    }

    /// Each member's share in basis points (parallel to `partyMembersOf`;
    /// sums to 10000).
    function partySharesOf(uint256 partyId) external view returns (uint16[] memory) {
        return LibPartyStorage.load().shares[partyId];
    }

    /// Whether a member seat (tokenId) has consented to the party's split.
    function partyConsentOf(uint256 partyId, uint256 tokenId) external view returns (bool) {
        return LibPartyStorage.load().consented[partyId][tokenId];
    }

    /// The distinct funder addresses (the refund roster).
    function partyFundersOf(uint256 partyId) external view returns (address[] memory) {
        return LibPartyStorage.load().funders[partyId];
    }

    /// A funder's exact cumulative contribution (wei) — what disband
    /// refunds them.
    function partyContributionOf(uint256 partyId, address funder)
        external
        view
        returns (uint256)
    {
        return LibPartyStorage.load().fundedBy[partyId][funder];
    }

    /// Every party id a given creator has formed (live + terminal).
    function partiesOf(address creator) external view returns (uint256[] memory) {
        return LibPartyStorage.load().partiesOfCreator[creator];
    }

    /// Total parties ever formed (== highest party id; ids are monotonic).
    function partyCount() external view returns (uint256) {
        return LibPartyStorage.load().nextPartyId;
    }

    /// The count of a creator's currently-LIVE (Forming/Active) parties —
    /// the anti-sybil cap key.
    function activePartyCountOf(address creator) external view returns (uint256) {
        return LibPartyStorage.load().activeOf[creator];
    }

    /// Paginated scan of LIVE (Forming/Active), unexpired parties: returns
    /// up to `limit` ids scanning `partyIds` from index `startAfter` (a
    /// 0-based INDEX into `partyIds`, NOT a party id), plus the cursor to
    /// continue from. Index-window paging, identical to BountyFacet's
    /// `openBounties` — the cursor is index-based + partyIds is
    /// append-only, so pagination is stable across blocks.
    function liveParties(uint256 startAfter, uint256 limit)
        external
        view
        returns (uint256[] memory ids, uint256 nextCursor)
    {
        LibPartyStorage.Storage storage s = LibPartyStorage.load();
        uint256 total = s.partyIds.length;
        if (startAfter >= total || limit == 0) {
            return (new uint256[](0), total);
        }
        uint256 nowTs = block.timestamp;
        // Pass 1: count matches in the window so the array sizes exactly.
        uint256 scanned = 0;
        uint256 matches = 0;
        uint256 i = startAfter;
        while (i < total && scanned < limit) {
            if (_isLiveUnexpired(s, s.partyIds[i], nowTs)) matches++;
            i++;
            scanned++;
        }
        nextCursor = i;
        ids = new uint256[](matches);
        uint256 k = 0;
        for (uint256 m = startAfter; m < i; m++) {
            uint256 pid = s.partyIds[m];
            if (_isLiveUnexpired(s, pid, nowTs)) ids[k++] = pid;
        }
    }

    function _isLiveUnexpired(LibPartyStorage.Storage storage s, uint256 partyId, uint256 nowTs)
        internal
        view
        returns (bool)
    {
        LibPartyStorage.Party storage p = s.parties[partyId];
        return (
            p.status == LibPartyStorage.Status.Forming
                || p.status == LibPartyStorage.Status.Active
        ) && p.expiry >= nowTs;
    }
}
