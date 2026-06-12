// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {SubscribeFacet} from "../src/facets/SubscribeFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys SubscribeFacet and cuts `subscribe / unsubscribe / isSubscribed /
/// subscriberCount / subscribersOf` into the diamond — the on-chain
/// subscriber set behind the cartridge "Ready Up" feed.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddSubscribeFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddSubscribeFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        SubscribeFacet sub = new SubscribeFacet();

        bytes4[] memory selectors = new bytes4[](5);
        selectors[0] = SubscribeFacet.subscribe.selector;
        selectors[1] = SubscribeFacet.unsubscribe.selector;
        selectors[2] = SubscribeFacet.isSubscribed.selector;
        selectors[3] = SubscribeFacet.subscriberCount.selector;
        selectors[4] = SubscribeFacet.subscribersOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(sub),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- SubscribeFacet cut ---");
        console.log("diamond:        ", diamond);
        console.log("subscribeFacet: ", address(sub));
    }
}
