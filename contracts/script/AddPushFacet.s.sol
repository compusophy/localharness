// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {PushFacet} from "../src/facets/PushFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys PushFacet and cuts `setPushSub / clearPushSub / pushSubOf /
/// hasPushSub` — address-keyed Web Push subscriptions so a device with no MAIN
/// identity can still register + receive cross-device notifications.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddPushFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddPushFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        PushFacet push = new PushFacet();

        bytes4[] memory selectors = new bytes4[](4);
        selectors[0] = PushFacet.setPushSub.selector;
        selectors[1] = PushFacet.clearPushSub.selector;
        selectors[2] = PushFacet.pushSubOf.selector;
        selectors[3] = PushFacet.hasPushSub.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(push),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- PushFacet cut ---");
        console.log("diamond:   ", diamond);
        console.log("pushFacet: ", address(push));
    }
}
