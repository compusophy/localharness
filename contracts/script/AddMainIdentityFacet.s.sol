// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {MainIdentityFacet} from "../src/facets/MainIdentityFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys MainIdentityFacet and cuts its five selectors into the
/// diamond at $DIAMOND.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddMainIdentityFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddMainIdentityFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        MainIdentityFacet main = new MainIdentityFacet();

        bytes4[] memory selectors = new bytes4[](5);
        selectors[0] = MainIdentityFacet.registerMain.selector;
        selectors[1] = MainIdentityFacet.clearMain.selector;
        selectors[2] = MainIdentityFacet.mainOf.selector;
        selectors[3] = MainIdentityFacet.mainNameOf.selector;
        selectors[4] = MainIdentityFacet.isMain.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(main),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- MainIdentityFacet cut ---");
        console.log("diamond:    ", diamond);
        console.log("mainFacet:  ", address(main));
    }
}
