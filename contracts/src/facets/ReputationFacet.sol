// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibReputationStorage} from "../libraries/LibReputationStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

/// @title ReputationFacet
/// @notice The agent-economy TRUST rung — on-chain, attestation-based
///         reputation (ERC-8004-flavored). Agents ATTEST to each other's
///         completed work; a SUBJECT identity accrues a reputation aggregate
///         (count + ratingSum, avg computed off-chain). Composes with the
///         bounty board (the demand rung) and the colony: a worker who is
///         paid on a bounty can be attested for that `workRef`, and the
///         on-chain reputation lets posters / parties / guilds pick workers
///         they trust.
///
///         NON-FINANCIAL — there is NO `$LH` escrow, payout, or refund here,
///         so it carries none of the money facets' drain surface. It is still
///         held to the same discipline: custom errors, append-only storage,
///         and Checks-Effects ordering (validate first, then the single state
///         mutation, then the event — there is no external call at all, so
///         re-entrancy is structurally impossible).
///
///         WHAT AN ATTESTATION IS. One signal from `msg.sender` about one
///         subject identity's work: {attester, rating 1..5, workRef}. The
///         `workRef` is a hash / off-chain pointer (a bounty id hash, a git
///         commit, an IPFS CID) — its bytes are opaque to the facet; it only
///         serves as the per-work DEDUP discriminator.
///
///         ANTI-SYBIL MVP (this facet enforces all three):
///           1. DEDUP on (attester, subject, workRef) — one address can
///              attest a subject MANY times for DISTINCT works but NEVER
///              twice for the SAME workRef, so a single attester can't pump a
///              subject's count/sum by re-submitting one signal.
///           2. SELF-ATTESTATION rejection — if the subject token's owner IS
///              `msg.sender`, revert. An identity's controller can't attest
///              its own token (the most obvious self-pump).
///           3. RATING RANGE — 1..5 only; 0 and >5 revert.
///         NOTED FOLLOW-UPS (deliberately NOT built — additive cuts later;
///         the seam is THIS validation gate, not the storage shape):
///           • ATTESTER-REPUTATION WEIGHTING — weight each signal by the
///             attester's own accrued reputation / stake, so a fresh
///             throwaway address counts for less than an established agent.
///           • BOUNTY-PAYMENT COUPLING — require that `workRef` maps to a
///             BountyFacet bounty that was actually accepted + PAID to the
///             subject's TBA, so reputation can only be minted off real,
///             settled work. This is the strong sybil defense (it closes the
///             collusion farm where N throwaway addresses each attest a
///             confederate subject once for free); the MVP's dedup +
///             self-reject only bounds the cheapest abuse.
///
///         GAS / STORAGE: an attestation is a 2-slot push + a 2-field
///         aggregate bump + a 1-slot dedup flag — bounded, no unbounded blob
///         (the `workRef` is a fixed bytes32). `attestationsOf` is a paged
///         view (index-window paging, exactly like BountyFacet `openBounties`
///         / ScheduleFacet `jobsDue`), so an unbounded trail never has to be
///         returned in one call.
///
///         CUTTING IT (diamond owner; mirror script/AddBountyFacet): deploy +
///         diamondCut Add the 4 selectors in script/AddReputationFacet.s.sol.
///         No post-cut config — it only READS `ownerOfId` from the shared
///         registry storage slot (set by the registry on `register`).
contract ReputationFacet {
    // --- Events ---------------------------------------------------------

    /// Emitted on every successful attestation. Indexed for off-chain
    /// reputation indexers / the discovery board.
    event Attested(
        uint256 indexed subjectTokenId,
        address indexed attester,
        uint8 rating,
        bytes32 indexed workRef
    );

    // --- Errors ---------------------------------------------------------

    error BadRating(); // rating == 0 || rating > 5 (valid range is 1..5)
    error UnknownSubject(); // subjectTokenId is not a registered identity
    error SelfAttestation(); // the subject's owner is msg.sender
    error AlreadyAttested(); // (msg.sender, subject, workRef) already attested

    // --- Attest (permissionless; one signal per attester/subject/work) --

    /// Attest to `subjectTokenId`'s work. The CALLER (`msg.sender`) is the
    /// attester. `rating` is 1..5; `workRef` is an opaque hash / off-chain
    /// pointer to the attested work (a bounty-id hash, a commit, a CID — its
    /// bytes are never interpreted, only used as the per-work dedup key).
    ///
    /// On success: append the {attester, rating, workRef} record to the
    /// subject's audit trail, bump the subject's aggregate (count++,
    /// sumRating += rating), set the dedup flag, and emit `Attested`.
    ///
    /// Reverts:
    ///   • BadRating       — rating 0 or > 5.
    ///   • UnknownSubject  — subjectTokenId has no registered owner.
    ///   • SelfAttestation — the subject token's owner IS msg.sender.
    ///   • AlreadyAttested — this exact (attester, subject, workRef) already
    ///                       attested (the per-work dedup).
    ///
    /// CHECKS-EFFECTS: every revert condition is checked BEFORE any state
    /// write, then the dedup flag + array push + aggregate bump all land
    /// together. There is NO external call, so re-entrancy is impossible and
    /// a malformed call can never leave a half-written aggregate (the push
    /// and the count/sum bump are a single contiguous block).
    function attest(uint256 subjectTokenId, uint8 rating, bytes32 workRef) external {
        // --- Checks ---
        if (rating == 0 || rating > 5) revert BadRating();

        // The subject must be a registered identity (a minted tokenId has a
        // non-zero owner). Same existence test the bounty / schedule facets
        // use; without it, reputation could accrue to a phantom id.
        address subjectOwner = LibRegistryStorage.load().ownerOfId[subjectTokenId];
        if (subjectOwner == address(0)) revert UnknownSubject();

        // Can't attest your own identity (the most obvious self-pump).
        if (subjectOwner == msg.sender) revert SelfAttestation();

        LibReputationStorage.Storage storage s = LibReputationStorage.load();

        // One attestation per (attester, subject, workRef): an attester may
        // attest a subject for MANY distinct works, but never the SAME
        // workRef twice (anti-inflation).
        bytes32 dedupKey = keccak256(abi.encodePacked(msg.sender, subjectTokenId, workRef));
        if (s.attested[dedupKey]) revert AlreadyAttested();

        // --- Effects (single contiguous mutation; no external call) ---
        s.attested[dedupKey] = true;
        s.attestations[subjectTokenId].push(
            LibReputationStorage.Attestation({attester: msg.sender, rating: rating, workRef: workRef})
        );
        LibReputationStorage.Aggregate storage agg = s.aggregate[subjectTokenId];
        agg.count += 1;
        agg.sumRating += rating;

        emit Attested(subjectTokenId, msg.sender, rating, workRef);
    }

    // --- Views ----------------------------------------------------------

    /// The reputation aggregate for an identity: `(attestationCount,
    /// ratingSum)`. The average is `ratingSum / attestationCount`, computed
    /// OFF-CHAIN (no on-chain division). Returns (0, 0) for an identity that
    /// has never been attested (or an unknown id) — both indistinguishable
    /// and correct (zero reputation).
    function reputationOf(uint256 tokenId)
        external
        view
        returns (uint256 attestationCount, uint256 ratingSum)
    {
        LibReputationStorage.Aggregate storage agg = LibReputationStorage.load().aggregate[tokenId];
        return (agg.count, agg.sumRating);
    }

    /// Paginated scan of an identity's attestation trail. Returns up to
    /// `limit` records starting at index `start` (a 0-based INDEX into the
    /// append-only trail), as parallel arrays `(attesters, ratings,
    /// workRefs)` plus a `nextCursor` (the index scanned up to). Callers page
    /// with `nextCursor` until it stops advancing / the arrays come back
    /// short. Index-window paging, identical to BountyFacet `openBounties` /
    /// ScheduleFacet `jobsDue`: the trail is append-only so an index is stable
    /// across blocks and cursors never invalidate.
    ///
    /// An out-of-range `start` (>= length) or a zero `limit` returns empty
    /// arrays with `nextCursor == length` (a clean "you're at the end").
    function attestationsOf(uint256 tokenId, uint256 start, uint256 limit)
        external
        view
        returns (
            address[] memory attesters,
            uint8[] memory ratings,
            bytes32[] memory workRefs,
            uint256 nextCursor
        )
    {
        LibReputationStorage.Attestation[] storage trail =
            LibReputationStorage.load().attestations[tokenId];
        uint256 total = trail.length;
        if (start >= total || limit == 0) {
            return (new address[](0), new uint8[](0), new bytes32[](0), total);
        }

        uint256 end = start + limit;
        if (end > total) end = total;
        uint256 n = end - start;

        attesters = new address[](n);
        ratings = new uint8[](n);
        workRefs = new bytes32[](n);
        for (uint256 i = 0; i < n; i++) {
            LibReputationStorage.Attestation storage a = trail[start + i];
            attesters[i] = a.attester;
            ratings[i] = a.rating;
            workRefs[i] = a.workRef;
        }
        nextCursor = end;
    }

    /// Whether `attester` has already attested `subjectTokenId` for the given
    /// `workRef` — the dedup predicate. A caller checks this before `attest`
    /// to avoid an `AlreadyAttested` revert; the facet enforces it regardless.
    function hasAttested(address attester, uint256 subjectTokenId, bytes32 workRef)
        external
        view
        returns (bool)
    {
        bytes32 dedupKey = keccak256(abi.encodePacked(attester, subjectTokenId, workRef));
        return LibReputationStorage.load().attested[dedupKey];
    }
}
