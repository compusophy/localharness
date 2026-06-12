// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the push facet — Web Push subscriptions keyed by
///      the SUBSCRIBER'S ADDRESS, not a tokenId. Diamond storage pattern: a
///      fresh slot that can't collide with any other facet. Add fields ONLY at
///      the end.
///
///      WHY ADDRESS-KEYED: a device that merely subscribed to a feed (or wants
///      to receive notifications) often has NO registered MAIN identity — a bare
///      device key with `mainOf == 0`. The old design hung the push subscription
///      off the owner's MAIN tokenId, so such a device had nowhere to store it
///      and could never be reached. Keying by `msg.sender` lets ANY device
///      register + receive, no name/MAIN required.
library LibPushStorage {
    bytes32 constant POSITION = keccak256("localharness.push.storage.v1");

    struct Storage {
        /// subscriber address => Web Push subscription JSON ({endpoint, keys}).
        mapping(address => bytes) sub;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
