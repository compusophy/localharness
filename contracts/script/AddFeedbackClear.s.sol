// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {FeedbackFacet} from "../src/facets/FeedbackFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Adds garbage collection to FeedbackFacet: an owner-only `clearFeedback()` so
/// the otherwise append-only feedback array can be GC'd (on-chain feedback is a
/// TRANSIENT inbox — the durable record lives off-chain after harvest/bridge).
///
/// Redeploys the facet and, in ONE `diamondCut`:
///   - REPLACEs the 4 existing selectors with the new impl (their logic is
///     unchanged; redeploying keeps the whole facet at one address)
///   - ADDs `clearFeedback()`
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddFeedbackClear.s.sol --rpc-url tempo_moderato --broadcast
contract AddFeedbackClear is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        FeedbackFacet feedback = new FeedbackFacet();

        // REPLACE the existing selectors (logic unchanged) so the whole facet
        // lives at the new address.
        bytes4[] memory replaceSelectors = new bytes4[](4);
        replaceSelectors[0] = FeedbackFacet.submitFeedback.selector;
        replaceSelectors[1] = FeedbackFacet.feedbackCount.selector;
        replaceSelectors[2] = FeedbackFacet.feedbackAt.selector;
        replaceSelectors[3] = FeedbackFacet.feedbackRange.selector;

        // ADD the new GC selector.
        bytes4[] memory addSelectors = new bytes4[](1);
        addSelectors[0] = FeedbackFacet.clearFeedback.selector;

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

        console.log("--- FeedbackFacet GC added (clearFeedback) ---");
        console.log("diamond:       ", diamond);
        console.log("feedbackFacet: ", address(feedback));
        console.log("added clearFeedback():");
        console.logBytes4(addSelectors[0]);
    }
}
