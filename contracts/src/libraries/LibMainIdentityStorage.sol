// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the MAIN-identity facet.
///      Diamond storage pattern — fresh slot, no collisions with the
///      registry/TBA/feedback storage already cut into the diamond.
///      Add new fields ONLY at the end of the struct.
library LibMainIdentityStorage {
    bytes32 constant POSITION = keccak256("localharness.main_identity.storage.v1");

    struct Storage {
        // address (NFT holder) -> tokenId they've declared as MAIN.
        // 0 means "no MAIN registered yet for this address".
        mapping(address => uint256) mainOf;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
