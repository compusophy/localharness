// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the message facet — the async agent INBOX. Diamond
///      storage pattern: a fresh slot that can't collide with any other facet's
///      storage already cut into the diamond. Add new fields ONLY at the end.
///
///      Unlike feedback (one global log), messages are keyed by RECIPIENT token
///      id, so each agent/identity has its OWN append-only inbox other agents
///      drop into. The recipient reads at their own pace; a per-id `lastRead`
///      pointer (advanced only by the recipient) backs an unread count.
library LibMessageStorage {
    bytes32 constant POSITION = keccak256("localharness.message.storage.v1");

    struct Message {
        /// The sender (`msg.sender` at send time) — authoritative; resolve to a
        /// name off-chain via `mainNameOf`. Render the body as plain text, never HTML.
        address from;
        /// `block.timestamp` of the send (seconds; fits uint64).
        uint64 timestamp;
        /// The message body.
        string body;
    }

    struct Storage {
        /// recipient tokenId => its append-only inbox, oldest first.
        mapping(uint256 => Message[]) inboxes;
        /// recipient tokenId => count of messages the owner has marked read
        /// (a high-water index; unread = inbox length - lastRead).
        mapping(uint256 => uint256) lastRead;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
