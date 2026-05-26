// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the `register(name)` cost-gate. Fresh
///      slot — kept separate from `LibRegistryStorage` so the cost
///      knob can be added without touching the registry's existing
///      struct layout (no field-append risk).
///
///      Owner sets the cost via `LocalharnessRegistryFacet.setRegistrationCost`.
///      `register(name)` reads `costWei` and, when non-zero, calls
///      `ILocalharnessCredits(creditsToken).transferFrom(msg.sender,
///      address(this), costWei)` — pulling the credit pool into the
///      diamond's own balance (treasury). User must have approved the
///      diamond for the cost beforehand (typically batched into the
///      same Tempo tx as register).
///
///      Add new fields ONLY at the end of the struct.
library LibRegistrationCostStorage {
    bytes32 constant POSITION = keccak256("localharness.registration_cost.storage.v1");

    struct Storage {
        /// Credits charged per `register(name)`, in 18-decimal token
        /// wei. Zero disables the cost gate entirely (registration
        /// stays free — useful as a kill switch). Default 0 until
        /// owner sets it.
        uint256 costWei;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
