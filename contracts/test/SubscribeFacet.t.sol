// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {SubscribeFacet} from "../src/facets/SubscribeFacet.sol";

contract SubscribeFacetTest is Test {
    SubscribeFacet sub;

    address alice = address(0xA11CE);
    address bob = address(0xB0B);
    address carol = address(0xCA401);
    uint256 constant FEED = 7;

    event Subscribed(uint256 indexed targetId, address indexed who);
    event Unsubscribed(uint256 indexed targetId, address indexed who);

    function setUp() public {
        sub = new SubscribeFacet();
    }

    function test_subscribe_adds_member() public {
        vm.expectEmit(true, true, false, false);
        emit Subscribed(FEED, alice);
        vm.prank(alice);
        sub.subscribe(FEED);

        assertTrue(sub.isSubscribed(FEED, alice));
        assertEq(sub.subscriberCount(FEED), 1);
        assertEq(sub.subscribersOf(FEED)[0], alice);
    }

    function test_subscribe_is_idempotent() public {
        vm.startPrank(alice);
        sub.subscribe(FEED);
        sub.subscribe(FEED); // no double-add, no revert
        vm.stopPrank();
        assertEq(sub.subscriberCount(FEED), 1);
    }

    function test_unsubscribe_removes_and_keeps_others() public {
        vm.prank(alice);
        sub.subscribe(FEED);
        vm.prank(bob);
        sub.subscribe(FEED);
        vm.prank(carol);
        sub.subscribe(FEED);
        assertEq(sub.subscriberCount(FEED), 3);

        // Remove the MIDDLE one — swap-remove must keep alice + carol valid.
        vm.expectEmit(true, true, false, false);
        emit Unsubscribed(FEED, bob);
        vm.prank(bob);
        sub.unsubscribe(FEED);

        assertEq(sub.subscriberCount(FEED), 2);
        assertFalse(sub.isSubscribed(FEED, bob));
        assertTrue(sub.isSubscribed(FEED, alice));
        assertTrue(sub.isSubscribed(FEED, carol));
        // The dense array still contains exactly alice + carol.
        address[] memory list = sub.subscribersOf(FEED);
        assertEq(list.length, 2);
        bool sawAlice;
        bool sawCarol;
        for (uint256 i; i < list.length; i++) {
            if (list[i] == alice) sawAlice = true;
            if (list[i] == carol) sawCarol = true;
        }
        assertTrue(sawAlice && sawCarol);
    }

    function test_unsubscribe_when_not_subscribed_is_noop() public {
        vm.prank(alice);
        sub.unsubscribe(FEED); // no revert
        assertEq(sub.subscriberCount(FEED), 0);
    }

    function test_feeds_are_isolated() public {
        vm.prank(alice);
        sub.subscribe(FEED);
        vm.prank(alice);
        sub.subscribe(99);
        assertTrue(sub.isSubscribed(FEED, alice));
        assertTrue(sub.isSubscribed(99, alice));
        assertEq(sub.subscriberCount(FEED), 1);
        assertEq(sub.subscriberCount(99), 1);

        vm.prank(alice);
        sub.unsubscribe(FEED);
        assertFalse(sub.isSubscribed(FEED, alice));
        assertTrue(sub.isSubscribed(99, alice), "other feed untouched");
    }

    /// Re-subscribe after unsubscribe re-adds cleanly (index reset to 0 → push).
    function test_resubscribe_after_unsubscribe() public {
        vm.startPrank(alice);
        sub.subscribe(FEED);
        sub.unsubscribe(FEED);
        sub.subscribe(FEED);
        vm.stopPrank();
        assertTrue(sub.isSubscribed(FEED, alice));
        assertEq(sub.subscriberCount(FEED), 1);
    }
}
