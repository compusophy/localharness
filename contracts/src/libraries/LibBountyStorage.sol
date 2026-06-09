// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the bounty-board facet — the demand-side
///      marketplace (Rung 1 of design/agent-coordination.md). Diamond
///      storage pattern — fresh slot, no collision with the registry /
///      TBA / invite / schedule / credits storage already cut into the
///      diamond. Add new fields ONLY at the end of the struct, and ONLY
///      at the end of `Bounty` (diamond storage layout is positional and
///      immutable — the "append-only rule").
///
///      The bounty escrow IS the InviteFacet escrow state-machine with a
///      richer release condition: instead of "accept by knowing a code,"
///      release is "accept by the poster confirming the submitted result"
///      (design §Rung 1). The mechanics (`transferFrom` poster→diamond on
///      post, CEI status-flips before every payout/refund) are copied
///      verbatim; only the lifecycle states differ.
library LibBountyStorage {
    bytes32 constant POSITION = keccak256("localharness.bounty.storage.v1");

    // --- Bounds (anti-grief + anti-sybil circuit-breakers). The TTL
    //     bound is the one limit the contract MUST enforce — an unbounded
    //     expiry locks `$LH` forever and defeats the reclaim loop, a
    //     1-second expiry is a griefing trap. The per-poster active-bounty
    //     cap mirrors ScheduleFacet's MAX_ACTIVE_JOBS_PER_OWNER (anti-
    //     sybil: bounds the row count a single funded account can flood the
    //     board with — each bounty still escrows real `$LH`, but the row
    //     count itself is the griefing vector this bounds). 64 is generous
    //     for a real poster yet small enough to keep the `openBounties`
    //     scan + the "my bounties" UI cheap. The task/result byte caps keep
    //     a single store gas-bounded (CLAUDE.md ~7.6k gas/byte; callers are
    //     expected to store a hash/pointer, not full prose — these are the
    //     hard ceiling, not the intended size). -----------------------
    uint64 internal constant MIN_TTL = 1 hours;
    uint64 internal constant MAX_TTL = 90 days;
    uint256 internal constant MAX_ACTIVE_PER_POSTER = 64;
    /// Hard ceiling on the stored task / result byte length. A pointer or
    /// hash is the intended payload (gas-per-byte); this just stops an
    /// unbounded blob from being escrowed into a single SSTORE chain.
    uint256 internal constant MAX_TASK_BYTES = 4096;
    uint256 internal constant MAX_RESULT_BYTES = 4096;

    /// Bounty lifecycle (the ABI-pinned enum — Open=0 … Reclaimed=5).
    /// Open → Claimed → Submitted → Paid (the happy path), OR
    /// Open → Cancelled (poster aborts before any claim), OR
    /// {Open,Claimed,Submitted} → Reclaimed (anyone pokes after expiry,
    /// refunds the poster). Paid / Cancelled / Reclaimed are terminal.
    enum Status {
        Open, // 0 — escrowed; claimable while now <= expiry, reclaimable after
        Claimed, // 1 — a worker reserved it (claimantTokenId set); awaiting a result
        Submitted, // 2 — a result was committed; awaiting poster acceptance
        Paid, // 3 — poster accepted; reward settled to the worker's TBA; terminal
        Cancelled, // 4 — poster aborted while still Open; refunded; terminal
        Reclaimed // 5 — expired + unaccepted; poster refunded; terminal
    }

    /// One bounty record, keyed by a monotonic `uint256 id`. Scalars
    /// packed to minimise cold SSTOREs: `poster`(20B) + `expiry`(8B) +
    /// `status`(1B) = 29 bytes share slot 0; `rewardWei`(16B) lands in
    /// slot 1; `claimantTokenId`(32B) takes slot 2. The task/result
    /// `bytes` live in their OWN mappings (bytes don't pack and on-chain
    /// bytes storage is the gas-hungry path).
    struct Bounty {
        address poster; // who escrowed the reward; the refund recipient
        uint64 expiry; // unix seconds; the claim/reclaim window boundary
        Status status; // Open | Claimed | Submitted | Paid | Cancelled | Reclaimed
        uint128 rewardWei; // $LH escrowed (18-dec wei); $LH supply << 2^128
        uint256 claimantTokenId; // the agent identity that claimed; payout → its TBA. 0 until claimed.
    }

    struct Storage {
        /// bountyId -> bounty record. Monotonic id from `nextBountyId`.
        /// A non-zero `poster` means the id is live (the unknown-bounty
        /// guard). ids start at 1; 0 = no bounty.
        mapping(uint256 => Bounty) bounties;
        /// bountyId -> the task spec bytes (hash / pointer / short prose).
        /// Stored separately because bytes don't pack into the scalar
        /// slots and on-chain bytes storage is gas-hungry.
        mapping(uint256 => bytes) task;
        /// bountyId -> the submitted result bytes (hash / pointer). Empty
        /// until `submitResult`.
        mapping(uint256 => bytes) result;
        /// Monotonic bounty id counter (ids start at 1; 0 = no bounty).
        uint256 nextBountyId;
        /// Flat enumerable index of EVERY bounty id ever posted — the
        /// diamond has no cheap "iterate the mapping", so `openBounties`
        /// pages over this with (startAfter, limit), filtering Open + not-
        /// expired on read (index-window paging, exactly like
        /// ScheduleFacet's `jobsDue`). Append-only (bounties are never
        /// removed, just status-flipped to terminal), so an id's position
        /// is stable and pagination cursors stay valid across calls.
        uint256[] bountyIds;
        /// poster -> the bounty ids they posted (for the "my bounties" UI).
        mapping(address => uint256[]) bountiesOfPoster;
        /// poster -> count of their CURRENTLY-LIVE bounties (the anti-sybil
        /// cap key). Incremented on `postBounty`, decremented when a bounty
        /// leaves the live set (Paid / Cancelled / Reclaimed). "Live" means
        /// any non-terminal status (Open / Claimed / Submitted) — the
        /// poster's escrow is still locked in all three, so all three count
        /// toward the cap.
        mapping(address => uint256) activeOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
