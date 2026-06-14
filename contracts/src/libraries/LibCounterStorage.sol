// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for CounterFacet. Diamond storage pattern — a fresh,
///      keccak-namespaced base slot so the facet never collides with any other
///      facet's state when DELEGATECALL'd from the diamond. Add new fields ONLY
///      at the end of the struct. This is the deploy/cut target for SolidityLite
///      Installment 0 (design/soliditylite.md §3); the layout the future
///      compiler auto-synthesizes by hand here.
library LibCounterStorage {
    bytes32 constant POSITION = keccak256("localharness.counterfacet.storage.v1");

    struct Storage {
        /// per-caller increment count
        mapping(address => uint256) count;
        /// running sum of every increment across all callers
        uint256 total;
    }

    function s() internal pure returns (Storage storage st) {
        bytes32 position = POSITION;
        assembly {
            st.slot := position
        }
    }
}
