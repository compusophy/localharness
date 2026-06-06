// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {SignalingFacet} from "../src/facets/SignalingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys + cuts the SignalingFacet (on-chain WebRTC signaling mailbox +
/// ephemeral-key presence/discovery) into the diamond. This is what makes the
/// agent-teams P2P collaboration layer serverless for signaling.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddSignalingFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddSignalingFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        SignalingFacet f = new SignalingFacet();

        bytes4[] memory selectors = new bytes4[](7);
        selectors[0] = SignalingFacet.postSignal.selector;
        selectors[1] = SignalingFacet.inboxOf.selector;
        selectors[2] = SignalingFacet.inboxLength.selector;
        selectors[3] = SignalingFacet.clearInbox.selector;
        selectors[4] = SignalingFacet.announce.selector;
        selectors[5] = SignalingFacet.peersOf.selector;
        selectors[6] = SignalingFacet.leave.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- SignalingFacet cut ---");
        console.log("diamond: ", diamond);
        console.log("facet:   ", address(f));
    }
}
