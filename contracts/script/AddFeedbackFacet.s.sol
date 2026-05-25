// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {FeedbackFacet} from "../src/facets/FeedbackFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys FeedbackFacet and cuts `submitFeedback(string)` into the
/// diamond at $DIAMOND.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddFeedbackFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddFeedbackFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        FeedbackFacet feedback = new FeedbackFacet();

        bytes4[] memory selectors = new bytes4[](1);
        selectors[0] = FeedbackFacet.submitFeedback.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(feedback),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- FeedbackFacet cut ---");
        console.log("diamond:        ", diamond);
        console.log("feedbackFacet:  ", address(feedback));
        console.log("selector:       ");
        console.logBytes4(selectors[0]);
    }
}
