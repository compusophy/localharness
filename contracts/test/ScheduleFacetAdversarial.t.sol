// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {ScheduleFacet} from "../src/facets/ScheduleFacet.sol";
import {LibScheduleStorage} from "../src/libraries/LibScheduleStorage.sol";
import {LibCreditsStorage} from "../src/libraries/LibCreditsStorage.sol";
import {LibRegistryStorage} from "../src/libraries/LibRegistryStorage.sol";
import {LibDiamond} from "../src/libraries/LibDiamond.sol";

/// Standard `$LH`-shaped TIP-20 mock (no callback — the real token).
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

/// Reentrant TIP-20 mock: on `transfer` (the refund path), it calls back
/// into the diamond's `cancelJob`/`recordRun` for `attackJob` before
/// returning. Real `$LH` has NO callback, so this is a defense-in-depth
/// probe of the facet's CEI ordering (a hostile token can't double-pay).
contract ReentrantLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint256 public attackJob;
    uint8 public mode; // 0 = off, 1 = cancel, 2 = recordRun-replay
    bool internal entered;
    bool public reenterReverted;

    function arm(address d, uint256 job, uint8 m) external {
        diamond = d;
        attackJob = job;
        mode = m;
    }

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
        // Re-enter once during the refund external call.
        if (mode != 0 && !entered) {
            entered = true;
            if (mode == 1) {
                try ScheduleFacet(diamond).cancelJob(attackJob) {
                    reenterReverted = false;
                } catch {
                    reenterReverted = true;
                }
            }
        }
        return true;
    }
}

/// Harness wiring the shared diamond-storage slots, same as the base test.
contract AdvHarness is ScheduleFacet {
    function _setCreditsToken(address token) external {
        LibCreditsStorage.load().creditsToken = token;
    }

    function _registerTarget(uint256 id, address ownr) external {
        LibRegistryStorage.load().ownerOfId[id] = ownr;
    }

    function _setDiamondOwner(address ownr) external {
        LibDiamond.setContractOwner(ownr);
    }
}

contract ScheduleFacetAdversarialTest is Test {
    AdvHarness sched;
    MockLH lh;

    address owner = address(0xA11CE);
    address ownerB = address(0xB0B);
    address worker = address(0x401234);
    address diamondOwner = address(0xD1A);
    address stranger = address(0xBEEF);

    uint256 constant TARGET = 7;
    uint64 constant INTERVAL = 3600;
    uint128 constant BUDGET = 100 ether;
    uint32 constant MAX_RUNS = 10;
    uint128 constant COST = 5 ether;

    function setUp() public {
        sched = new AdvHarness();
        lh = new MockLH();
        sched._setCreditsToken(address(lh));
        sched._setDiamondOwner(diamondOwner);
        sched._registerTarget(TARGET, address(0xCAFE));

        vm.prank(diamondOwner);
        sched.setScheduler(worker);

        lh.mint(owner, 1_000 ether);
        vm.prank(owner);
        lh.approve(address(sched), type(uint256).max);

        lh.mint(ownerB, 1_000 ether);
        vm.prank(ownerB);
        lh.approve(address(sched), type(uint256).max);

        vm.warp(1_000_000);
    }

    function _schedule() internal returns (uint256 id) {
        vm.prank(owner);
        id = sched.scheduleJob(TARGET, bytes("task"), INTERVAL, BUDGET, MAX_RUNS);
    }

    // === CROSS-JOB ESCROW CONSERVATION ==================================
    // The diamond's $LH balance must NEVER drop below the sum of every
    // live job's budgetWei. If any refund over-pays, one job's refund
    // drains another job's escrow. This is the master invariant.

    function _liveBudgetSum() internal view returns (uint256 sum) {
        uint256 n = sched.jobCount();
        for (uint256 i = 1; i <= n; i++) {
            sum += sched.getJob(i).budgetWei;
        }
    }

    function test_invariant_diamond_balance_covers_all_live_budgets() public {
        // Two owners, several jobs, mixed run/cancel/exhaust, then assert
        // the diamond still holds >= the sum of remaining budgets at every
        // step (the spent remainder becomes treasury, never a deficit).
        uint256 a = _schedule(); // owner, BUDGET
        vm.prank(ownerB);
        uint256 b = sched.scheduleJob(TARGET, bytes("b"), INTERVAL, 40 ether, 3);
        vm.prank(owner);
        uint256 c = sched.scheduleJob(TARGET, bytes("c"), INTERVAL, 20 ether, 2);

        assertGe(lh.balanceOf(address(sched)), _liveBudgetSum(), "after escrow");

        // Run a once.
        uint64 dueA = sched.getJob(a).nextRun;
        vm.warp(dueA);
        vm.prank(worker);
        sched.recordRun(a, dueA, COST);
        assertGe(lh.balanceOf(address(sched)), _liveBudgetSum(), "after run a");

        // Cancel b (refunds its full 40 ether).
        vm.prank(ownerB);
        sched.cancelJob(b);
        assertGe(lh.balanceOf(address(sched)), _liveBudgetSum(), "after cancel b");

        // Exhaust c on runs (2 runs, cheap cost).
        uint64 dueC = sched.getJob(c).nextRun;
        vm.warp(dueC);
        vm.prank(worker);
        sched.recordRun(c, dueC, 1 ether);
        uint64 dueC2 = sched.getJob(c).nextRun;
        vm.warp(dueC2);
        vm.prank(worker);
        sched.recordRun(c, dueC2, 1 ether);
        assertGe(lh.balanceOf(address(sched)), _liveBudgetSum(), "after exhaust c");

        // Cancel a.
        vm.prank(owner);
        sched.cancelJob(a);
        assertGe(lh.balanceOf(address(sched)), _liveBudgetSum(), "after cancel a");
    }

    /// FUZZ the master invariant: across a random sequence of run / cancel
    /// / topUp / pause operations on several jobs, the diamond's $LH balance
    /// must NEVER fall below the sum of live job budgets (no over-refund can
    /// drain another job's escrow), and total refunds + retained treasury
    /// must equal total escrowed (conservation).
    function testFuzz_escrow_conservation_under_random_ops(uint256 seedRaw) public {
        uint256 seed = seedRaw;
        // Seed 4 jobs with varied budgets/runs.
        uint256[] memory ids = new uint256[](4);
        // The owner + diamond form a CLOSED system: every wei the owner
        // escrows lands in the diamond; every refund returns to the owner;
        // spent budget stays in the diamond as treasury. No wei is minted or
        // burned by the facet, so owner.balance + diamond.balance is constant
        // == the owner's full starting balance, forever. (Plus the per-step
        // under-collateralisation guard below.)
        uint256 closedSystem = lh.balanceOf(owner) + lh.balanceOf(address(sched));
        for (uint256 i = 0; i < 4; i++) {
            uint128 b = uint128(1 ether + (seed % 50) * 1 ether);
            uint32 r = uint32(1 + (seed % 6));
            seed = uint256(keccak256(abi.encode(seed)));
            vm.prank(owner);
            ids[i] = sched.scheduleJob(TARGET, bytes("f"), INTERVAL, b, r);
        }

        for (uint256 step = 0; step < 40; step++) {
            uint256 idx = seed % 4;
            uint256 op = (seed >> 8) % 4;
            uint256 jid = ids[idx];
            LibScheduleStorage.Job memory j = sched.getJob(jid);
            seed = uint256(keccak256(abi.encode(seed, step)));

            if (op == 0 && j.status == LibScheduleStorage.Status.Active) {
                // record a run (the worker)
                vm.warp(j.nextRun);
                uint128 cost = uint128((seed % 4) * 1 ether); // 0..3 ether
                if (cost > j.budgetWei) cost = j.budgetWei;
                vm.prank(worker);
                sched.recordRun(jid, j.nextRun, cost);
            } else if (op == 1 && (j.status == LibScheduleStorage.Status.Active || j.status == LibScheduleStorage.Status.Paused)) {
                vm.prank(owner);
                sched.cancelJob(jid);
            } else if (op == 2 && (j.status == LibScheduleStorage.Status.Active || j.status == LibScheduleStorage.Status.Paused)) {
                uint128 add = uint128(1 ether + (seed % 10) * 1 ether);
                vm.prank(owner);
                sched.topUpJob(jid, add);
            } else if (op == 3 && j.status == LibScheduleStorage.Status.Active) {
                vm.prank(owner);
                sched.pauseJob(jid);
            }

            // INVARIANT 1: diamond never under-collateralised (no refund can
            // over-pay and drain another job's escrow).
            assertGe(lh.balanceOf(address(sched)), _liveBudgetSum(), "diamond covers all live budgets");
            // INVARIANT 2: closed-system conservation holds at EVERY step.
            assertEq(
                lh.balanceOf(owner) + lh.balanceOf(address(sched)),
                closedSystem,
                "owner + diamond balance conserved"
            );
        }

        // Cancel every still-cancellable job to flush refunds.
        for (uint256 i = 0; i < 4; i++) {
            LibScheduleStorage.Job memory j = sched.getJob(ids[i]);
            if (j.status == LibScheduleStorage.Status.Active || j.status == LibScheduleStorage.Status.Paused) {
                vm.prank(owner);
                sched.cancelJob(ids[i]);
            }
        }

        // CONSERVATION: after all jobs are terminal there is no live budget,
        // and the closed system is intact — every escrowed wei is either back
        // with the owner (refund) or held by the diamond (treasury).
        assertEq(_liveBudgetSum(), 0, "no live budget left after all terminal");
        assertEq(
            lh.balanceOf(owner) + lh.balanceOf(address(sched)),
            closedSystem,
            "no wei created or destroyed across the whole run"
        );
    }

    // === DOUBLE-REFUND: cancel after exhaust, exhaust after cancel ======

    function test_cannot_cancel_after_exhaust() public {
        // Exhaust on runs (1 run), then a cancel must revert — no 2nd refund.
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, 1);
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, COST); // -> Exhausted, refunds BUDGET-COST

        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.cancelJob(id);
        assertEq(lh.balanceOf(owner), ownerBal, "no second refund after exhaust");
    }

    function test_cannot_record_after_cancel_then_exhaust_path() public {
        // Cancel refunds; a subsequent recordRun must revert (not Active),
        // so the exhaust-refund path can never fire on an already-refunded job.
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.prank(owner);
        sched.cancelJob(id);
        uint256 ownerBal = lh.balanceOf(owner);
        vm.warp(due);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.recordRun(id, due, COST);
        assertEq(lh.balanceOf(owner), ownerBal, "cancel then record: no extra pay");
    }

    function test_double_cancel_no_double_refund_exact_balance() public {
        uint256 id = _schedule();
        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(owner);
        sched.cancelJob(id); // refunds full BUDGET (no runs yet)
        assertEq(lh.balanceOf(owner), ownerBal + BUDGET, "first cancel refunds full budget");

        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.cancelJob(id);
        assertEq(lh.balanceOf(owner), ownerBal + BUDGET, "second cancel pays nothing");
    }

    // === REFUND RECIPIENT + AMOUNT EXACTNESS ============================

    function test_refund_amount_is_remaining_not_original() public {
        // After 3 runs of COST, cancel must refund BUDGET - 3*COST, never BUDGET.
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        for (uint256 i = 0; i < 3; i++) {
            vm.warp(due);
            vm.prank(worker);
            sched.recordRun(id, due, COST);
            due = sched.getJob(id).nextRun;
        }
        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(owner);
        sched.cancelJob(id);
        assertEq(lh.balanceOf(owner), ownerBal + (BUDGET - 3 * COST), "refund = remaining only");
    }

    function test_refund_goes_to_job_owner_not_canceller_context() public {
        // Job owner is `owner`; only owner can cancel, and refund lands on
        // the stored j.owner. (Stranger cancel already reverts; this asserts
        // recipient identity on the legitimate path.)
        uint256 id = _schedule();
        uint256 strangerBal = lh.balanceOf(stranger);
        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(owner);
        sched.cancelJob(id);
        assertEq(lh.balanceOf(stranger), strangerBal, "stranger never receives a refund");
        assertEq(lh.balanceOf(owner), ownerBal + BUDGET, "owner is the refund recipient");
    }

    // === recordRun ABUSE ===============================================

    function test_recordRun_unknown_job_reverts() public {
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.UnknownJob.selector);
        sched.recordRun(999, 0, COST);
    }

    function test_recordRun_id_zero_reverts() public {
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.UnknownJob.selector);
        sched.recordRun(0, 0, 0);
    }

    function test_recordRun_cannot_overdebit_across_many_runs() public {
        // Spend the budget down run-by-run; spentWei can never exceed the
        // CURRENT budget (each run re-checks against the decremented value),
        // so cumulative spend can't exceed the original escrow.
        uint128 budget = 30 ether;
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, budget, 1000);
        uint256 spent = 0;
        uint64 due = sched.getJob(id).nextRun;
        // Try to spend 10 ether each tick. Budget covers 3 (the 3rd leaves 0,
        // 0 < 10 => exhaust), so at most 30 ether ever leaves as treasury.
        for (uint256 i = 0; i < 5; i++) {
            LibScheduleStorage.Job memory j = sched.getJob(id);
            if (j.status != LibScheduleStorage.Status.Active) break;
            vm.warp(due);
            vm.prank(worker);
            sched.recordRun(id, due, 10 ether);
            spent += 10 ether;
            due = sched.getJob(id).nextRun;
        }
        // Total spent (retained as treasury) never exceeds the escrow.
        assertLe(spent, budget, "cumulative spend bounded by escrow");
        // Job is terminal; diamond retains exactly what it kept (budget),
        // owner got any refund. No deficit.
        assertGe(lh.balanceOf(address(sched)), _liveBudgetSum(), "no cross-job drain");
    }

    function test_recordRun_zero_cost_keeps_running_until_runs_out() public {
        // A zero-cost run decrements runsLeft but not budget; eventually
        // runsLeft==0 exhausts and refunds the FULL untouched budget.
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, 2);
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, 0); // runsLeft 2->1, still active, budget intact
        assertEq(sched.getJob(id).budgetWei, BUDGET);
        assertEq(uint8(sched.getJob(id).status), uint8(LibScheduleStorage.Status.Active));

        uint256 ownerBal = lh.balanceOf(owner);
        due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, 0); // runsLeft 1->0 -> Exhausted, refund full BUDGET
        assertEq(uint8(sched.getJob(id).status), uint8(LibScheduleStorage.Status.Exhausted));
        assertEq(lh.balanceOf(owner), ownerBal + BUDGET, "full budget refunded on zero-cost exhaust");
    }

    function test_recordRun_exact_budget_spend_exhausts_no_refund() public {
        // spentWei == remaining budget on a multi-run job: remaining hits 0,
        // 0 < spentWei => exhaust, nothing to refund, no underflow.
        uint128 budget = 10 ether;
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, budget, 5);
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(worker);
        sched.recordRun(id, due, budget); // spend it all in one run
        assertEq(uint8(sched.getJob(id).status), uint8(LibScheduleStorage.Status.Exhausted));
        assertEq(sched.getJob(id).budgetWei, 0);
        assertEq(lh.balanceOf(owner), ownerBal, "nothing to refund when fully spent");
    }

    function test_recordRun_replay_after_exhaust_no_second_refund() public {
        // The worker fires, the job exhausts + refunds. A racing/replayed
        // recordRun with the same args MUST revert (not Active) and pay
        // nothing more — the refund happened exactly once.
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, 1);
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(worker);
        sched.recordRun(id, due, COST); // Exhausted, refunds BUDGET-COST
        assertEq(lh.balanceOf(owner), ownerBal + (BUDGET - COST), "one refund");

        // Replay the exact same call.
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.recordRun(id, due, COST);
        assertEq(lh.balanceOf(owner), ownerBal + (BUDGET - COST), "replay pays nothing");
        assertEq(lh.balanceOf(address(sched)), COST, "diamond keeps exactly the spent cost");
    }

    function test_recordRun_pause_midstream_blocks_then_resume_then_cancel() public {
        // recordRun mid-flight vs owner pause: a paused job is not runnable;
        // recordRun reverts; resume + cancel refunds the unspent budget once.
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, COST); // 1 run

        vm.prank(owner);
        sched.pauseJob(id);

        uint64 due2 = sched.getJob(id).nextRun;
        vm.warp(due2 + 1000);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.recordRun(id, due2, COST); // paused -> not runnable

        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(owner);
        sched.cancelJob(id); // refunds BUDGET - COST
        assertEq(lh.balanceOf(owner), ownerBal + (BUDGET - COST), "paused-then-cancel refunds remainder once");
    }

    // === ESCROW VALIDATION ORDER ========================================

    function test_scheduleJob_rejects_unregistered_before_escrow() public {
        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.UnregisteredTarget.selector);
        sched.scheduleJob(999, bytes("x"), INTERVAL, BUDGET, MAX_RUNS);
        assertEq(lh.balanceOf(owner), ownerBal, "no escrow pulled on validation revert");
    }

    function test_scheduleJob_maxRuns_clamped_not_reverted() public {
        // maxRuns above the cap is silently clamped (not a revert) and the
        // job is otherwise valid + escrowed.
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, type(uint32).max);
        assertEq(sched.getJob(id).runsLeft, 1_000_000, "maxRuns clamped to MAX_RUNS");
    }

    // === topUp ADVERSARIAL ==============================================

    function test_topUp_on_exhausted_job_reverts() public {
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, 1);
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, COST); // Exhausted

        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.topUpJob(id, 10 ether);
    }

    function test_topUp_on_cancelled_job_reverts() public {
        uint256 id = _schedule();
        vm.prank(owner);
        sched.cancelJob(id);
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.topUpJob(id, 10 ether);
    }

    function test_topUp_zero_reverts() public {
        uint256 id = _schedule();
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.ZeroBudget.selector);
        sched.topUpJob(id, 0);
    }

    function test_topUp_no_ghost_increment_when_pull_fails() public {
        // CEI on topUp: a failed transferFrom must revert the budget bump.
        uint256 id = _schedule();
        // Drain owner's allowance so the next pull fails.
        vm.prank(owner);
        lh.approve(address(sched), 0);
        vm.prank(owner);
        vm.expectRevert(); // MockLH "allowance"
        sched.topUpJob(id, 10 ether);
        assertEq(sched.getJob(id).budgetWei, BUDGET, "budget unchanged after failed top-up");
    }

    function test_topUp_then_cancel_refunds_topped_up_total() public {
        uint256 id = _schedule();
        vm.prank(owner);
        sched.topUpJob(id, 50 ether);
        uint256 ownerBal = lh.balanceOf(owner);
        vm.prank(owner);
        sched.cancelJob(id);
        assertEq(lh.balanceOf(owner), ownerBal + BUDGET + 50 ether, "cancel refunds escrow + top-up");
    }

    // === REENTRANCY (defense-in-depth; real $LH has no callback) ========

    function test_reentrant_cancel_during_refund_cannot_double_pay() public {
        ReentrantLH rlh = new ReentrantLH();
        AdvHarness s2 = new AdvHarness();
        s2._setCreditsToken(address(rlh));
        s2._setDiamondOwner(diamondOwner);
        s2._registerTarget(TARGET, address(0xCAFE));
        vm.prank(diamondOwner);
        s2.setScheduler(worker);

        rlh.mint(owner, 1_000 ether);
        vm.prank(owner);
        rlh.approve(address(s2), type(uint256).max);
        // Fund the diamond extra so a (hypothetical) double-pay would have
        // funds to steal — proving the GUARD, not just insufficient balance.
        rlh.mint(address(s2), 1_000 ether);

        vm.prank(owner);
        uint256 id = s2.scheduleJob(TARGET, bytes("x"), INTERVAL, BUDGET, MAX_RUNS);

        // Arm the token to re-enter cancelJob(id) during the refund transfer.
        rlh.arm(address(s2), id, 1);

        uint256 ownerBefore = rlh.balanceOf(owner);
        vm.prank(owner);
        s2.cancelJob(id); // refund transfer triggers a reentrant cancelJob(id)

        // The reentrant cancel must have reverted (status already Cancelled),
        // so the owner is refunded EXACTLY once.
        assertTrue(rlh.reenterReverted(), "reentrant cancel reverted (CEI status-first)");
        assertEq(rlh.balanceOf(owner), ownerBefore + BUDGET, "exactly one refund, no double-pay");
        assertEq(uint8(s2.getJob(id).status), uint8(LibScheduleStorage.Status.Cancelled));
    }

    // === GRIEFING / ENUMERATION =========================================

    function test_jobsDue_paging_stable_under_griefing_spam() public {
        // A griefer floods many jobs; jobsDue paging stays bounded by `limit`
        // (the worker controls gas via the page size) and the cursor is
        // monotonic, so a flood can't wedge the scan.
        for (uint256 i = 0; i < 20; i++) {
            vm.prank(owner);
            sched.scheduleJob(TARGET, bytes("spam"), INTERVAL, 1 ether, 1);
        }
        vm.warp(block.timestamp + INTERVAL + 1);
        (uint256[] memory page, uint256 cursor) = sched.jobsDue(0, 5);
        assertLe(page.length, 5, "page bounded by limit");
        assertEq(cursor, 5, "cursor advances by limit regardless of flood");
        // Walk all pages; total due must equal the number scheduled.
        uint256 seen = page.length;
        while (cursor < sched.jobCount()) {
            (uint256[] memory p, uint256 c) = sched.jobsDue(cursor, 5);
            seen += p.length;
            cursor = c;
        }
        assertEq(seen, 20, "every due job enumerated across pages");
    }

    function test_non_owner_cannot_pause_cancel_topup() public {
        uint256 id = _schedule();
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.cancelJob(id);
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.pauseJob(id);
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.topUpJob(id, 1 ether);
        // ownerB (a different real owner) also cannot touch owner's job.
        vm.prank(ownerB);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.cancelJob(id);
    }

    function test_only_scheduler_can_record_even_diamond_owner_cannot() public {
        uint256 id = _schedule();
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        // Even the diamond owner is not the scheduler role.
        vm.prank(diamondOwner);
        vm.expectRevert(ScheduleFacet.NotScheduler.selector);
        sched.recordRun(id, due, COST);
        // And the job owner can't self-fire either.
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.NotScheduler.selector);
        sched.recordRun(id, due, COST);
    }
}
