// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the per-request credit meter. Diamond
///      storage pattern — fresh slot, no collision with any other
///      facet. Add new fields ONLY at the end.
library LibCreditMeterStorage {
    bytes32 constant POSITION = keccak256("localharness.creditmeter.storage.v1");

    struct Storage {
        /// Per-user prepaid `$LH` credit balance (18-decimal wei) the
        /// proxy debits per request via `meter`.
        mapping(address => uint256) creditOf;
        /// The single address allowed to call `meter` — the credit
        /// proxy's metering key. Owner-set.
        address meter;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
