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

    /// Child-tree metadata for a recursively-scheduled job
    /// (`scheduleChildJob`). Lives in its OWN mapping (NOT new `Job`
    /// fields) so the live `Job` storage layout is byte-for-byte
    /// unchanged — append-only discipline. A non-child job has no entry
    /// here (all zero: parentId 0, depth 0, rootId 0).
    struct ChildMeta {
        uint256 parentId; // the job this child was spawned from (0 for a root)
        uint64 depth; // 1 for a direct child; parent.depth + 1; bounded by MAX_DEPTH
        uint256 rootId; // the top-of-tree job id whose original budget caps the tree
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
        // === APPENDED 2026-06-08 (hardening: per-owner cap + recursion).
        //     New members ONLY at the end — the live diamond's storage
        //     for the members above is untouched (positional layout). ===
        /// owner -> count of their CURRENTLY-ACTIVE-or-PAUSED jobs (the
        /// anti-sybil cap key). `scheduleJob`/`scheduleChildJob` bump it
        /// and revert `TooManyActiveJobs` at `MAX_ACTIVE_JOBS_PER_OWNER`;
        /// `recordRun` (on exhaust) and `cancelJob` decrement it.
        /// CAVEAT (forward-looking): live jobs that predate this counter
        /// were never counted, so it starts at 0 on the live diamond and
        /// tracks only NEW jobs — an owner with old jobs can hold up to
        /// the cap MORE. Acceptable: the cap exists to bound future spam,
        /// not to retroactively reclassify the existing handful of jobs.
        mapping(address => uint256) activeJobsOf;
        /// jobId -> child-tree metadata (parent / depth / root). Only
        /// populated for jobs created by `scheduleChildJob`; absent (all
        /// zero) for normal root jobs. A NEW mapping, NOT new `Job`
        /// fields — keeps the live `Job` layout immutable.
        mapping(uint256 => ChildMeta) childMeta;
        // === APPENDED 2026-06-13 (#52: last-run timestamp + status). New
        //     member ONLY at the end — every prior member is byte-for-byte
        //     unchanged (positional layout), so live jobs are not touched. ===
        /// jobId -> the LAST `recordRun` outcome, packed: the unix-second
        /// timestamp in the high bits (`<< 8`) and the post-run `Status` in
        /// the low byte. 0 = the job has NEVER run (no `recordRun` yet) —
        /// distinguishable from a run at timestamp 0 because a real run's
        /// timestamp is always far above the status-byte range. `lastRunOf`
        /// unpacks it. Lets the `jobs`/`status` UIs show "last run: <when>
        /// [status]" without scraping the `JobRan` event log (Tempo RPC
        /// caps block ranges at 100k — same enumerable-state discipline as
        /// DeviceRegistry replacing log scraping).
        mapping(uint256 => uint72) lastRunRecord;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
