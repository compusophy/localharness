// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the user-funded invite facet
///      (design/invites.md §1.2). Diamond storage pattern — fresh slot,
///      no collision with the registry / TBA / feedback / credits /
///      session / redeem / schedule storage already cut. Add new fields
///      ONLY at the end of the struct, and ONLY at the end of `Invite`
///      (diamond storage layout is positional and immutable — §1.2
///      "append-only rule").
library LibInviteStorage {
    bytes32 constant POSITION = keccak256("localharness.invite.storage.v1");

    // --- Bounds (§2.2 / §2.4). The TTL bound is the one limit the
    //     contract MUST enforce; an unbounded expiry locks `$LH` forever
    //     and defeats the refund loop, a 1-second expiry is a griefing
    //     trap. The per-funder escrow cap is a testnet circuit-breaker
    //     (§2.4) — a sane hard ceiling on how much a single funder can
    //     lock in OPEN invites at once. Documented value, not the design's
    //     "unlimited default": the design leaves the cap as a seam, this
    //     MVP picks a generous-but-finite testnet value so a runaway /
    //     buggy client can't lock a funder's whole balance. 1_000_000 $LH
    //     (18-dec wei) — far above the largest UI tier (1000) so it never
    //     bites normal use, low enough to bound a single funder's footprint.
    uint64 internal constant MIN_TTL = 1 hours;
    uint64 internal constant MAX_TTL = 90 days;
    uint256 internal constant MAX_ESCROWED = 1_000_000 ether;

    /// Invite lifecycle. Open (escrowed, acceptable until expiry) →
    /// Accepted (paid out to the accepter, terminal) OR Reclaimed
    /// (refunded to the funder after expiry, terminal). Open's accept
    /// window (`now <= expiry`) and reclaim window (`now > expiry`) are
    /// DISJOINT, so an invite is accepted XOR reclaimed, never both
    /// (§3.1). The trichotomy is why this is a uint8 enum, not a bool —
    /// on-chain reads must distinguish "paid accepter" from "refunded
    /// funder" (§1.2).
    enum Status {
        Open, // 0 — escrowed; acceptable while now <= expiry, reclaimable after
        Accepted, // 1 — paid out to the accepter; terminal
        Reclaimed // 2 — refunded to the funder after expiry; terminal
    }

    /// One invite record, keyed by `keccak256(bytes(code))`. Scalars
    /// packed to minimise cold SSTOREs:
    ///   slot 0: funder(160) + expiry(64) + status(8)  = 232 bits
    ///   slot 1: amount(128)
    /// (Solidity packs in declaration order; field order below is chosen
    /// so funder+expiry+status share slot 0 and amount lands alone in slot
    /// 1 — two cold SSTOREs per create.)
    struct Invite {
        address funder; // who escrowed the $LH; the refund recipient
        uint64 expiry; // unix seconds; the accept/reclaim window boundary
        Status status; // Open | Accepted | Reclaimed
        uint128 amount; // $LH escrowed (18-dec wei); $LH supply << 2^128
    }

    struct Storage {
        /// codeHash (keccak256(bytes(code))) -> invite record. A non-zero
        /// `funder` means the code is taken (the dup-create guard). Only
        /// the HASH lives on-chain, so creating an invite never leaks the
        /// plaintext — the funder distributes it off-chain (the `?invite=`
        /// link).
        mapping(bytes32 => Invite) invites;
        /// funder -> total `$LH` currently locked in OPEN invites. A
        /// running sum maintained on create (+) / accept (−) / reclaim
        /// (−), so the UI can show "you have N $LH locked" and the facet
        /// can enforce MAX_ESCROWED without iterating (§2.4).
        mapping(address => uint256) escrowedOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
