// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibMessageStorage} from "../libraries/LibMessageStorage.sol";

/// @dev Minimal view into the diamond's own ERC-721 surface, used to gate
///      `markRead` to the inbox owner (the recipient identity's holder).
interface IOwnerOf {
    function ownerOf(uint256 tokenId) external view returns (address);
}

/// @title MessageFacet
/// @notice The async agent INBOX — the counterpart to the synchronous
///         `call_agent` (call-and-response). Any identity can DROP a message in
///         another identity's inbox (keyed by recipient tokenId); the recipient
///         reads it at their own pace via the paged views, and advances their
///         own `lastRead` pointer so an unread count is cheap to show.
///
///         Permissionless (gas is the spam filter, like feedback); the body is
///         bounded so one send can't blow gas. Append-only — messages can't be
///         edited or deleted by anyone (the inbox is an immutable record); only
///         the recipient can move their read pointer.
contract MessageFacet {
    event MessageSent(
        uint256 indexed toId,
        address indexed from,
        uint256 index,
        uint64 timestamp
    );
    event InboxRead(uint256 indexed toId, uint256 upTo);

    error EmptyMessage();
    error MessageTooLong(uint256 length);
    error NotInboxOwner();
    error BadReadPointer();

    /// Send `body` to recipient identity `toId`'s inbox. Permissionless; the
    /// sender is recorded as `msg.sender`. Sending to an id that holds nothing
    /// just costs the sender gas — harmless, so no existence check.
    function sendMessage(uint256 toId, string calldata body) external {
        uint256 len = bytes(body).length;
        if (len == 0) revert EmptyMessage();
        if (len > 1024) revert MessageTooLong(len);

        LibMessageStorage.Message[] storage inbox =
            LibMessageStorage.load().inboxes[toId];
        inbox.push(
            LibMessageStorage.Message({
                from: msg.sender,
                timestamp: uint64(block.timestamp),
                body: body
            })
        );

        emit MessageSent(toId, msg.sender, inbox.length - 1, uint64(block.timestamp));
    }

    /// Total messages ever delivered to `toId`'s inbox (read + unread).
    function inboxCount(uint256 toId) external view returns (uint256) {
        return LibMessageStorage.load().inboxes[toId].length;
    }

    /// How many messages `toId` has marked read (the high-water pointer).
    function inboxLastRead(uint256 toId) external view returns (uint256) {
        return LibMessageStorage.load().lastRead[toId];
    }

    /// Unread count = total - lastRead (clamped at 0). The badge for "you have
    /// N new messages".
    function unreadCount(uint256 toId) external view returns (uint256) {
        LibMessageStorage.Storage storage s = LibMessageStorage.load();
        uint256 total = s.inboxes[toId].length;
        uint256 read = s.lastRead[toId];
        return total > read ? total - read : 0;
    }

    /// Read one message by index (0-based, oldest first). Reverts out of range.
    function messageAt(uint256 toId, uint256 i)
        external
        view
        returns (address from, uint64 timestamp, string memory body)
    {
        LibMessageStorage.Message storage m =
            LibMessageStorage.load().inboxes[toId][i];
        return (m.from, m.timestamp, m.body);
    }

    /// Page over an inbox: up to `count` messages from `start` (clamped to the
    /// inbox length). Three parallel arrays keep the ABI flat for `cast`.
    function inboxRange(uint256 toId, uint256 start, uint256 count)
        external
        view
        returns (
            address[] memory froms,
            uint64[] memory timestamps,
            string[] memory bodies
        )
    {
        LibMessageStorage.Message[] storage inbox =
            LibMessageStorage.load().inboxes[toId];
        uint256 total = inbox.length;

        if (start >= total) {
            return (new address[](0), new uint64[](0), new string[](0));
        }

        uint256 end = start + count;
        if (end > total) {
            end = total;
        }
        uint256 n = end - start;

        froms = new address[](n);
        timestamps = new uint64[](n);
        bodies = new string[](n);

        for (uint256 k = 0; k < n; k++) {
            LibMessageStorage.Message storage m = inbox[start + k];
            froms[k] = m.from;
            timestamps[k] = m.timestamp;
            bodies[k] = m.body;
        }
    }

    /// Advance the recipient's read pointer to `upTo` (an index, typically the
    /// current inbox length once they've caught up). ONLY the holder of `toId`
    /// may mark their own inbox read; the pointer is monotonic (can't rewind)
    /// and can't exceed the inbox length.
    function markRead(uint256 toId, uint256 upTo) external {
        if (IOwnerOf(address(this)).ownerOf(toId) != msg.sender) {
            revert NotInboxOwner();
        }
        LibMessageStorage.Storage storage s = LibMessageStorage.load();
        if (upTo > s.inboxes[toId].length) revert BadReadPointer();
        if (upTo < s.lastRead[toId]) revert BadReadPointer();
        s.lastRead[toId] = upTo;
        emit InboxRead(toId, upTo);
    }
}
