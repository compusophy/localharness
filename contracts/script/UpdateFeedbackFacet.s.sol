// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {FeedbackFacet} from "../src/facets/FeedbackFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Upgrades FeedbackFacet so feedback lands in contract STATE (an
/// append-only array) instead of being event-log-only. The new impl still
/// emits `FeedbackSubmitted`, but also pushes an entry that the new view
/// functions page over — so harvesting no longer depends on a 100k-block
/// log window.
///
/// Deploys the new FeedbackFacet and, in ONE `diamondCut`:
///   - REPLACEs `submitFeedback(string)` (new impl writes storage too)
///   - ADDs `feedbackCount() / feedbackAt(uint256) / feedbackRange(uint256,uint256)`
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/UpdateFeedbackFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract UpdateFeedbackFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        FeedbackFacet feedback = new FeedbackFacet();

        // REPLACE the existing submitFeedback selector with the new impl.
        bytes4[] memory replaceSelectors = new bytes4[](1);
        replaceSelectors[0] = FeedbackFacet.submitFeedback.selector;

        // ADD the new read-state view selectors.
        bytes4[] memory addSelectors = new bytes4[](3);
        addSelectors[0] = FeedbackFacet.feedbackCount.selector;
        addSelectors[1] = FeedbackFacet.feedbackAt.selector;
        addSelectors[2] = FeedbackFacet.feedbackRange.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](2);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(feedback),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: replaceSelectors
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(feedback),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: addSelectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- FeedbackFacet upgraded (state-backed) ---");
        console.log("diamond:        ", diamond);
        console.log("feedbackFacet:  ", address(feedback));
        console.log("replaced submitFeedback(string):");
        console.logBytes4(replaceSelectors[0]);
        console.log("added feedbackCount() / feedbackAt(uint256) / feedbackRange(uint256,uint256):");
        console.logBytes4(addSelectors[0]);
        console.logBytes4(addSelectors[1]);
        console.logBytes4(addSelectors[2]);
    }
}
