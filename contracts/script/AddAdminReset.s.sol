// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ReleaseFacet} from "../src/facets/ReleaseFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Redeploys ReleaseFacet (now carrying the diamond-owner admin reset) and
/// cuts it into the diamond at $DIAMOND. `adminBurnNames(uint256[])` and
/// `adminResetAll()` are Added; the existing `releaseName(uint256)`
/// selector is Replaced so it points at the same redeployed facet.
///
/// adminBurnNames / adminResetAll are EIP-173 owner-only (LibDiamond
/// enforceIsContractOwner): force-burn names regardless of holder to wipe
/// the registry to a clean slate on testnet.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddAdminReset.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddAdminReset is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ReleaseFacet rel = new ReleaseFacet();

        // New admin selectors — Added.
        bytes4[] memory addSelectors = new bytes4[](2);
        addSelectors[0] = ReleaseFacet.adminBurnNames.selector;
        addSelectors[1] = ReleaseFacet.adminResetAll.selector;

        // Existing selector — Replaced onto the redeployed facet.
        bytes4[] memory replaceSelectors = new bytes4[](1);
        replaceSelectors[0] = ReleaseFacet.releaseName.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](2);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(rel),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: addSelectors
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(rel),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: replaceSelectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- AdminReset (ReleaseFacet) cut ---");
        console.log("diamond:       ", diamond);
        console.log("releaseFacet:  ", address(rel));
    }
}
