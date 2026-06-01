// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for x402 payment settlement. Diamond storage
///      pattern — fresh slot. Add new fields ONLY at the end.
library LibX402Storage {
    bytes32 constant POSITION = keccak256("localharness.x402.storage.v1");

    struct Storage {
        /// payer => authorization nonce => consumed. One-shot replay
        /// guard per (from, nonce), mirroring EIP-3009's authorization
        /// state.
        mapping(address => mapping(bytes32 => bool)) authState;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
