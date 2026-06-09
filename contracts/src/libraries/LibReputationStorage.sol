// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the ReputationFacet — the agent-economy TRUST
///      rung (ERC-8004-flavored on-chain attestations). Diamond storage
///      pattern: a fresh slot, no collision with the registry / TBA / invite
///      / schedule / credits / bounty storage already cut into the diamond.
///      Add new fields ONLY at the end of the struct, and ONLY at the end of
///      `Attestation` (diamond storage layout is positional and immutable —
///      the "append-only rule").
///
///      NON-FINANCIAL, so this is lower-risk than the money facets (bounty /
///      invite / schedule): there is no `$LH` escrow, no payout, no refund,
///      hence no value to drain. The discipline is the SAME, though — custom
///      errors, careful (append-only) storage layout, and Checks-Effects so a
///      malformed call can never leave a half-written aggregate.
///
///      DATA MODEL. An attestation is a single signal from one attester
///      address about one SUBJECT identity's work:
///        {attester, rating (1..5), workRef (a hash / off-chain pointer)}.
///      The append-only `Attestation[]` per subject IS the audit trail; the
///      per-subject `Aggregate {count, sumRating}` is the O(1) reputation
///      summary (avg = sumRating / count computed OFF-CHAIN — no on-chain
///      division). The DEDUP key is (attester, subject, workRef): one address
///      can attest a given subject MANY times for DISTINCT works, but never
///      twice for the SAME workRef — so a single attester can't inflate a
///      subject's reputation by re-submitting one signal.
///
///      ANTI-SYBIL MVP (in this facet): the (attester, subject, workRef)
///      dedup + the self-attestation rejection (an identity's owner can't
///      attest its own token) + the rating-range check (1..5). NOTED FOLLOW-
///      UPS, deliberately NOT built here: (1) attester-reputation WEIGHTING —
///      weight each signal by the attester's own accrued reputation / stake
///      so a fresh throwaway address counts less than a long-lived agent; and
///      (2) BOUNTY-PAYMENT COUPLING — require that `workRef` corresponds to a
///      BountyFacet bounty that was actually accepted + PAID to the subject's
///      TBA, so reputation can only be minted off real settled work (closes
///      the free-attestation sybil farm where N throwaway addresses each
///      attest a colluding subject once). Both are additive cuts later; the
///      seam is the `attest` validation gate, not this facet's storage shape.
library LibReputationStorage {
    bytes32 constant POSITION = keccak256("localharness.reputation.storage.v1");

    /// One attestation record (append-only). Scalars packed to minimise cold
    /// SSTOREs: `attester`(20B) + `rating`(1B) share slot 0; `workRef`(32B)
    /// takes slot 1. Append fields ONLY at the end.
    struct Attestation {
        address attester; // who attested (msg.sender at attest-time)
        uint8 rating; // 1..5 (0 is never stored — BadRating rejects it)
        bytes32 workRef; // hash / off-chain pointer to the attested work
    }

    /// The O(1) reputation summary for one subject identity. `avg` is
    /// `sumRating / count`, computed OFF-CHAIN to avoid on-chain division /
    /// rounding. Both fields are monotonically non-decreasing (attestations
    /// are never removed), so `count` doubles as the audit-trail length.
    struct Aggregate {
        uint256 count; // number of attestations received
        uint256 sumRating; // running sum of ratings (1..5 each)
    }

    struct Storage {
        /// subjectTokenId -> the append-only list of attestations it received.
        /// The audit trail; `attestationsOf` pages over it.
        mapping(uint256 => Attestation[]) attestations;
        /// subjectTokenId -> its reputation aggregate (count + sumRating).
        /// Bumped in lockstep with the array push, so `agg.count` always
        /// equals `attestations[subject].length` (the conservation invariant
        /// the tests assert).
        mapping(uint256 => Aggregate) aggregate;
        /// dedup key -> already-attested flag. Key =
        /// keccak256(attester, subjectTokenId, workRef). One attester can
        /// attest a subject for MANY distinct works, but never the SAME
        /// workRef twice (the anti-inflation guard).
        mapping(bytes32 => bool) attested;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
