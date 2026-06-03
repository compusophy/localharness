// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the feedback facet. Diamond storage
///      pattern — fresh slot, no collision with registry / TBA /
///      redeem / main-identity / credits / session / device storage
///      already cut into the diamond. Add new fields ONLY at the end.
///
///      Feedback is now mirrored into contract STATE (an append-only
///      array) so it can be read via view functions, rather than only
///      scraped from event logs (Tempo caps `eth_getLogs` to a 100k-block
///      window, which loses older submissions).
library LibFeedbackStorage {
    bytes32 constant POSITION = keccak256("localharness.feedback.storage.v1");

    struct Entry {
        /// The submitter (`msg.sender` at submit time).
        address sender;
        /// `block.timestamp` of the submission (seconds; fits uint64).
        uint64 timestamp;
        /// The raw feedback text. Render as plain text only, NEVER HTML.
        string text;
    }

    struct Storage {
        /// Append-only log of every submission, oldest first.
        Entry[] entries;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
