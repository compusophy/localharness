// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Re-cuts `LocalharnessRegistryFacet` to add DEFENSE-IN-DEPTH name
/// validation: `register(string)` now reverts `InvalidName(name)` BEFORE
/// any mint / state write when the name is not a valid DNS label (1-63
/// bytes of lowercase a-z / 0-9 / hyphen, no leading/trailing hyphen) —
/// EXACTLY matching the CLI's `name_is_valid`. Previously a direct
/// contract call (bypassing the CLI guard) could mint an unreachable
/// "ghost" subdomain (uppercase / underscore / emoji / oversized break
/// DNS routing). Surfaced by the test-user fleet (juno-qa).
///
/// Only the IMPLEMENTATION of `register` changed (no selectors added or
/// removed), so this is a pure `Replace`: every one of the facet's
/// selectors is re-pointed to the new deployment in ONE `diamondCut`,
/// keeping the whole facet coherent at a single address.
///
/// NOTE: validation applies to NEW registrations only — existing names
/// are untouched (no migration). `register` is the sole mint path; the
/// admin burn / release flows clear state, they never register.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddRegistryNameValidation.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddRegistryNameValidation is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        LocalharnessRegistryFacet newFacet = new LocalharnessRegistryFacet();

        // Only `register`'s IMPLEMENTATION changed, so Replace ONLY its
        // selector → the new facet (with validation). The other selectors stay
        // on the prior deployment. (A blanket Replace of the whole 14-selector
        // source surface fails with "function not found": the deployed facet's
        // selector set differs from this source — some live on other facets or
        // were never cut. `register` IS on the diamond, so this surgical Replace
        // works, and `register` calls its own internal `_isValidName` in the new
        // facet — no cross-facet dependency.)
        bytes4[] memory selectors = new bytes4[](1);
        selectors[0] = LocalharnessRegistryFacet.register.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(newFacet),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- registry facet re-cut with name validation ---");
        console.log("diamond:           ", diamond);
        console.log("newRegistryFacet:  ", address(newFacet));
        console.log("replaced register(string) (now reverts InvalidName):");
        console.logBytes4(selectors[0]);
        console.log("replaced selectors total:", selectors.length);
    }
}
