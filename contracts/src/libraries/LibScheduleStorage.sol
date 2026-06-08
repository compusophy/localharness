// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the agent-scheduling facet — the durable,
///      tab-independent job registry (design/agent-scheduling.md §3).
///      Diamond storage pattern — fresh slot, no collision with any
///      other facet already cut. Add new fields ONLY at the end of the
///      struct, and ONLY at the end of `Job` (diamond storage layout is
///      positional and immutable — §3.2 "append-only rule").
library LibScheduleStorage {
    bytes32 constant POSITION = keccak256("localharness.schedule.storage.v1");

    /// Job lifecycle. Active → Paused (owner) → Active; or → Cancelled
    /// (owner, refunds remainder) / Exhausted (worker, budget or runs
    /// ran out, refunds remainder). Cancelled + Exhausted are terminal.
    enum Status {
        Active, // 0 — due to fire when nextRun <= now
        Paused, // 1 — owner-suspended; not fired, no refund
        Cancelled, // 2 — owner-cancelled; remainder refunded; terminal
        Exhausted // 3 — budget/runs spent; remainder refunded; terminal
    }

    /// One scheduled job. Scalars packed to minimise cold SSTOREs:
    ///   slot 0: owner(160) + interval(64) + status(8)        = 232 bits
    ///   slot 1: nextRun(64) + budgetWei(128) + runsLeft(32)  = 224 bits
    ///   slot 2: targetId(256)
    /// The `task` prompt lives in its own mapping (strings/bytes don't
    /// pack and on-chain string storage is the gas-hungry path —
    /// CLAUDE.md ~7.6k gas/byte; §3.2).
    struct Job {
        address owner; // who scheduled it; refund recipient; billing identity
        uint64 interval; // seconds between runs (the cadence)
        Status status; // Active | Paused | Cancelled | Exhausted
        uint64 nextRun; // unix seconds of the next due fire (the CAS key)
        uint128 budgetWei; // $LH escrowed for this job; debited per run; refundable
        uint32 runsLeft; // remaining runs (hard count cap); hitting 0 → Exhausted
        uint256 targetId; // tokenId of the agent to run (name resolved off-chain)
    }

    struct Storage {
        /// jobId -> job record. Monotonic id from `nextJobId`.
        mapping(uint256 => Job) jobs;
        /// jobId -> the task prompt (or an off-chain pointer). Stored
        /// separately because bytes don't pack into the scalar slots
        /// and on-chain string storage is gas-hungry (§3.4).
        mapping(uint256 => bytes) task;
        /// Monotonic job id counter (ids start at 1; 0 = no job).
        uint256 nextJobId;
        /// Flat enumerable index of EVERY job id ever scheduled — the
        /// diamond has no cheap "iterate the mapping", so `jobsDue`
        /// pages over this with (startAfter, limit), filtering Active +
        /// due on read (§3.3). Same enumerable-index discipline as
        /// DeviceRegistry / Team. Append-only (jobs are never removed,
        /// just status-flipped to terminal), so an id's position is
        /// stable and pagination cursors stay valid.
        uint256[] jobIds;
        /// owner -> the job ids they scheduled (for the "my jobs" UI).
        mapping(address => uint256[]) jobsOfOwner;
        /// The single address allowed to call `recordRun` — the worker
        /// (the credit proxy's scheduler key). Owner-set. A DEDICATED
        /// scheduler role, separable from the meter key (§7.3 Q3
        /// recommendation): firing authority distinct from metering.
        address scheduler;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
