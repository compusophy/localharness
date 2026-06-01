// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the device-link index. Diamond storage
///      pattern — fresh slot. Add new fields ONLY at the end.
library LibDeviceRegistryStorage {
    bytes32 constant POSITION = keccak256("localharness.deviceregistry.storage.v1");

    struct Storage {
        /// MAIN tokenId => the identity's linked device addresses.
        mapping(uint256 => address[]) devices;
        /// MAIN tokenId => device => (index + 1) into `devices`, 0 = absent.
        /// Enables O(1) dedupe + swap-pop removal.
        mapping(uint256 => mapping(address => uint256)) slot;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
