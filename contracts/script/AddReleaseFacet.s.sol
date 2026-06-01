// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ReleaseFacet} from "../src/facets/ReleaseFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys ReleaseFacet and cuts `releaseName(uint256)` into the diamond
/// at $DIAMOND — the recycle/release half of the subdomain lifecycle.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddReleaseFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddReleaseFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ReleaseFacet rel = new ReleaseFacet();

        bytes4[] memory selectors = new bytes4[](1);
        selectors[0] = ReleaseFacet.releaseName.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(rel),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ReleaseFacet cut ---");
        console.log("diamond:       ", diamond);
        console.log("releaseFacet:  ", address(rel));
    }
}
