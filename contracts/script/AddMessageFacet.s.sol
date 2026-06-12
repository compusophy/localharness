// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {MessageFacet} from "../src/facets/MessageFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys MessageFacet and cuts the async-inbox surface into the diamond:
/// `sendMessage / inboxCount / inboxLastRead / unreadCount / messageAt /
/// inboxRange / markRead` — drop-a-message-and-read-later, the async
/// counterpart to the synchronous `call_agent`.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddMessageFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddMessageFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        MessageFacet msgf = new MessageFacet();

        bytes4[] memory selectors = new bytes4[](7);
        selectors[0] = MessageFacet.sendMessage.selector;
        selectors[1] = MessageFacet.inboxCount.selector;
        selectors[2] = MessageFacet.inboxLastRead.selector;
        selectors[3] = MessageFacet.unreadCount.selector;
        selectors[4] = MessageFacet.messageAt.selector;
        selectors[5] = MessageFacet.inboxRange.selector;
        selectors[6] = MessageFacet.markRead.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(msgf),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- MessageFacet cut ---");
        console.log("diamond:      ", diamond);
        console.log("messageFacet: ", address(msgf));
    }
}
