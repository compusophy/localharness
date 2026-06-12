// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the PARTY facet — Rung 2 of the coordination
///      ladder (design/shipped/agent-coordination.md): an EPHEMERAL squad of
///      agents formed around ONE objective (often a bounty), with a
///      pre-agreed reward split, that dissolves after settlement. Diamond
///      storage pattern — fresh slot, no collision with the registry / TBA /
///      bounty / guild / invite / schedule / credits storage already cut
///      into the diamond. Add new fields ONLY at the end of the struct (and
///      only at the end of `Party`) — diamond storage layout is positional
///      and immutable (the "append-only rule").
///
///      MEMBERSHIP KEYS ON TOKEN IDS, NOT ADDRESSES (unlike GuildFacet).
///      A party's whole point is the SPLIT: each member is an agent IDENTITY
///      (a registry tokenId) whose share of the pot settles to ITS TBA on
///      completion — the BountyFacet payout precedent (the reward is bound
///      to the claimed identity's TBA). Consent therefore comes from the
///      identity's OWNER: `joinParty` consents every seat whose tokenId the
///      caller's address owns.
///
///      THE POT is a FACET-BALANCE ESCROW — `escrowWei`, `$LH` physically
///      held IN THE DIAMOND, the SAME safe pattern as BountyFacet/GuildFacet
///      (NOT a TBA-execute). `fundParty` credits it (`transferFrom`
///      funder→diamond, CEI); `completeParty` splits it to member TBAs by
///      `sharesBps` (remainder to the LAST member, so the split conserves
///      the escrow EXACTLY); `disbandParty` / TTL expiry refunds each FUNDER
///      their exact contribution. Escrow-conservation invariant: at every
///      point the diamond's party-held `$LH` equals the sum of live
///      (Forming/Active) parties' `escrowWei` — nothing stranded, nothing
///      minted (proven by the conservation fuzz).
library LibPartyStorage {
    bytes32 constant POSITION = keccak256("localharness.party.storage.v1");

    // --- Bounds (anti-grief / anti-sybil circuit-breakers). The TTL bound
    //     is the one limit the contract MUST enforce — an unbounded expiry
    //     locks `$LH` forever and defeats the refund loop; a 1-second expiry
    //     is a griefing trap (BountyFacet's exact reasoning). The member cap
    //     bounds the completeParty payout loop (one token transfer per
    //     member — a squad, not a guild; GuildFacet's 128 is the org-scale
    //     cap). The funder cap bounds the disband refund loop the same way.
    //     The per-creator active cap mirrors MAX_ACTIVE_PER_POSTER (bounds
    //     the row count a single funded account can flood). ---------------
    uint64 internal constant MIN_TTL = 1 hours;
    uint64 internal constant MAX_TTL = 90 days;
    uint256 internal constant MAX_PARTY_MEMBERS = 16;
    uint256 internal constant MAX_FUNDERS = 64;
    uint256 internal constant MAX_ACTIVE_PER_CREATOR = 32;
    /// Shares are basis points and MUST sum to exactly this (100%).
    uint256 internal constant TOTAL_SHARES_BPS = 10_000;

    /// Party lifecycle (the ABI-pinned enum — Forming=0 … Disbanded=3).
    /// Forming → Active (every member consented) → Completed (creator
    /// settles; pot splits to member TBAs), OR {Forming, Active} →
    /// Disbanded (creator any time, anyone after expiry; funders refunded
    /// 100%). Completed / Disbanded are terminal. The complete window
    /// (now <= expiry) and the permissionless-disband window (now > expiry)
    /// are DISJOINT, the InviteFacet accept/reclaim discipline.
    enum Status {
        Forming, // 0 — proposed; awaiting member consent (joinParty)
        Active, // 1 — fully consented; fundable + completable
        Completed, // 2 — pot split to member TBAs by shares; terminal
        Disbanded // 3 — dissolved; funders refunded; terminal
    }

    /// One party record, keyed by a monotonic `uint256 id`. Scalars packed
    /// to minimise cold SSTOREs: `creator`(20B) + `expiry`(8B) + `status`
    /// (1B) + `acceptedCount`(2B) = 31 bytes share slot 0; `escrowWei`(16B)
    /// lands in slot 1. The member/share/funder lists live in their own
    /// mappings (dynamic arrays don't pack).
    struct Party {
        address creator; // who proposed it; the complete/disband authority
        uint64 expiry; // unix seconds; the consent/fund/complete window end
        Status status; // Forming | Active | Completed | Disbanded
        uint16 acceptedCount; // members consented so far (== length → Active)
        uint128 escrowWei; // $LH pooled in the diamond ($LH supply << 2^128)
    }

    struct Storage {
        /// partyId -> party record. A non-zero `creator` means the id is
        /// live (the unknown-party guard). ids start at 1; 0 = no party.
        mapping(uint256 => Party) parties;
        /// partyId -> the member identity tokenIds (parallel to `shares`).
        /// Payouts settle to each tokenId's TBA on completion.
        mapping(uint256 => uint256[]) members;
        /// partyId -> each member's share in basis points (parallel to
        /// `members`; sums to TOTAL_SHARES_BPS — enforced at formParty).
        mapping(uint256 => uint16[]) shares;
        /// partyId -> member tokenId -> has the identity's owner consented.
        /// The consent gate (GuildFacet's invite/accept precedent): no one
        /// is conscripted into a split — every seat's owner must joinParty
        /// (creator-owned seats auto-consent at formParty).
        mapping(uint256 => mapping(uint256 => bool)) consented;
        /// partyId -> the distinct funder addresses (the disband refund
        /// loop's enumerable index; bounded by MAX_FUNDERS).
        mapping(uint256 => address[]) funders;
        /// partyId -> funder -> their exact cumulative contribution (wei).
        /// Disband refunds THIS, per funder — conservation by construction.
        mapping(uint256 => mapping(address => uint128)) fundedBy;
        /// Monotonic party id counter (ids start at 1; 0 = no party).
        uint256 nextPartyId;
        /// Flat enumerable index of EVERY party ever formed — `liveParties`
        /// pages over this with (startAfter, limit), filtering live +
        /// unexpired on read (index-window paging, exactly like
        /// BountyFacet's `openBounties`). Append-only, so cursors stay
        /// valid across calls.
        uint256[] partyIds;
        /// creator -> the party ids they formed (the "my parties" index).
        mapping(address => uint256[]) partiesOfCreator;
        /// creator -> count of their CURRENTLY-LIVE (Forming/Active)
        /// parties — the anti-sybil cap key. Incremented on formParty,
        /// decremented when a party turns terminal.
        mapping(address => uint256) activeOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
