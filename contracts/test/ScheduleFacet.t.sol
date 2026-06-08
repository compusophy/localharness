// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {ScheduleFacet} from "../src/facets/ScheduleFacet.sol";
import {LibScheduleStorage} from "../src/libraries/LibScheduleStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";
import {LibDiamond} from "../src/libraries/LibDiamond.sol";

/// Minimal `$LH`-shaped TIP-20 mock: 18-decimal balances + the
/// approve/transferFrom/transfer surface ScheduleFacet escrows + refunds
/// through. Reverts (returns false) on an under-allowance / under-balance
/// pull so we can prove the facet's CEI ordering.
contract MockLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    function mint(address to, uint256 amt) external {
        balanceOf[to] += amt;
    }

    function approve(address spender, uint256 amt) external returns (bool) {
        allowance[msg.sender][spender] = amt;
        return true;
    }

    function transferFrom(address from, address to, uint256 amt) external returns (bool) {
        uint256 a = allowance[from][msg.sender];
        require(a >= amt, "allowance");
        require(balanceOf[from] >= amt, "balance");
        allowance[from][msg.sender] = a - amt;
        balanceOf[from] -= amt;
        balanceOf[to] += amt;
        return true;
    }

    function transfer(address to, uint256 amt) external returns (bool) {
        require(balanceOf[msg.sender] >= amt, "balance");
        balanceOf[msg.sender] -= amt;
        balanceOf[to] += amt;
        return true;
    }
}

/// Test harness: ScheduleFacet + tiny setters that write the SHARED
/// diamond-storage slots a real diamond would populate via other facets
/// (creditsToken from CreditsFacet, ownerOfId from the registry, the
/// diamond owner from DiamondInit). Because every `Lib*Storage.load()`
/// resolves against THIS contract's storage, writing them here is
/// exactly the cross-facet storage sharing the diamond provides — so the
/// facet under test reads them identically to production.
contract ScheduleHarness is ScheduleFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function _registerTarget(uint256 id, address owner) external {
        LibRegistryStorage.load().ownerOfId[id] = owner;
    }

    function _setDiamondOwner(address owner) external {
        LibDiamond.setContractOwner(owner);
    }
}

contract ScheduleFacetTest is Test {
    ScheduleHarness sched;
    MockLH lh;

    address owner = address(0xA11CE); // job owner / scheduler of jobs
    address worker = address(0x401234); // the scheduler role (worker)
    address diamondOwner = address(0xD1A); // diamond EIP-173 owner
    address stranger = address(0xBEEF);

    uint256 constant TARGET = 7; // a registered agent tokenId
    uint64 constant INTERVAL = 3600; // 1h
    uint128 constant BUDGET = 100 ether; // 100 $LH
    uint32 constant MAX_RUNS = 10;
    uint128 constant COST = 5 ether; // per-run cost

    function setUp() public {
        sched = new ScheduleHarness();
        lh = new MockLH();
        sched._setCreditsToken(address(lh));
        sched._setDiamondOwner(diamondOwner);
        sched._registerTarget(TARGET, address(0xCAFE)); // target is registered

        // Diamond owner authorizes the worker as the scheduler role.
        vm.prank(diamondOwner);
        sched.setScheduler(worker);

        // Fund the job owner and pre-approve the diamond (the facet) for escrow.
        lh.mint(owner, 1_000 ether);
        vm.prank(owner);
        lh.approve(address(sched), type(uint256).max);

        // Pin a stable timestamp so nextRun math is deterministic.
        vm.warp(1_000_000);
    }

    // --- scheduleJob: escrow + validation -------------------------------

    function _schedule() internal returns (uint256 id) {
        vm.prank(owner);
        id = sched.scheduleJob(TARGET, bytes("ping the deploy"), INTERVAL, BUDGET, MAX_RUNS);
    }

    function test_scheduleJob_escrows_and_stores() public {
        uint256 ownerBefore = lh.balanceOf(owner);
        uint256 id = _schedule();

        assertEq(id, 1, "first job id is 1");
        // $LH moved owner -> diamond (the facet).
        assertEq(lh.balanceOf(owner), ownerBefore - BUDGET, "budget escrowed from owner");
        assertEq(lh.balanceOf(address(sched)), BUDGET, "diamond holds the escrow");

        LibScheduleStorage.Job memory j = sched.getJob(id);
        assertEq(j.owner, owner);
        assertEq(j.targetId, TARGET);
        assertEq(j.interval, INTERVAL);
        assertEq(j.budgetWei, BUDGET);
        assertEq(j.runsLeft, MAX_RUNS);
        assertEq(uint8(j.status), uint8(LibScheduleStorage.Status.Active));
        assertEq(j.nextRun, uint64(block.timestamp) + INTERVAL, "nextRun = now + interval");
        assertEq(string(sched.taskOf(id)), "ping the deploy");

        uint256[] memory mine = sched.jobsOf(owner);
        assertEq(mine.length, 1);
        assertEq(mine[0], id);
        assertEq(sched.jobCount(), 1);
    }

    function test_scheduleJob_reverts_zero_budget() public {
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.ZeroBudget.selector);
        sched.scheduleJob(TARGET, bytes("x"), INTERVAL, 0, MAX_RUNS);
    }

    function test_scheduleJob_reverts_below_min_interval() public {
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.ZeroInterval.selector);
        sched.scheduleJob(TARGET, bytes("x"), 59, BUDGET, MAX_RUNS); // < MIN_INTERVAL 60
    }

    function test_scheduleJob_reverts_zero_runs() public {
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.ZeroRuns.selector);
        sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, 0);
    }

    function test_scheduleJob_reverts_unregistered_target() public {
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.UnregisteredTarget.selector);
        sched.scheduleJob(999, bytes("x"), INTERVAL, BUDGET, MAX_RUNS); // id 999 not registered
    }

    function test_scheduleJob_no_ghost_when_escrow_fails() public {
        // A fresh under-funded owner: approve but no balance → transferFrom
        // reverts → the whole tx reverts, no job persisted.
        address broke = address(0x0B0B);
        vm.prank(broke);
        lh.approve(address(sched), type(uint256).max);
        vm.prank(broke);
        vm.expectRevert(); // MockLH "balance"
        sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, MAX_RUNS);
        assertEq(sched.jobCount(), 0, "no job id consumed on a failed escrow");
    }

    // --- recordRun: role gate, CAS, debit, advance ----------------------

    function test_recordRun_only_scheduler() public {
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotScheduler.selector);
        sched.recordRun(id, due, COST);
    }

    function test_recordRun_debits_and_advances() public {
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due + 10);

        vm.prank(worker);
        uint64 newNext = sched.recordRun(id, due, COST);

        LibScheduleStorage.Job memory j = sched.getJob(id);
        assertEq(j.budgetWei, BUDGET - COST, "budget debited by cost");
        assertEq(j.runsLeft, MAX_RUNS - 1, "runsLeft decremented");
        assertEq(uint8(j.status), uint8(LibScheduleStorage.Status.Active));
        // Skip-don't-pile-up: next fire is now + interval, NOT old + interval.
        assertEq(newNext, uint64(block.timestamp) + INTERVAL);
        assertEq(j.nextRun, newNext);
        // The spent $LH stays in the diamond (becomes treasury); escrow
        // total unchanged (debit is internal accounting, not a transfer).
        assertEq(lh.balanceOf(address(sched)), BUDGET, "spent $LH stays in diamond as treasury");
    }

    function test_recordRun_reverts_not_due() public {
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        // Still before due.
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.NotDue.selector);
        sched.recordRun(id, due, COST);
    }

    function test_recordRun_double_fire_blocked_no_double_bill() public {
        // Two firers (the tab racing the cron) both read nextRun and both
        // try to record. The first lands; the second — replaying the SAME
        // expectedNextRun — MUST revert and MUST NOT debit again. After a
        // successful run, nextRun is advanced into the future (now +
        // interval), so the second replay trips the NotDue guard first;
        // the StaleNextRun CAS is the defense-in-depth behind it. Either
        // way the invariant that matters holds: no second debit.
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due + 1);

        vm.prank(worker);
        sched.recordRun(id, due, COST);

        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.NotDue.selector); // advanced nextRun is in the future
        sched.recordRun(id, due, COST);

        // Exactly one debit happened.
        assertEq(sched.getJob(id).budgetWei, BUDGET - COST, "no double-bill");
        assertEq(sched.getJob(id).runsLeft, MAX_RUNS - 1, "one run consumed");
    }

    function test_recordRun_cas_rejects_stale_expected_nextrun() public {
        // Direct CAS coverage: a firer that passes a WRONG expectedNextRun
        // (it read a stale value) is rejected by StaleNextRun while the job
        // is still genuinely due — no debit.
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due + 5); // job is due (now >= nextRun)

        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.StaleNextRun.selector);
        sched.recordRun(id, due - 1, COST); // expectedNextRun != stored nextRun

        assertEq(sched.getJob(id).budgetWei, BUDGET, "no debit on a stale CAS");
    }

    function test_recordRun_reverts_spend_over_budget() public {
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.SpendExceedsBudget.selector);
        sched.recordRun(id, due, BUDGET + 1);
    }

    // --- Hard stops: runs exhausted / budget exhausted, with refund -----

    function test_recordRun_exhausts_on_runs_and_refunds() public {
        // One run allowed, cheap cost: after it, runsLeft==0 → Exhausted +
        // refund the remainder.
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, 1);
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);

        uint256 ownerBefore = lh.balanceOf(owner);
        vm.prank(worker);
        uint64 newNext = sched.recordRun(id, due, COST);

        LibScheduleStorage.Job memory j = sched.getJob(id);
        assertEq(uint8(j.status), uint8(LibScheduleStorage.Status.Exhausted), "runs exhausted");
        assertEq(j.budgetWei, 0, "budget zeroed after refund");
        assertEq(newNext, 0, "no next run");
        // Remainder (BUDGET - COST) refunded to owner; the COST stays as treasury.
        assertEq(lh.balanceOf(owner), ownerBefore + (BUDGET - COST), "remainder refunded");
        assertEq(lh.balanceOf(address(sched)), COST, "only the spent cost retained");
    }

    function test_recordRun_exhausts_on_budget_and_refunds() public {
        // Budget just covers two runs; the third would underflow the
        // "remaining >= cost" check → Exhausted on the run that empties it.
        uint128 budget = 2 * COST; // exactly two runs of COST
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, budget, 100);

        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, COST); // budget now == COST, runs left

        LibScheduleStorage.Job memory j = sched.getJob(id);
        assertEq(j.budgetWei, COST);
        assertEq(uint8(j.status), uint8(LibScheduleStorage.Status.Active));

        // Second run drains to 0; remaining(0) < cost → Exhausted, no refund left.
        uint64 due2 = j.nextRun;
        vm.warp(due2);
        vm.prank(worker);
        sched.recordRun(id, due2, COST);

        j = sched.getJob(id);
        assertEq(uint8(j.status), uint8(LibScheduleStorage.Status.Exhausted), "budget exhausted");
        assertEq(j.budgetWei, 0);
        assertEq(lh.balanceOf(address(sched)), budget, "all of it spent as treasury, nothing refunded");
    }

    // --- cancel: refunds remainder, terminal ----------------------------

    function test_cancelJob_refunds_remainder() public {
        uint256 id = _schedule();
        // Run once so there's a partial spend.
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, COST);

        uint256 ownerBefore = lh.balanceOf(owner);
        vm.prank(owner);
        sched.cancelJob(id);

        LibScheduleStorage.Job memory j = sched.getJob(id);
        assertEq(uint8(j.status), uint8(LibScheduleStorage.Status.Cancelled));
        assertEq(j.budgetWei, 0, "budget zeroed");
        // Remainder (BUDGET - COST) refunded.
        assertEq(lh.balanceOf(owner), ownerBefore + (BUDGET - COST), "remaining budget refunded");
        assertEq(lh.balanceOf(address(sched)), COST, "diamond keeps only the spent cost");
    }

    function test_cancelJob_only_owner() public {
        uint256 id = _schedule();
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.cancelJob(id);
    }

    function test_cancelJob_reverts_when_already_terminal() public {
        uint256 id = _schedule();
        vm.prank(owner);
        sched.cancelJob(id);
        // Second cancel hits the non-active/paused guard.
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.cancelJob(id);
    }

    function test_recordRun_reverts_on_cancelled_job() public {
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.prank(owner);
        sched.cancelJob(id);
        vm.warp(due);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.recordRun(id, due, COST);
    }

    // --- pause / resume -------------------------------------------------

    function test_pause_blocks_run_then_resume_reschedules() public {
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;

        vm.prank(owner);
        sched.pauseJob(id);
        assertEq(uint8(sched.getJob(id).status), uint8(LibScheduleStorage.Status.Paused));

        // Paused job is not runnable.
        vm.warp(due + 100);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.recordRun(id, due, COST);

        // Resume reschedules forward from now (no instant-due burst).
        vm.prank(owner);
        sched.resumeJob(id);
        LibScheduleStorage.Job memory j = sched.getJob(id);
        assertEq(uint8(j.status), uint8(LibScheduleStorage.Status.Active));
        assertEq(j.nextRun, uint64(block.timestamp) + INTERVAL, "resume reschedules from now");
    }

    function test_pause_only_owner() public {
        uint256 id = _schedule();
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.pauseJob(id);
    }

    function test_resume_reverts_when_not_paused() public {
        uint256 id = _schedule(); // Active, not Paused
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.JobNotPaused.selector);
        sched.resumeJob(id);
    }

    function test_cancel_a_paused_job_refunds() public {
        uint256 id = _schedule();
        vm.prank(owner);
        sched.pauseJob(id);
        uint256 ownerBefore = lh.balanceOf(owner);
        vm.prank(owner);
        sched.cancelJob(id);
        assertEq(lh.balanceOf(owner), ownerBefore + BUDGET, "paused job cancel refunds full budget");
    }

    // --- topUp ----------------------------------------------------------

    function test_topUpJob_escrows_more() public {
        uint256 id = _schedule();
        uint128 add = 50 ether;
        uint256 diamondBefore = lh.balanceOf(address(sched));
        vm.prank(owner);
        sched.topUpJob(id, add);
        assertEq(sched.getJob(id).budgetWei, BUDGET + add);
        assertEq(lh.balanceOf(address(sched)), diamondBefore + add, "extra $LH escrowed");
    }

    function test_topUpJob_only_owner() public {
        uint256 id = _schedule();
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.topUpJob(id, 1 ether);
    }

    // --- setScheduler (diamond owner) -----------------------------------

    function test_setScheduler_only_diamond_owner() public {
        vm.prank(stranger);
        vm.expectRevert("LibDiamond: not owner");
        sched.setScheduler(stranger);
    }

    function test_setScheduler_updates() public {
        address newWorker = address(0x9999);
        vm.prank(diamondOwner);
        sched.setScheduler(newWorker);
        assertEq(sched.schedulerAddress(), newWorker);
    }

    // --- jobsDue pagination ---------------------------------------------

    function test_jobsDue_returns_only_due_active_paged() public {
        // Schedule 5 jobs. Their nextRun = now + INTERVAL.
        uint256[] memory ids = new uint256[](5);
        for (uint256 i = 0; i < 5; i++) {
            vm.prank(owner);
            ids[i] = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, MAX_RUNS);
        }

        // Before any are due: empty.
        (uint256[] memory due0,) = sched.jobsDue(0, 100);
        assertEq(due0.length, 0, "nothing due before interval elapses");

        // Advance past due. Pause job index 2, cancel job index 4 — both
        // must be filtered out of the due set.
        vm.warp(block.timestamp + INTERVAL + 1);
        vm.prank(owner);
        sched.pauseJob(ids[2]);
        vm.prank(owner);
        sched.cancelJob(ids[4]);

        // Page 1: first 2 of the index window.
        (uint256[] memory p1, uint256 cur1) = sched.jobsDue(0, 2);
        assertEq(cur1, 2, "cursor advanced by limit");
        assertEq(p1.length, 2, "ids 1,2 are active+due"); // jobIds[0], jobIds[1]
        assertEq(p1[0], ids[0]);
        assertEq(p1[1], ids[1]);

        // Page 2: window [2,4) covers ids[2] (paused) + ids[3] (active) →
        // only ids[3] returned.
        (uint256[] memory p2, uint256 cur2) = sched.jobsDue(cur1, 2);
        assertEq(cur2, 4);
        assertEq(p2.length, 1, "paused id filtered out");
        assertEq(p2[0], ids[3]);

        // Page 3: window [4,5) covers ids[4] (cancelled) → empty.
        (uint256[] memory p3, uint256 cur3) = sched.jobsDue(cur2, 2);
        assertEq(cur3, 5, "cursor at end");
        assertEq(p3.length, 0, "cancelled id filtered out");

        // Past the end: empty, cursor clamps to total.
        (uint256[] memory p4, uint256 cur4) = sched.jobsDue(cur3, 2);
        assertEq(p4.length, 0);
        assertEq(cur4, 5);
    }
}
