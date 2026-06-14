// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {CounterFacet} from "../src/facets/CounterFacet.sol";

/// CounterFacet unit tests. The facet is exercised directly —
/// `LibCounterStorage.s()` resolves against THIS deployment's storage, so the
/// `view` reads see exactly what the writes stored (same pattern as
/// SignalingAuth.t.sol). We prove:
///   - increment bumps the caller's count + the global total by one
///   - incrementBy bumps both by n (and accumulates across calls)
///   - per-caller counts are isolated; total sums across callers
///   - the Incremented event fires with the right (who, newCount, newTotal)
///   - the require bounds revert: n == 0 ("zero"), n > 100 ("too big")
contract CounterFacetTest is Test {
    CounterFacet counter;

    address alice = address(0xA11CE);
    address bob = address(0xB0B);

    event Incremented(address indexed who, uint256 newCount, uint256 newTotal);

    function setUp() public {
        counter = new CounterFacet();
    }

    // --- increment -------------------------------------------------------

    function test_increment_bumps_count_and_total() public {
        vm.prank(alice);
        counter.increment();

        assertEq(counter.countOf(alice), 1, "alice count = 1");
        assertEq(counter.totalCount(), 1, "total = 1");

        vm.prank(alice);
        counter.increment();

        assertEq(counter.countOf(alice), 2, "alice count = 2");
        assertEq(counter.totalCount(), 2, "total = 2");
    }

    function test_increment_emits_event() public {
        vm.expectEmit(true, false, false, true);
        emit Incremented(alice, 1, 1);
        vm.prank(alice);
        counter.increment();
    }

    // --- incrementBy -----------------------------------------------------

    function test_incrementBy_bumps_by_n() public {
        vm.prank(alice);
        counter.incrementBy(5);
        assertEq(counter.countOf(alice), 5, "alice count = 5");
        assertEq(counter.totalCount(), 5, "total = 5");

        vm.prank(alice);
        counter.incrementBy(3);
        assertEq(counter.countOf(alice), 8, "alice count accumulates = 8");
        assertEq(counter.totalCount(), 8, "total = 8");
    }

    function test_incrementBy_boundary_100_ok() public {
        vm.prank(alice);
        counter.incrementBy(100);
        assertEq(counter.countOf(alice), 100, "n == 100 accepted");
        assertEq(counter.totalCount(), 100, "total = 100");
    }

    function test_incrementBy_emits_event() public {
        vm.expectEmit(true, false, false, true);
        emit Incremented(bob, 7, 7);
        vm.prank(bob);
        counter.incrementBy(7);
    }

    // --- per-caller isolation + global total ----------------------------

    function test_counts_are_per_caller_total_is_global() public {
        vm.prank(alice);
        counter.incrementBy(4);
        vm.prank(bob);
        counter.incrementBy(6);

        assertEq(counter.countOf(alice), 4, "alice isolated");
        assertEq(counter.countOf(bob), 6, "bob isolated");
        assertEq(counter.totalCount(), 10, "total sums across callers");

        vm.prank(alice);
        counter.increment();
        assertEq(counter.countOf(alice), 5, "alice +1");
        assertEq(counter.countOf(bob), 6, "bob untouched");
        assertEq(counter.totalCount(), 11, "total = 11");
    }

    function test_countOf_unseen_is_zero() public view {
        assertEq(counter.countOf(address(0xDEAD)), 0, "unseen address reads zero");
        assertEq(counter.totalCount(), 0, "fresh total is zero");
    }

    // --- require bounds (revert paths) ----------------------------------

    function test_incrementBy_zero_reverts() public {
        vm.expectRevert(bytes("zero"));
        vm.prank(alice);
        counter.incrementBy(0);
        assertEq(counter.totalCount(), 0, "no state change on revert");
    }

    function test_incrementBy_too_big_reverts() public {
        vm.expectRevert(bytes("too big"));
        vm.prank(alice);
        counter.incrementBy(101);
        assertEq(counter.totalCount(), 0, "no state change on revert");
    }
}
