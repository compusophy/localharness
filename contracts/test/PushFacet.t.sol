// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {PushFacet} from "../src/facets/PushFacet.sol";

contract PushFacetTest is Test {
    PushFacet push;
    address alice = address(0xA11CE);
    address bob = address(0xB0B);

    function setUp() public {
        push = new PushFacet();
    }

    function test_set_and_read_self_keyed() public {
        bytes memory sub = bytes('{"endpoint":"https://x","keys":{}}');
        vm.prank(alice);
        push.setPushSub(sub);

        assertTrue(push.hasPushSub(alice));
        assertEq(push.pushSubOf(alice), sub);
        // bob — who never registered — is empty, no MAIN required either way
        assertFalse(push.hasPushSub(bob));
        assertEq(push.pushSubOf(bob).length, 0);
    }

    function test_replace_and_clear() public {
        vm.startPrank(alice);
        push.setPushSub(bytes("first"));
        push.setPushSub(bytes("second"));
        assertEq(push.pushSubOf(alice), bytes("second"));
        push.clearPushSub();
        vm.stopPrank();
        assertFalse(push.hasPushSub(alice));
    }

    function test_empty_and_too_long_revert() public {
        vm.expectRevert(PushFacet.EmptyPushSub.selector);
        push.setPushSub("");

        bytes memory big = new bytes(4097);
        vm.expectRevert(abi.encodeWithSelector(PushFacet.PushSubTooLong.selector, uint256(4097)));
        push.setPushSub(big);
    }
}
