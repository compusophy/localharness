// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibDiamond} from "../libraries/LibDiamond.sol";
import {LibScheduleStorage} from "../libraries/LibScheduleStorage.sol";
import {LibCreditsStorage} from "../libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../libraries/LibRegistryStorage.sol";

interface IERC20Min {
    function transferFrom(address from, address to, uint256 amount) external returns (bool);
    function transfer(address to, uint256 amount) external returns (bool);
}

/// @title ScheduleFacet
/// @notice The durable, tab-independent job registry — the on-chain
///         FOUNDATION for agent scheduling (design/agent-scheduling.md).
///         A holder ESCROWS their own `$LH` to back a recurring job that
///         runs a `<name>.localharness.xyz` agent on a fixed interval.
///         The job + its escrowed budget live on-chain, so they SURVIVE
///         any browser tab or process dying — the answer to "persistent
///         without keeping the tab open". A single off-chain worker (the
///         credit proxy's Vercel cron, the `scheduler` role) reads
///         `jobsDue`, fires each due job through the existing headless
///         `call` path, and calls `recordRun` to debit the job's budget
///         and advance its clock atomically.
///
///         TRUST ENVELOPE (§2.6): the worker gains NO new authority — it
///         can only fire owner-defined jobs and spend their PRE-COMMITTED
///         budgets. It never holds the owner's key; `recordRun` only
///         advances a clock + decrements a budget the owner already
///         escrowed. Same blast radius as the proxy holding the meter key.
///
///         BUDGET = THE HARD STOP (§4.1): the per-job `budgetWei` is the
///         ultimate leash. A runaway recursive A↔B loop drains both jobs'
///         budgets and HALTS — money is the floor under every softer
///         guard (max-runs, interval). Recursion / ping-pong itself is a
///         worker-side concern (the run's tool surface) layered on TOP of
///         this foundation; this facet only supplies the durable job +
///         escrow + the budget hard-stop it leans on (Phase 2, §7.1).
///
///         BILLING SINK: escrow pulls `$LH` caller → diamond. The
///         per-run debit simply REDUCES `budgetWei` — the already-escrowed
///         `$LH` stays in the diamond and BECOMES platform treasury
///         (withdrawable via `LocalharnessRegistryFacet.withdrawTreasury`,
///         the same "diamond IS the treasury" convention `register` fees
///         use). Cancel / exhaust REFUNDS the *unspent* remainder to the
///         owner. (§5.1A: debit-in-`recordRun` is what makes "the runaway
///         loop drains its budget and stops" actually true.)
///
///         CUTTING IT (diamond owner; mirror script/AddCreditMeterFacet):
///         deploy + diamondCut Add the 11 selectors in
///         script/AddScheduleFacet.s.sol, then `setScheduler(proxyKey)`.
contract ScheduleFacet {
    using LibScheduleStorage for LibScheduleStorage.Storage;

    // --- Events (indexed for off-chain harvest; §3.3) -------------------

    event JobScheduled(
        uint256 indexed id,
        address indexed owner,
        uint256 indexed targetId,
        uint64 interval,
        uint128 budgetWei,
        uint64 nextRun
    );
    /// `status` is the post-run `LibScheduleStorage.Status` as a uint8 —
    /// JobRan is the durable audit trail of every scheduled execution.
    event JobRan(uint256 indexed id, uint32 runsLeft, uint128 spentWei, uint64 nextRun, uint8 status);
    event JobCancelled(uint256 indexed id, uint128 refundedWei);
    event JobExhausted(uint256 indexed id, uint128 refundedWei);
    /// The job's GOAL was declared complete by the run itself (the worker
    /// relayed the agent's `finish_goal`) — terminal, remainder refunded.
    /// Distinguishable from JobExhausted (budget/runs ran dry) and
    /// JobCancelled (the owner pulled the plug).
    event JobCompleted(uint256 indexed id, uint128 refundedWei);
    event JobPaused(uint256 indexed id);
    event JobResumed(uint256 indexed id, uint64 nextRun);
    event JobToppedUp(uint256 indexed id, uint128 addWei, uint128 newBudget);
    event SchedulerUpdated(address indexed scheduler);
    /// #4 A child job spawned from `parentId`'s escrow. `rootId` is the
    /// top-of-tree job whose ORIGINAL budget caps the whole recursion;
    /// `depth` = parent depth + 1. No `$LH` minted — `budgetWei` MOVED
    /// from the parent's remaining budget (internal accounting only).
    event ChildJobScheduled(
        uint256 indexed id,
        uint256 indexed parentId,
        uint256 indexed rootId,
        uint64 depth,
        uint128 budgetWei
    );

    // --- Errors ---------------------------------------------------------

    error NotConfigured();
    error NotScheduler();
    error NotJobOwner();
    error ZeroBudget();
    error ZeroInterval();
    error ZeroRuns();
    error UnregisteredTarget();
    error UnknownJob();
    error JobNotActive();
    error JobNotPaused();
    error NotDue();
    error StaleNextRun(); // CAS guard — another firer already advanced this run
    error SpendExceedsBudget();
    // --- Hardening (#3 per-owner cap, #4 recursion) ---------------------
    error TooManyActiveJobs(); // owner already at MAX_ACTIVE_JOBS_PER_OWNER
    error InsufficientParentBudget(); // child draw exceeds parent's remaining budget
    error MaxDepthExceeded(); // child depth would exceed MAX_DEPTH
    error ParentNotActive(); // can only spawn a child from an Active parent

    // --- Bounds (§4.1 / §7.3 Q7). Sanity guards baked into the facet;
    //     finer per-owner caps + recursion depth live in Phase 2. ------

    /// No sub-minute hammering — bounds the firing rate. MUST be >= the
    /// worker's cron tick (§7.3 Q1: 5-min MVP, so 60s is a safe floor).
    uint64 internal constant MIN_INTERVAL = 60;
    /// A job fires at most this many times ever, regardless of budget.
    uint32 internal constant MAX_RUNS = 1_000_000;

    /// #3 ANTI-SYBIL: max simultaneously-live (Active|Paused) jobs ONE
    /// owner may hold. Caps the per-owner row count the worker's
    /// `jobsDue` scan and the "my jobs" UI must walk, so a single funded
    /// account can't flood the registry (each job still escrows real
    /// `$LH`, but the row-count itself is the griefing vector this
    /// bounds). 32 is generous for a real user yet small enough to keep
    /// enumeration cheap. Counter is `activeJobsOf`; see its storage doc
    /// for the forward-looking (pre-existing-jobs start at 0) caveat.
    uint256 internal constant MAX_ACTIVE_JOBS_PER_OWNER = 32;

    /// #4 RECURSION: max depth of a child-job tree (root = depth 0, its
    /// direct child = depth 1, …). Bounds runaway self-scheduling; the
    /// root's original budget already caps total spend (children draw
    /// from the parent's escrow — no new `$LH`), this just bounds the
    /// TREE HEIGHT so the depth field + any off-chain tree walk stay
    /// bounded. A child at MAX_DEPTH can spawn no further children.
    uint64 internal constant MAX_DEPTH = 4;

    // --- Schedule (permissionless to create; owner escrows the budget) --

    /// Schedule a recurring job. ESCROWS `budgetWei` `$LH` from the
    /// caller into the diamond (`transferFrom`; approve the diamond
    /// first — the bundle batches approve + scheduleJob into one
    /// sponsored tx, exactly like `depositCredits` / `openSession`).
    /// Stores the job, sets `nextRun = now + interval`, returns the id.
    ///
    /// Rejects zero budget / zero interval / zero maxRuns / an
    /// unregistered target. The `task` is the prompt run each tick
    /// (inline bytes for the MVP; a metadata pointer is the Phase-2
    /// scale path — §3.4). CEI: ALL job state is written before the
    /// external `transferFrom`, so a failed pull reverts the whole tx
    /// and leaves no ghost job.
    function scheduleJob(
        uint256 targetId,
        bytes calldata task,
        uint64 interval,
        uint128 budgetWei,
        uint32 maxRuns
    ) external returns (uint256 id) {
        if (budgetWei == 0) revert ZeroBudget();
        if (interval < MIN_INTERVAL) revert ZeroInterval();
        if (maxRuns == 0) revert ZeroRuns();
        if (maxRuns > MAX_RUNS) maxRuns = MAX_RUNS;
        // Target must be a registered agent (its tokenId has an owner).
        if (LibRegistryStorage.load().ownerOfId[targetId] == address(0)) {
            revert UnregisteredTarget();
        }

        LibScheduleStorage.Storage storage s = LibScheduleStorage.load();
        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();

        // Commit the job record + per-owner cap bump (#3). `_writeJob`
        // reverts TooManyActiveJobs at the cap BEFORE the escrow.
        id = _writeJob(s, msg.sender, targetId, task, interval, budgetWei, maxRuns);

        // CEI: escrow LAST. State is fully committed above; a failed
        // pull reverts everything (and these writes with it).
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), budgetWei),
            "schedule: escrow failed"
        );

        emit JobScheduled(id, msg.sender, targetId, interval, budgetWei, s.jobs[id].nextRun);
    }

    /// Shared job-record writer for `scheduleJob` + `scheduleChildJob`.
    /// Enforces the #3 per-owner active-job cap (reverts
    /// `TooManyActiveJobs`), bumps `activeJobsOf[owner]`, allocates the
    /// next id, and writes the `Job` + task + both enumerable indexes.
    /// Does NOT move any `$LH` (callers handle escrow / parent-budget
    /// draw) and does NOT touch `childMeta` (the child caller writes that
    /// after). Factored out to keep both callers under the stack limit.
    function _writeJob(
        LibScheduleStorage.Storage storage s,
        address jobOwner,
        uint256 targetId,
        bytes calldata task,
        uint64 interval,
        uint128 budgetWei,
        uint32 maxRuns
    ) internal returns (uint256 id) {
        if (s.activeJobsOf[jobOwner] >= MAX_ACTIVE_JOBS_PER_OWNER) {
            revert TooManyActiveJobs();
        }
        s.activeJobsOf[jobOwner] += 1;

        id = ++s.nextJobId; // ids start at 1
        s.jobs[id] = LibScheduleStorage.Job({
            owner: jobOwner,
            interval: interval,
            status: LibScheduleStorage.Status.Active,
            nextRun: uint64(block.timestamp) + interval,
            budgetWei: budgetWei,
            runsLeft: maxRuns,
            targetId: targetId
        });
        s.task[id] = task;
        s.jobIds.push(id);
        s.jobsOfOwner[jobOwner].push(id);
    }

    // --- Run accounting (the WORKER's only write; scheduler-only) -------

    /// Record one fired run. SCHEDULER-ROLE-ONLY (the worker). The single
    /// commit point for a run: it atomically (1) CAS-guards against a
    /// double-fire, (2) debits `spentWei` from the job budget (the
    /// `$LH` is already in the diamond → it becomes treasury), and (3)
    /// advances the clock. Folding the debit and the clock-advance into
    /// one tx makes double-fire STRUCTURALLY impossible (§2.5 / §5.1A).
    ///
    /// CAS guard (`expectedNextRun`): the worker reads `nextRun`, runs
    /// the turn, then calls `recordRun` with that read value. Whoever
    /// commits first wins; a racing firer (e.g. the tab + the cron)
    /// reverts with `StaleNextRun` and does NOT double-bill.
    ///
    /// "Skip, don't pile up" (§2.5): the next fire is `now + interval`,
    /// NOT `oldNextRun + interval` — a job idle through an outage fires
    /// ONCE and reschedules forward, never burst-drains its budget.
    ///
    /// HARD STOP: if `runsLeft` hits 0 OR the remaining budget can't
    /// cover another run of the same size (`budgetWei < spentWei`), the
    /// job is marked `Exhausted` and the unspent remainder is refunded
    /// to the owner. The budget is the leash; this is where it bites.
    function recordRun(uint256 id, uint64 expectedNextRun, uint128 spentWei)
        external
        returns (uint64 newNextRun)
    {
        LibScheduleStorage.Storage storage s = LibScheduleStorage.load();
        if (msg.sender != s.scheduler) revert NotScheduler();

        LibScheduleStorage.Job storage j = s.jobs[id];
        if (j.owner == address(0)) revert UnknownJob();
        if (j.status != LibScheduleStorage.Status.Active) revert JobNotActive();
        if (block.timestamp < j.nextRun) revert NotDue();
        if (j.nextRun != expectedNextRun) revert StaleNextRun();
        if (spentWei > j.budgetWei) revert SpendExceedsBudget();

        // Debit the JOB budget (the spent $LH stays in the diamond =
        // treasury; no transfer needed — it was escrowed here at
        // scheduleJob). Decrement the run counter.
        uint128 remaining = j.budgetWei - spentWei;
        j.budgetWei = remaining;
        uint32 runsLeft = j.runsLeft - 1;
        j.runsLeft = runsLeft;

        // Decide: keep running, or hit a hard stop. Exhaust when no runs
        // are left OR the remaining budget can't cover another run of
        // this size (`remaining < spentWei`). A zero-cost run with runs
        // left keeps going (budget untouched).
        if (runsLeft == 0 || remaining < spentWei) {
            j.status = LibScheduleStorage.Status.Exhausted;
            newNextRun = 0;
            // #3: the job leaves the live (Active|Paused) set — drop the
            // owner's active count so freed slots return to the cap.
            // (Underflow-safe: scheduleJob/scheduleChildJob always
            // incremented on creation; status guards above guarantee each
            // job is exhausted at most once.)
            s.activeJobsOf[j.owner] -= 1;
            // Refund the unspent remainder to the owner (CEI: status is
            // already terminal, budget already zeroed below, before the
            // external transfer).
            j.budgetWei = 0;
            emit JobRan(id, runsLeft, spentWei, 0, uint8(LibScheduleStorage.Status.Exhausted));
            emit JobExhausted(id, remaining);
            if (remaining > 0) _refund(j.owner, remaining);
        } else {
            // Skip-don't-pile-up: schedule forward from NOW.
            newNextRun = uint64(block.timestamp) + j.interval;
            j.nextRun = newNextRun;
            emit JobRan(id, runsLeft, spentWei, newNextRun, uint8(LibScheduleStorage.Status.Active));
        }

        // #52: stamp the last-run record — timestamp in the high bits, the
        // post-run status in the low byte — AFTER the status above is final
        // (Exhausted on a hard stop, else Active). Read via `lastRunOf` so
        // UIs show "last run: <when> [status]" without scraping JobRan logs.
        s.lastRunRecord[id] = (uint72(block.timestamp) << 8) | uint8(j.status);
    }

    /// SCHEDULER-ROLE-ONLY goal completion — the on-chain half of the
    /// `/goal` ralph loop. When a scheduled run's agent calls its
    /// `finish_goal` tool (the goal is verifiably met), the worker relays
    /// it here: the job is marked terminal (`Exhausted` — same terminal
    /// semantics as a budget hard-stop) and the FULL remaining `budgetWei`
    /// is refunded to the job's owner. This is the only way a job ends
    /// EARLY without its owner acting: `cancelJob` is owner-only and
    /// `recordRun` only exhausts when budget/runs run dry.
    ///
    /// Same trust envelope as `recordRun` (§2.6): the scheduler gains no
    /// spend authority — completing a job can only REFUND escrow to its
    /// owner, never move it anywhere else. Accepts Active OR Paused (an
    /// owner pausing mid-run must not strand a met goal's refund);
    /// terminal states revert `JobNotActive`, so a double-complete (or a
    /// complete after cancel/exhaust) cannot double-refund. CEI: budget
    /// zeroed + status terminal before the external transfer.
    function completeJob(uint256 id) external {
        LibScheduleStorage.Storage storage s = LibScheduleStorage.load();
        if (msg.sender != s.scheduler) revert NotScheduler();

        LibScheduleStorage.Job storage j = s.jobs[id];
        if (j.owner == address(0)) revert UnknownJob();
        if (
            j.status != LibScheduleStorage.Status.Active
                && j.status != LibScheduleStorage.Status.Paused
        ) revert JobNotActive();

        uint128 refund = j.budgetWei;
        j.budgetWei = 0;
        j.status = LibScheduleStorage.Status.Exhausted;
        // #3: the job leaves the live (Active|Paused) set — return the
        // owner's cap slot. (Underflow-safe: incremented on creation; the
        // status guard above means a job completes at most once.)
        s.activeJobsOf[j.owner] -= 1;
        emit JobCompleted(id, refund);
        if (refund > 0) _refund(j.owner, refund);
    }

    // --- Recursion (#4 cross-tick child jobs, parent-budget-funded) -----

    /// Schedule a CHILD job funded entirely out of a PARENT job's
    /// already-escrowed budget. SCHEDULER-ROLE-ONLY: the worker calls
    /// this DURING a parent's run, on the scheduled agent's behalf, so a
    /// run can spawn follow-up work that fires on a LATER tick (cross-tick
    /// recursion / agent-to-agent ping-pong — design §7.1 Phase 2).
    ///
    /// PURE INTERNAL ACCOUNTING — no wallet pull, no `transferFrom`. The
    /// `$LH` is ALREADY escrowed in the diamond (the parent pulled it at
    /// `scheduleJob`); this MOVES `budgetWei` out of the parent's
    /// remaining `budgetWei` and into a fresh child job. The diamond's
    /// `$LH` balance is unchanged; the live-budget SUM is unchanged (the
    /// wei just moved rows). Reverts `InsufficientParentBudget` if the
    /// parent holds less than `budgetWei`.
    ///
    /// ROOT-BUDGET-CAPS-THE-TREE: because every child is carved out of
    /// its parent's escrow and NO new `$LH` is ever created here, the sum
    /// of all live budgets in a tree can never exceed the root job's
    /// original budget. That single number is the hard ceiling on the
    /// entire recursive fan-out — money is the leash, exactly as for a
    /// flat job (§4.1). `MAX_DEPTH` additionally bounds the tree HEIGHT.
    ///
    /// OWNER INHERITANCE: the child's `owner` is the PARENT's owner, so
    /// cancel/exhaust refunds flow to the right account and the child
    /// counts toward the PARENT owner's `activeJobsOf` cap. The parent
    /// must be Active (`ParentNotActive` otherwise).
    ///
    /// CEI / reentrancy-safe: there is NO external call in this function
    /// (no transfer — the escrow never moves), so it's reentrancy-inert
    /// by construction; all writes are committed before it returns.
    function scheduleChildJob(
        uint256 parentJobId,
        uint256 targetId,
        bytes calldata task,
        uint64 interval,
        uint128 budgetWei,
        uint32 maxRuns
    ) external returns (uint256 childJobId) {
        LibScheduleStorage.Storage storage s = LibScheduleStorage.load();
        if (msg.sender != s.scheduler) revert NotScheduler();

        // Same input validation as scheduleJob (the child is a real job).
        if (budgetWei == 0) revert ZeroBudget();
        if (interval < MIN_INTERVAL) revert ZeroInterval();
        if (maxRuns == 0) revert ZeroRuns();
        if (maxRuns > MAX_RUNS) maxRuns = MAX_RUNS;
        if (LibRegistryStorage.load().ownerOfId[targetId] == address(0)) {
            revert UnregisteredTarget();
        }

        LibScheduleStorage.Job storage parent = s.jobs[parentJobId];
        if (parent.owner == address(0)) revert UnknownJob();
        if (parent.status != LibScheduleStorage.Status.Active) revert ParentNotActive();

        // Depth: root is depth 0; a direct child is depth 1; bounded by
        // MAX_DEPTH so the tree height stays finite. rootId chains up to
        // the top: parent's rootId if the parent is itself a child, else
        // the parent IS the root.
        uint64 childDepth = s.childMeta[parentJobId].depth + 1;
        if (childDepth > MAX_DEPTH) revert MaxDepthExceeded();
        uint256 rootId =
            s.childMeta[parentJobId].rootId == 0 ? parentJobId : s.childMeta[parentJobId].rootId;

        // MOVE budget from the parent's escrow into the child. No mint,
        // no transfer — the $LH already sits in the diamond. This is the
        // ONLY way budget enters a child, so the root's original budget
        // inherently caps the whole tree. (Drawn BEFORE `_writeJob` so a
        // cap-revert in there also rolls back this debit.)
        if (parent.budgetWei < budgetWei) revert InsufficientParentBudget();
        parent.budgetWei -= budgetWei;

        // Child record + per-owner cap (#3), inheriting the parent's
        // owner (refund identity). `_writeJob` reverts TooManyActiveJobs
        // at the cap (and the parent-budget debit above reverts with it).
        childJobId = _writeJob(s, parent.owner, targetId, task, interval, budgetWei, maxRuns);
        s.childMeta[childJobId] =
            LibScheduleStorage.ChildMeta({parentId: parentJobId, depth: childDepth, rootId: rootId});

        emit JobScheduled(
            childJobId, parent.owner, targetId, interval, budgetWei, s.jobs[childJobId].nextRun
        );
        emit ChildJobScheduled(childJobId, parentJobId, rootId, childDepth, budgetWei);
    }

    /// Child-tree metadata for a job (parentId / depth / rootId). Returns
    /// all-zero for a root (non-child) job. Lets the worker + UIs read a
    /// job's place in a recursive tree without touching the live `Job`.
    function childMetaOf(uint256 id)
        external
        view
        returns (uint256 parentId, uint64 depth, uint256 rootId)
    {
        LibScheduleStorage.ChildMeta storage m = LibScheduleStorage.load().childMeta[id];
        return (m.parentId, m.depth, m.rootId);
    }

    /// The count of an owner's currently-live (Active|Paused) jobs — the
    /// `activeJobsOf` cap key (#3). Read it to know how many slots remain
    /// before `MAX_ACTIVE_JOBS_PER_OWNER`.
    function activeJobCountOf(address owner) external view returns (uint256) {
        return LibScheduleStorage.load().activeJobsOf[owner];
    }

    // --- Owner controls -------------------------------------------------

    /// Owner-only. Cancel an Active or Paused job and REFUND its full
    /// remaining `budgetWei` to the owner. Terminal. CEI: budget is
    /// zeroed + status set Cancelled before the external `transfer`.
    function cancelJob(uint256 id) external {
        LibScheduleStorage.Storage storage s = LibScheduleStorage.load();
        LibScheduleStorage.Job storage j = s.jobs[id];
        if (j.owner == address(0)) revert UnknownJob();
        if (msg.sender != j.owner) revert NotJobOwner();
        if (
            j.status != LibScheduleStorage.Status.Active
                && j.status != LibScheduleStorage.Status.Paused
        ) revert JobNotActive();

        uint128 refund = j.budgetWei;
        j.budgetWei = 0;
        j.status = LibScheduleStorage.Status.Cancelled;
        // #3: the job leaves the live (Active|Paused) set — drop the
        // owner's active count. (Underflow-safe: it was incremented on
        // creation and the Active|Paused status guard above means a job
        // is cancelled at most once.)
        s.activeJobsOf[j.owner] -= 1;
        emit JobCancelled(id, refund);
        if (refund > 0) _refund(j.owner, refund);
    }

    /// Owner-only. Suspend an Active job — it won't be fired (no refund;
    /// the budget stays escrowed). `resumeJob` reactivates it.
    function pauseJob(uint256 id) external {
        LibScheduleStorage.Job storage j = LibScheduleStorage.load().jobs[id];
        if (j.owner == address(0)) revert UnknownJob();
        if (msg.sender != j.owner) revert NotJobOwner();
        if (j.status != LibScheduleStorage.Status.Active) revert JobNotActive();
        j.status = LibScheduleStorage.Status.Paused;
        emit JobPaused(id);
    }

    /// Owner-only. Reactivate a Paused job. `nextRun` is moved forward
    /// to `now + interval` so a long pause doesn't make it instantly
    /// (and repeatedly) due — same skip-don't-pile-up discipline.
    function resumeJob(uint256 id) external {
        LibScheduleStorage.Job storage j = LibScheduleStorage.load().jobs[id];
        if (j.owner == address(0)) revert UnknownJob();
        if (msg.sender != j.owner) revert NotJobOwner();
        if (j.status != LibScheduleStorage.Status.Paused) revert JobNotPaused();
        j.status = LibScheduleStorage.Status.Active;
        uint64 next = uint64(block.timestamp) + j.interval;
        j.nextRun = next;
        emit JobResumed(id, next);
    }

    /// Owner-only. Escrow MORE `$LH` into an Active/Paused job's budget
    /// (approve the diamond first). CEI: budget is bumped before the
    /// pull — a failed `transferFrom` reverts the increment with it.
    function topUpJob(uint256 id, uint128 addWei) external {
        if (addWei == 0) revert ZeroBudget();
        LibScheduleStorage.Storage storage s = LibScheduleStorage.load();
        LibScheduleStorage.Job storage j = s.jobs[id];
        if (j.owner == address(0)) revert UnknownJob();
        if (msg.sender != j.owner) revert NotJobOwner();
        if (
            j.status != LibScheduleStorage.Status.Active
                && j.status != LibScheduleStorage.Status.Paused
        ) revert JobNotActive();

        uint128 newBudget = j.budgetWei + addWei;
        j.budgetWei = newBudget;

        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(
            IERC20Min(token).transferFrom(msg.sender, address(this), addWei),
            "schedule: topup failed"
        );
        emit JobToppedUp(id, addWei, newBudget);
    }

    // --- Owner (diamond) ------------------------------------------------

    /// Diamond-owner-only. Set the scheduler role — the worker key
    /// allowed to call `recordRun`. A DEDICATED role, kept separable
    /// from the meter key (§7.3 Q3).
    function setScheduler(address newScheduler) external {
        LibDiamond.enforceIsContractOwner();
        LibScheduleStorage.load().scheduler = newScheduler;
        emit SchedulerUpdated(newScheduler);
    }

    // --- Views (the worker + UIs read these) ----------------------------

    /// Paginated scan of due jobs: returns up to `limit` ids of Active
    /// jobs with `nextRun <= block.timestamp`, scanning `jobIds` after
    /// position `startAfter` (a 0-based index into `jobIds`, NOT a job
    /// id). The worker pages with the returned `nextCursor` (the index
    /// it scanned up to) until it comes back empty or short. A flat
    /// scan for the MVP; a `nextRun`-bucketed index is the scale path
    /// (§3.3). The cursor is index-based + jobIds is append-only, so
    /// pagination is stable across ticks.
    function jobsDue(uint256 startAfter, uint256 limit)
        external
        view
        returns (uint256[] memory ids, uint256 nextCursor)
    {
        LibScheduleStorage.Storage storage s = LibScheduleStorage.load();
        uint256 total = s.jobIds.length;
        if (startAfter >= total || limit == 0) {
            return (new uint256[](0), total);
        }
        uint256 nowTs = block.timestamp;
        // First pass: count matches in the [startAfter, total) window so
        // we size the result array exactly (view = free gas).
        uint256 scanned = 0;
        uint256 matches = 0;
        uint256 i = startAfter;
        while (i < total && scanned < limit) {
            uint256 jid = s.jobIds[i];
            LibScheduleStorage.Job storage j = s.jobs[jid];
            if (j.status == LibScheduleStorage.Status.Active && j.nextRun <= nowTs) {
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
            uint256 jid = s.jobIds[m];
            LibScheduleStorage.Job storage j = s.jobs[jid];
            if (j.status == LibScheduleStorage.Status.Active && j.nextRun <= nowTs) {
                ids[k++] = jid;
            }
            m++;
            scanned2++;
        }
    }

    /// Full job record by id.
    function getJob(uint256 id) external view returns (LibScheduleStorage.Job memory) {
        return LibScheduleStorage.load().jobs[id];
    }

    /// The task prompt (or pointer) for a job.
    function taskOf(uint256 id) external view returns (bytes memory) {
        return LibScheduleStorage.load().task[id];
    }

    /// #52: the LAST `recordRun` outcome for a job — `timestamp` is the
    /// unix second of the most recent fire, `status` the post-run
    /// `LibScheduleStorage.Status` byte (Active still running / Exhausted
    /// hard-stopped). Returns `(0, 0)` if the job has NEVER run yet (no
    /// `recordRun`). Unpacks the packed `lastRunRecord` word (timestamp in
    /// the high bits, status in the low byte).
    function lastRunOf(uint256 id) external view returns (uint64 timestamp, uint8 status) {
        uint72 packed = LibScheduleStorage.load().lastRunRecord[id];
        return (uint64(packed >> 8), uint8(packed));
    }

    /// Every job id a given owner has scheduled (Active + terminal).
    function jobsOf(address owner) external view returns (uint256[] memory) {
        return LibScheduleStorage.load().jobsOfOwner[owner];
    }

    /// Total jobs ever scheduled (== highest job id; ids are monotonic).
    function jobCount() external view returns (uint256) {
        return LibScheduleStorage.load().nextJobId;
    }

    /// The current scheduler (worker) role address.
    function schedulerAddress() external view returns (address) {
        return LibScheduleStorage.load().scheduler;
    }

    // --- internal -------------------------------------------------------

    /// Refund `$LH` from the diamond (the escrow holder) to `to`. A
    /// plain `transfer` against the credits token — the diamond IS the
    /// holder, so no allowance ceremony (same as `withdrawTreasury`).
    function _refund(address to, uint128 amount) internal {
        address token = LibCreditsStorage.load().creditsToken;
        if (token == address(0)) revert NotConfigured();
        require(IERC20Min(token).transfer(to, amount), "schedule: refund failed");
    }
}
