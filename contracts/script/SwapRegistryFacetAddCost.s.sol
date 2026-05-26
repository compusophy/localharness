// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Re-cuts `LocalharnessRegistryFacet` on the diamond to swap in the
/// cost-gated `register()` plus the two new selectors
/// `setRegistrationCost(uint256)` / `registrationCost()`. Atomic:
/// Remove (facetAddress=0) on the 10 existing selectors, Add on the
/// new facet with all 12, then call `setRegistrationCost` to seed
/// the initial cost.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   INITIAL_REGISTRATION_COST_WEI=50000000000000000000  # 50 LH
///   forge script script/SwapRegistryFacetAddCost.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract SwapRegistryFacetAddCost is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");
        // Default to 50 LH = half a daily allowance.
        uint256 initialCost = vm.envOr(
            "INITIAL_REGISTRATION_COST_WEI",
            uint256(50 ether)
        );

        vm.startBroadcast(pk);

        LocalharnessRegistryFacet newFacet = new LocalharnessRegistryFacet();

        // Existing selectors (10). Remove them in one cut entry.
        bytes4[] memory oldSelectors = new bytes4[](10);
        oldSelectors[0] = bytes4(keccak256("register(string)"));
        oldSelectors[1] = bytes4(keccak256("setMetadata(uint256,bytes32,bytes)"));
        oldSelectors[2] = bytes4(keccak256("isTaken(string)"));
        oldSelectors[3] = bytes4(keccak256("ownerOfName(string)"));
        oldSelectors[4] = bytes4(keccak256("ownerOfId(uint256)"));
        oldSelectors[5] = bytes4(keccak256("idOfName(string)"));
        oldSelectors[6] = bytes4(keccak256("nameOfId(uint256)"));
        oldSelectors[7] = bytes4(keccak256("idOf(address)"));
        oldSelectors[8] = bytes4(keccak256("nextId()"));
        oldSelectors[9] = bytes4(keccak256("metadata(uint256,bytes32)"));

        // New facet's full surface (12 — old 10 + setRegistrationCost +
        // registrationCost).
        bytes4[] memory newSelectors = new bytes4[](12);
        newSelectors[0] = LocalharnessRegistryFacet.register.selector;
        newSelectors[1] = LocalharnessRegistryFacet.setMetadata.selector;
        newSelectors[2] = LocalharnessRegistryFacet.isTaken.selector;
        newSelectors[3] = LocalharnessRegistryFacet.ownerOfName.selector;
        newSelectors[4] = LocalharnessRegistryFacet.ownerOfId.selector;
        newSelectors[5] = LocalharnessRegistryFacet.idOfName.selector;
        newSelectors[6] = LocalharnessRegistryFacet.nameOfId.selector;
        newSelectors[7] = LocalharnessRegistryFacet.idOf.selector;
        newSelectors[8] = LocalharnessRegistryFacet.nextId.selector;
        newSelectors[9] = LocalharnessRegistryFacet.metadata.selector;
        newSelectors[10] = LocalharnessRegistryFacet.setRegistrationCost.selector;
        newSelectors[11] = LocalharnessRegistryFacet.registrationCost.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](2);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(0),
            action: IDiamond.FacetCutAction.Remove,
            functionSelectors: oldSelectors
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(newFacet),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: newSelectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        // Seed the initial cost. Owner-only — broadcaster is the diamond
        // owner.
        LocalharnessRegistryFacet(diamond).setRegistrationCost(initialCost);

        vm.stopBroadcast();

        console.log("--- registry facet re-cut with cost gate ---");
        console.log("diamond:           ", diamond);
        console.log("newRegistryFacet:  ", address(newFacet));
        console.log("registrationCost: ", initialCost);
        console.log("  (in LH):        ", initialCost / 1 ether);
    }
}
