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

/// Reentrant TIP-20 mock: on `transfer` (the refund path) it tries to
/// re-enter `scheduleChildJob` for the just-cancelled job. Real `$LH` has
/// NO callback; this is a defense-in-depth probe that the recursion path
/// can't be abused mid-refund to mint budget from a terminal job.
contract ReentrantChildLH {
    mapping(address => uint256) public balanceOf;
    mapping(address => mapping(address => uint256)) public allowance;

    address public diamond;
    uint256 public attackParent;
    uint256 public attackTarget;
    bool internal entered;
    bool public reenterReverted;

    function arm(address d, uint256 parent, uint256 target) external {
        diamond = d;
        attackParent = parent;
        attackTarget = target;
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
        // Re-enter once during the refund external call: try to spawn a
        // child from the (now-Cancelled) parent. Must revert (ParentNotActive).
        if (diamond != address(0) && !entered) {
            entered = true;
            try ScheduleFacet(diamond).scheduleChildJob(attackParent, attackTarget, bytes("re"), 3600, 1 ether, 1) {
                reenterReverted = false;
            } catch {
                reenterReverted = true;
            }
        }
        return true;
    }
}

/// Harness wiring the shared diamond-storage slots, same as the base test.
contract HardenHarness is ScheduleFacet {
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

contract ScheduleHardeningAdversarialTest is Test {
    HardenHarness sched;
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

    // Mirror the facet constants (internal in the facet; restate here).
    uint256 constant MAX_ACTIVE = 32;
    uint64 constant MAX_DEPTH = 4;

    function setUp() public {
        sched = new HardenHarness();
        lh = new MockLH();
        sched._setCreditsToken(address(lh));
        sched._setDiamondOwner(diamondOwner);
        sched._registerTarget(TARGET, address(0xCAFE));

        vm.prank(diamondOwner);
        sched.setScheduler(worker);

        lh.mint(owner, 100_000 ether);
        vm.prank(owner);
        lh.approve(address(sched), type(uint256).max);

        lh.mint(ownerB, 100_000 ether);
        vm.prank(ownerB);
        lh.approve(address(sched), type(uint256).max);

        vm.warp(1_000_000);
    }

    function _schedule(address who, uint128 budget) internal returns (uint256 id) {
        vm.prank(who);
        id = sched.scheduleJob(TARGET, bytes("task"), INTERVAL, budget, MAX_RUNS);
    }

    function _liveBudgetSum() internal view returns (uint256 sum) {
        uint256 n = sched.jobCount();
        for (uint256 i = 1; i <= n; i++) {
            sum += sched.getJob(i).budgetWei;
        }
    }

    // === #3 PER-OWNER ACTIVE-JOB CAP ====================================

    function test_cap_blocks_the_Nplus1th_job() public {
        // Fill exactly to the cap, then the next scheduleJob reverts.
        for (uint256 i = 0; i < MAX_ACTIVE; i++) {
            _schedule(owner, 1 ether);
        }
        assertEq(sched.activeJobCountOf(owner), MAX_ACTIVE, "owner at the cap");
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.TooManyActiveJobs.selector);
        sched.scheduleJob(TARGET, bytes("over"), INTERVAL, 1 ether, MAX_RUNS);
    }

    function test_cap_no_escrow_pulled_on_cap_revert() public {
        for (uint256 i = 0; i < MAX_ACTIVE; i++) {
            _schedule(owner, 1 ether);
        }
        uint256 balBefore = lh.balanceOf(owner);
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.TooManyActiveJobs.selector);
        sched.scheduleJob(TARGET, bytes("over"), INTERVAL, 5 ether, MAX_RUNS);
        assertEq(lh.balanceOf(owner), balBefore, "cap revert pulls no escrow");
    }

    function test_cap_is_per_owner_not_global() public {
        // owner fills the cap; ownerB can still schedule freely.
        for (uint256 i = 0; i < MAX_ACTIVE; i++) {
            _schedule(owner, 1 ether);
        }
        uint256 id = _schedule(ownerB, 1 ether);
        assertEq(sched.getJob(id).owner, ownerB);
        assertEq(sched.activeJobCountOf(ownerB), 1, "ownerB independent count");
    }

    function test_cap_decrements_on_cancel_then_reschedule_allowed() public {
        uint256[] memory ids = new uint256[](MAX_ACTIVE);
        for (uint256 i = 0; i < MAX_ACTIVE; i++) {
            ids[i] = _schedule(owner, 1 ether);
        }
        // At cap -> next reverts.
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.TooManyActiveJobs.selector);
        sched.scheduleJob(TARGET, bytes("x"), INTERVAL, 1 ether, MAX_RUNS);

        // Cancel one -> count drops -> a new schedule succeeds.
        vm.prank(owner);
        sched.cancelJob(ids[0]);
        assertEq(sched.activeJobCountOf(owner), MAX_ACTIVE - 1, "cancel decremented");
        uint256 fresh = _schedule(owner, 1 ether);
        assertEq(sched.getJob(fresh).owner, owner, "reschedule allowed after cancel");
        assertEq(sched.activeJobCountOf(owner), MAX_ACTIVE, "back at cap");
    }

    function test_cap_decrements_on_exhaust() public {
        // One job, exhaust it on runs, count returns to 0.
        vm.prank(owner);
        uint256 id = sched.scheduleJob(TARGET, bytes("x"), INTERVAL, 10 ether, 1);
        assertEq(sched.activeJobCountOf(owner), 1);
        uint64 due = sched.getJob(id).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(id, due, COST); // 1 run -> Exhausted
        assertEq(uint8(sched.getJob(id).status), uint8(LibScheduleStorage.Status.Exhausted));
        assertEq(sched.activeJobCountOf(owner), 0, "exhaust decremented the cap");
    }

    function test_cap_pause_does_not_decrement_cancel_after_pause_does() public {
        // Paused jobs still count (they can resume); only terminal exits free a slot.
        uint256 id = _schedule(owner, 1 ether);
        assertEq(sched.activeJobCountOf(owner), 1);
        vm.prank(owner);
        sched.pauseJob(id);
        assertEq(sched.activeJobCountOf(owner), 1, "pause keeps the slot held");
        vm.prank(owner);
        sched.cancelJob(id); // cancel a Paused job
        assertEq(sched.activeJobCountOf(owner), 0, "cancel of paused frees the slot");
    }

    function test_cap_double_cancel_does_not_double_decrement() public {
        // Underflow guard: a second cancel reverts (not Active) so the
        // counter can't go negative.
        uint256 a = _schedule(owner, 1 ether);
        uint256 b = _schedule(owner, 1 ether);
        assertEq(sched.activeJobCountOf(owner), 2);
        vm.prank(owner);
        sched.cancelJob(a);
        assertEq(sched.activeJobCountOf(owner), 1);
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.JobNotActive.selector);
        sched.cancelJob(a); // already cancelled
        assertEq(sched.activeJobCountOf(owner), 1, "no second decrement");
        // b unaffected and still cancellable.
        vm.prank(owner);
        sched.cancelJob(b);
        assertEq(sched.activeJobCountOf(owner), 0);
    }

    // === #4 scheduleChildJob: ACCESS CONTROL ============================

    function test_child_scheduler_only_non_scheduler_reverts() public {
        uint256 parent = _schedule(owner, BUDGET);
        // Job owner cannot call it.
        vm.prank(owner);
        vm.expectRevert(ScheduleFacet.NotScheduler.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 10 ether, 3);
        // A stranger cannot.
        vm.prank(stranger);
        vm.expectRevert(ScheduleFacet.NotScheduler.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 10 ether, 3);
        // Even the diamond owner cannot.
        vm.prank(diamondOwner);
        vm.expectRevert(ScheduleFacet.NotScheduler.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 10 ether, 3);
    }

    // === #4 scheduleChildJob: BUDGET DRAW ACCOUNTING ====================

    function test_child_draws_exactly_from_parent_budget() public {
        uint256 parent = _schedule(owner, BUDGET);
        uint128 draw = 30 ether;
        uint256 diamondBalBefore = lh.balanceOf(address(sched));
        uint256 liveSumBefore = _liveBudgetSum();

        vm.prank(worker);
        uint256 child = sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, draw, 3);

        // Parent shrinks by EXACTLY the child's budget.
        assertEq(sched.getJob(parent).budgetWei, BUDGET - draw, "parent shrank by draw");
        assertEq(sched.getJob(child).budgetWei, draw, "child holds the draw");
        // NO new escrow pulled — diamond balance unchanged (pure internal move).
        assertEq(lh.balanceOf(address(sched)), diamondBalBefore, "no transfer on child schedule");
        // Live-budget SUM is conserved (wei just moved rows).
        assertEq(_liveBudgetSum(), liveSumBefore, "total live budget conserved across the move");
        // Child inherits the parent's owner.
        assertEq(sched.getJob(child).owner, owner, "child inherits parent owner");
    }

    function test_child_reverts_on_insufficient_parent_budget() public {
        uint256 parent = _schedule(owner, 10 ether);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.InsufficientParentBudget.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 11 ether, 1);
        // Parent budget untouched after the revert.
        assertEq(sched.getJob(parent).budgetWei, 10 ether, "parent intact on revert");
    }

    function test_child_exact_parent_budget_drains_to_zero() public {
        uint256 parent = _schedule(owner, 40 ether);
        vm.prank(worker);
        uint256 child = sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 40 ether, 1);
        assertEq(sched.getJob(parent).budgetWei, 0, "parent fully drained");
        assertEq(sched.getJob(child).budgetWei, 40 ether, "child holds all of it");
    }

    function test_child_zero_budget_reverts() public {
        uint256 parent = _schedule(owner, BUDGET);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.ZeroBudget.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 0, 1);
    }

    function test_child_unknown_parent_reverts() public {
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.UnknownJob.selector);
        sched.scheduleChildJob(999, TARGET, bytes("c"), INTERVAL, 1 ether, 1);
    }

    function test_child_parent_not_active_reverts() public {
        uint256 parent = _schedule(owner, BUDGET);
        vm.prank(owner);
        sched.cancelJob(parent); // parent now Cancelled
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.ParentNotActive.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 1 ether, 1);
    }

    function test_child_unregistered_target_reverts() public {
        uint256 parent = _schedule(owner, BUDGET);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.UnregisteredTarget.selector);
        sched.scheduleChildJob(parent, 999, bytes("c"), INTERVAL, 1 ether, 1);
    }

    function test_child_below_min_interval_reverts() public {
        uint256 parent = _schedule(owner, BUDGET);
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.ZeroInterval.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), 59, 1 ether, 1);
    }

    // === #4 DEPTH ENFORCEMENT ===========================================

    function test_child_depth_metadata_and_max_depth_exceeded() public {
        // Build a chain root -> d1 -> d2 -> d3 -> d4, then d4's child reverts.
        uint256 root = _schedule(owner, BUDGET);
        (uint256 p0, uint64 d0, uint256 r0) = sched.childMetaOf(root);
        assertEq(p0, 0);
        assertEq(d0, 0, "root depth 0");
        assertEq(r0, 0, "root has no rootId meta");

        uint256 prev = root;
        for (uint64 depth = 1; depth <= MAX_DEPTH; depth++) {
            vm.prank(worker);
            uint256 child = sched.scheduleChildJob(prev, TARGET, bytes("c"), INTERVAL, 5 ether, 1);
            (uint256 pp, uint64 dd, uint256 rr) = sched.childMetaOf(child);
            assertEq(pp, prev, "parentId chains");
            assertEq(dd, depth, "depth increments by 1");
            assertEq(rr, root, "rootId stays the original root");
            prev = child;
        }
        // prev is now at MAX_DEPTH; one more child must revert.
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.MaxDepthExceeded.selector);
        sched.scheduleChildJob(prev, TARGET, bytes("c"), INTERVAL, 1 ether, 1);
    }

    // === #4 ROOT-BUDGET-CAPS-THE-TREE INVARIANT =========================

    function test_root_budget_caps_the_whole_tree() public {
        // A fan-out tree: root -> several children + grandchildren. The sum
        // of ALL live budgets in the tree can never exceed the root's
        // original budget, because every child is carved from a parent's
        // escrow and no new $LH is minted.
        uint128 rootBudget = 100 ether;
        uint256 root = _schedule(owner, rootBudget);

        // Two direct children.
        vm.prank(worker);
        uint256 c1 = sched.scheduleChildJob(root, TARGET, bytes("c1"), INTERVAL, 40 ether, 5);
        vm.prank(worker);
        uint256 c2 = sched.scheduleChildJob(root, TARGET, bytes("c2"), INTERVAL, 30 ether, 5);
        // A grandchild out of c1.
        vm.prank(worker);
        uint256 g1 = sched.scheduleChildJob(c1, TARGET, bytes("g1"), INTERVAL, 20 ether, 5);

        // Sum of the tree's live budgets == root original (nothing minted).
        uint256 treeSum = sched.getJob(root).budgetWei + sched.getJob(c1).budgetWei
            + sched.getJob(c2).budgetWei + sched.getJob(g1).budgetWei;
        assertEq(treeSum, rootBudget, "tree budget sum == root original");
        assertLe(treeSum, rootBudget, "tree never exceeds the root cap");

        // A child can never draw more than its parent currently holds, so
        // the cap holds no matter the request order.
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.InsufficientParentBudget.selector);
        sched.scheduleChildJob(root, TARGET, bytes("c3"), INTERVAL, 31 ether, 1); // root has 30 left

        // Diamond escrow still exactly the root's original (no mint, no extra pull).
        assertEq(lh.balanceOf(address(sched)), rootBudget, "diamond holds exactly the escrow");
    }

    function test_root_budget_caps_tree_after_runs_spend_treasury() public {
        // After children run and spend, the live tree sum only ever
        // DECREASES (spent wei becomes treasury); never exceeds the root.
        uint128 rootBudget = 60 ether;
        uint256 root = _schedule(owner, rootBudget);
        vm.prank(worker);
        uint256 c1 = sched.scheduleChildJob(root, TARGET, bytes("c1"), INTERVAL, 30 ether, 5);

        // Run the child once, spending 10 ether.
        uint64 due = sched.getJob(c1).nextRun;
        vm.warp(due);
        vm.prank(worker);
        sched.recordRun(c1, due, 10 ether);

        uint256 treeSum = sched.getJob(root).budgetWei + sched.getJob(c1).budgetWei;
        assertEq(treeSum, rootBudget - 10 ether, "spent wei left the live tree as treasury");
        assertLe(treeSum, rootBudget, "still capped by the root original");
    }

    // === #4 CHILD OWNER INHERITANCE + REFUND ============================

    function test_child_cancel_refunds_the_inherited_owner() public {
        // Worker spawns a child off owner's parent; only owner (the
        // inherited owner) can cancel, and the refund lands on owner.
        uint256 parent = _schedule(owner, BUDGET);
        vm.prank(worker);
        uint256 child = sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 25 ether, 3);
        assertEq(sched.getJob(child).owner, owner, "child owner inherited");

        // The worker is NOT the owner -> cannot cancel the child.
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.NotJobOwner.selector);
        sched.cancelJob(child);

        // The inherited owner cancels -> refund of the child's 25 ether
        // goes to owner (NOT to the worker).
        uint256 ownerBal = lh.balanceOf(owner);
        uint256 workerBal = lh.balanceOf(worker);
        vm.prank(owner);
        sched.cancelJob(child);
        assertEq(lh.balanceOf(owner), ownerBal + 25 ether, "child refund to inherited owner");
        assertEq(lh.balanceOf(worker), workerBal, "worker never receives the refund");
    }

    function test_child_counts_toward_parent_owner_cap() public {
        // A child increments the PARENT owner's activeJobsOf (not the
        // worker's). Fill owner to the cap-1 with a parent, then children
        // count toward owner.
        uint256 parent = _schedule(owner, BUDGET); // owner count = 1
        assertEq(sched.activeJobCountOf(owner), 1);
        vm.prank(worker);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 10 ether, 3);
        assertEq(sched.activeJobCountOf(owner), 2, "child bumped the PARENT owner's count");
        assertEq(sched.activeJobCountOf(worker), 0, "worker count untouched");
    }

    function test_child_blocked_when_inherited_owner_at_cap() public {
        // Fill owner to the cap with their own jobs (one is the parent),
        // then a child for that parent reverts TooManyActiveJobs and the
        // parent budget is NOT drained.
        uint256 parent = _schedule(owner, BUDGET); // 1
        for (uint256 i = 0; i < MAX_ACTIVE - 1; i++) {
            _schedule(owner, 1 ether); // up to MAX_ACTIVE
        }
        assertEq(sched.activeJobCountOf(owner), MAX_ACTIVE, "owner at cap");
        uint128 parentBudgetBefore = sched.getJob(parent).budgetWei;
        vm.prank(worker);
        vm.expectRevert(ScheduleFacet.TooManyActiveJobs.selector);
        sched.scheduleChildJob(parent, TARGET, bytes("c"), INTERVAL, 5 ether, 1);
        assertEq(sched.getJob(parent).budgetWei, parentBudgetBefore, "parent budget rolled back on cap revert");
    }

    // === #4 REENTRANCY PROBE ============================================

    function test_reentrant_child_during_refund_cannot_spawn_from_terminal() public {
        ReentrantChildLH rlh = new ReentrantChildLH();
        HardenHarness s2 = new HardenHarness();
        s2._setCreditsToken(address(rlh));
        s2._setDiamondOwner(diamondOwner);
        s2._registerTarget(TARGET, address(0xCAFE));
        vm.prank(diamondOwner);
        s2.setScheduler(worker);

        rlh.mint(owner, 1_000 ether);
        vm.prank(owner);
        rlh.approve(address(s2), type(uint256).max);
        rlh.mint(address(s2), 1_000 ether); // extra funds so a double-spawn would have something to steal

        vm.prank(owner);
        uint256 parent = s2.scheduleJob(TARGET, bytes("p"), INTERVAL, BUDGET, MAX_RUNS);

        // The reentrant token calls scheduleChildJob during the refund.
        // But the refund callback runs as the TOKEN (msg.sender != worker),
        // AND the parent is already Cancelled — either guard reverts it.
        rlh.arm(address(s2), parent, TARGET);

        uint256 jobsBefore = s2.jobCount();
        vm.prank(owner);
        s2.cancelJob(parent); // refund transfer triggers a reentrant scheduleChildJob

        assertTrue(rlh.reenterReverted(), "reentrant scheduleChildJob reverted");
        assertEq(s2.jobCount(), jobsBefore, "no phantom child created by reentrancy");
        assertEq(uint8(s2.getJob(parent).status), uint8(LibScheduleStorage.Status.Cancelled));
    }

    // === FUZZ: child draws never create or destroy budget ===============

    function testFuzz_child_draws_conserve_tree_budget(uint256 seedRaw) public {
        uint128 rootBudget = 1000 ether;
        uint256 root = _schedule(owner, rootBudget);
        uint256 diamondStart = lh.balanceOf(address(sched));

        uint256 seed = seedRaw;
        uint256[] memory parents = new uint256[](8);
        parents[0] = root;
        uint256 pcount = 1;

        for (uint256 i = 0; i < 12; i++) {
            uint256 pidx = seed % pcount;
            uint256 parent = parents[pidx];
            seed = uint256(keccak256(abi.encode(seed, i)));

            LibScheduleStorage.Job memory pj = sched.getJob(parent);
            if (pj.status != LibScheduleStorage.Status.Active || pj.budgetWei == 0) continue;
            (, uint64 pdepth,) = sched.childMetaOf(parent);
            if (pdepth >= MAX_DEPTH) continue;

            uint128 draw = uint128(1 ether + (seed % uint256(pj.budgetWei)));
            if (draw > pj.budgetWei) draw = pj.budgetWei;
            if (draw == 0) continue;

            vm.prank(worker);
            uint256 child = sched.scheduleChildJob(parent, TARGET, bytes("f"), INTERVAL, draw, 2);
            if (pcount < parents.length) {
                parents[pcount] = child;
                pcount++;
            }

            // INVARIANT: the diamond's escrow NEVER changes on a child move
            // (no mint, no transfer) and the live-budget sum stays <= the
            // root original (== it, since nothing has been spent yet).
            assertEq(lh.balanceOf(address(sched)), diamondStart, "no escrow movement on child draw");
            assertLe(_liveBudgetSum(), rootBudget, "tree budget never exceeds root original");
        }
        // With no runs recorded, the live sum equals the root original exactly.
        assertEq(_liveBudgetSum(), rootBudget, "every wei still live in the tree");
    }
}
