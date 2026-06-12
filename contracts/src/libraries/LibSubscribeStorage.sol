// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for SubscribeFacet — a per-subdomain notification
///      subscriber set (the "Ready Up" feed). Fresh diamond-storage slot,
///      no collision with any cut facet. Append-only: add fields ONLY at
///      the end (diamond layout is positional + immutable).
library LibSubscribeStorage {
    bytes32 constant POSITION = keccak256("localharness.subscribe.storage.v1");

    struct Storage {
        // targetId (the subscribed-to subdomain's NFT id) => subscriber list.
        mapping(uint256 => address[]) subscribers;
        // targetId => subscriber => 1-based index into `subscribers[targetId]`
        // (0 == not subscribed). Powers O(1) membership + swap-remove.
        mapping(uint256 => mapping(address => uint256)) indexOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 p = POSITION;
        assembly {
            s.slot := p
        }
    }
}
