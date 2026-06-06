// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the owner -> tokenIds index. Diamond storage
///      pattern — fresh slot, never collides with other facets. Add new
///      fields ONLY at the end of the struct.
library LibOwnedTokensStorage {
    bytes32 constant POSITION = keccak256("localharness.ownedtokens.storage.v1");

    struct Storage {
        /// owner address => the tokenIds they currently hold.
        mapping(address => uint256[]) owned;
        /// tokenId => (index + 1) into its owner's `owned` array; 0 = absent.
        /// Enables an O(1) presence check + swap-pop removal.
        mapping(uint256 => uint256) indexOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
