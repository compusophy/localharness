// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the `registerMain` cost-gate. Same
///      pattern as `LibRegistrationCostStorage` but for the MAIN
///      identity facet — a separate slot so adding the knob doesn't
///      touch the existing `LibMainIdentityStorage` layout.
///
///      Default zero (gate off) on a fresh diamond. Owner sets via
///      `MainIdentityFacet.setMainCost`; non-zero values turn MAIN
///      registration into a paid action (sybil deterrent).
///
///      Add new fields ONLY at the end of the struct.
library LibMainCostStorage {
    bytes32 constant POSITION = keccak256("localharness.main_cost.storage.v1");

    struct Storage {
        /// LH charged per `registerMain` call, in 18-decimal token
        /// wei. Zero disables the cost gate entirely. Owner-tunable
        /// via `setMainCost` on `MainIdentityFacet`.
        uint256 costWei;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
