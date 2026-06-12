// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {MessageFacet} from "../src/facets/MessageFacet.sol";

/// Harness: behind the real diamond, `markRead` calls `ownerOf` on the same
/// address (the diamond's ERC721Facet). Standalone, MessageFacet has no
/// `ownerOf`, so this harness supplies a settable one for the gate tests.
contract MessageHarness is MessageFacet {
    mapping(uint256 => address) public owners;

    function setOwner(uint256 id, address who) external {
        owners[id] = who;
    }

    function ownerOf(uint256 id) external view returns (address) {
        return owners[id];
    }
}

contract MessageFacetTest is Test {
    MessageHarness msgf;

    address alice = address(0xA11CE);
    address bob = address(0xB0B);
    uint256 constant INBOX = 42; // bob's identity tokenId
    uint256 constant OTHER = 7;

    event MessageSent(uint256 indexed toId, address indexed from, uint256 index, uint64 timestamp);
    event InboxRead(uint256 indexed toId, uint256 upTo);

    function setUp() public {
        msgf = new MessageHarness();
        msgf.setOwner(INBOX, bob);
    }

    function test_send_appends_and_counts() public {
        vm.expectEmit(true, true, false, true);
        emit MessageSent(INBOX, alice, 0, uint64(block.timestamp));
        vm.prank(alice);
        msgf.sendMessage(INBOX, "gm bob");

        assertEq(msgf.inboxCount(INBOX), 1);
        assertEq(msgf.unreadCount(INBOX), 1);
        (address from,, string memory body) = msgf.messageAt(INBOX, 0);
        assertEq(from, alice);
        assertEq(body, "gm bob");
    }

    function test_empty_and_too_long_revert() public {
        vm.expectRevert(MessageFacet.EmptyMessage.selector);
        msgf.sendMessage(INBOX, "");

        bytes memory big = new bytes(1025);
        vm.expectRevert(abi.encodeWithSelector(MessageFacet.MessageTooLong.selector, uint256(1025)));
        msgf.sendMessage(INBOX, string(big));
    }

    function test_inboxes_are_isolated() public {
        vm.prank(alice);
        msgf.sendMessage(INBOX, "to bob");
        vm.prank(alice);
        msgf.sendMessage(OTHER, "to other");
        assertEq(msgf.inboxCount(INBOX), 1);
        assertEq(msgf.inboxCount(OTHER), 1);
    }

    function test_range_pages_and_clamps() public {
        for (uint256 i = 0; i < 5; i++) {
            vm.prank(alice);
            msgf.sendMessage(INBOX, "m");
        }
        (address[] memory froms,, string[] memory bodies) = msgf.inboxRange(INBOX, 1, 10);
        assertEq(froms.length, 4); // clamped: indices 1..4
        assertEq(bodies.length, 4);

        // start past the end → empty, not a revert
        (address[] memory none,,) = msgf.inboxRange(INBOX, 99, 5);
        assertEq(none.length, 0);
    }

    function test_markRead_only_owner_and_monotonic() public {
        vm.startPrank(alice);
        msgf.sendMessage(INBOX, "a");
        msgf.sendMessage(INBOX, "b");
        vm.stopPrank();
        assertEq(msgf.unreadCount(INBOX), 2);

        // a non-owner can't mark the inbox read
        vm.expectRevert(MessageFacet.NotInboxOwner.selector);
        vm.prank(alice);
        msgf.markRead(INBOX, 2);

        // the owner can
        vm.expectEmit(true, false, false, true);
        emit InboxRead(INBOX, 2);
        vm.prank(bob);
        msgf.markRead(INBOX, 2);
        assertEq(msgf.unreadCount(INBOX), 0);
        assertEq(msgf.inboxLastRead(INBOX), 2);

        // can't rewind the pointer or exceed the inbox length
        vm.prank(bob);
        vm.expectRevert(MessageFacet.BadReadPointer.selector);
        msgf.markRead(INBOX, 1);

        vm.prank(bob);
        vm.expectRevert(MessageFacet.BadReadPointer.selector);
        msgf.markRead(INBOX, 99);
    }

    function test_unread_tracks_new_arrivals_after_read() public {
        vm.prank(alice);
        msgf.sendMessage(INBOX, "first");
        vm.prank(bob);
        msgf.markRead(INBOX, 1);
        assertEq(msgf.unreadCount(INBOX), 0);

        // a new message after catching up shows as unread again
        vm.prank(alice);
        msgf.sendMessage(INBOX, "second");
        assertEq(msgf.unreadCount(INBOX), 1);
    }
}
