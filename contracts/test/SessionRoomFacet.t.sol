// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {SessionRoomFacet} from "../src/facets/SessionRoomFacet.sol";
import {LibSessionRoomStorage} from "../src/libraries/LibSessionRoomStorage.sol";

contract SessionRoomFacetTest is Test {
    SessionRoomFacet room;

    address alice = address(0xA11CE);
    address bob = address(0xB0B);
    address mallory = address(0x4A11);

    function setUp() public {
        room = new SessionRoomFacet();
    }

    function _create() internal returns (uint256 id) {
        vm.prank(alice);
        id = room.createRoom();
    }

    function test_create_sets_creator_and_member() public {
        uint256 id = _create();
        assertEq(id, 1);
        assertEq(room.roomCreator(id), alice);
        assertTrue(room.roomIsMember(id, alice));
        assertEq(room.roomMembersOf(id).length, 1);
        // ids are monotonic.
        vm.prank(bob);
        assertEq(room.createRoom(), 2);
    }

    function test_member_can_append_nonmember_cannot() public {
        uint256 id = _create();
        vm.prank(alice);
        uint256 idx = room.appendOp(id, "op0");
        assertEq(idx, 0);
        assertEq(room.opCount(id), 1);

        // bob is not a member yet.
        vm.expectRevert(SessionRoomFacet.NotRoomMember.selector);
        vm.prank(bob);
        room.appendOp(id, "nope");

        // creator enrolls bob → he can write.
        vm.prank(alice);
        room.roomAddMember(id, bob);
        vm.prank(bob);
        assertEq(room.appendOp(id, "op1"), 1);
        assertEq(room.opCount(id), 2);
    }

    function test_only_creator_manages_members() public {
        uint256 id = _create();
        vm.expectRevert(SessionRoomFacet.NotRoomCreator.selector);
        vm.prank(mallory);
        room.roomAddMember(id, mallory);
    }

    function test_remove_member_revokes_writes_and_protects_creator() public {
        uint256 id = _create();
        vm.prank(alice);
        room.roomAddMember(id, bob);
        vm.prank(alice);
        room.roomRemoveMember(id, bob);
        assertFalse(room.roomIsMember(id, bob));
        vm.expectRevert(SessionRoomFacet.NotRoomMember.selector);
        vm.prank(bob);
        room.appendOp(id, "after-revoke");

        // the creator can never be removed.
        vm.expectRevert(SessionRoomFacet.NotRoomCreator.selector);
        vm.prank(alice);
        room.roomRemoveMember(id, alice);
    }

    function test_empty_and_oversized_blob_revert() public {
        uint256 id = _create();
        vm.prank(alice);
        vm.expectRevert(SessionRoomFacet.EmptyBlob.selector);
        room.appendOp(id, "");

        bytes memory big = new bytes(2049);
        vm.prank(alice);
        vm.expectRevert(abi.encodeWithSelector(SessionRoomFacet.BlobTooLarge.selector, uint256(2049)));
        room.appendOp(id, big);
    }

    function test_append_to_missing_room_reverts() public {
        vm.expectRevert(SessionRoomFacet.NoSuchRoom.selector);
        vm.prank(alice);
        room.appendOp(999, "x");
    }

    function test_opsOf_paging() public {
        uint256 id = _create();
        vm.startPrank(alice);
        room.appendOp(id, "a");
        room.appendOp(id, "b");
        room.appendOp(id, "c");
        vm.stopPrank();

        LibSessionRoomStorage.Op[] memory all = room.opsOf(id, 0);
        assertEq(all.length, 3);
        assertEq(all[0].writer, alice);
        assertEq(string(all[1].blob), "b");

        LibSessionRoomStorage.Op[] memory tail = room.opsOf(id, 2);
        assertEq(tail.length, 1);
        assertEq(string(tail[0].blob), "c");

        // out-of-range cursor → empty, not a revert.
        assertEq(room.opsOf(id, 5).length, 0);
    }

    function test_clear_bumps_epoch_and_creator_only() public {
        uint256 id = _create();
        vm.prank(alice);
        room.appendOp(id, "a");
        assertEq(room.roomEpoch(id), 0);

        vm.expectRevert(SessionRoomFacet.NotRoomCreator.selector);
        vm.prank(mallory);
        room.clearRoom(id);

        vm.prank(alice);
        uint64 ep = room.clearRoom(id);
        assertEq(ep, 1);
        assertEq(room.roomEpoch(id), 1);
        assertEq(room.opCount(id), 0);
        // log restarts from index 0 after a clear.
        vm.prank(alice);
        assertEq(room.appendOp(id, "fresh"), 0);
    }
}
