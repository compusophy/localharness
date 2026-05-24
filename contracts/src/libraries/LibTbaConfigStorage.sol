// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Token-bound-account configuration for the TbaFacet — the
///      ERC-6551 registry address + reference account implementation
///      we point our tokenBoundAccount() helper at. Isolated storage
///      slot so future facets can read/write without conflict.
library LibTbaConfigStorage {
    bytes32 constant POSITION = keccak256("localharness.tba.config.storage.v1");

    struct Storage {
        address registry;
        address accountImpl;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
