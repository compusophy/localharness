// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {SessionFacet} from "../src/facets/SessionFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys SessionFacet and cuts `openSession / setSessionPrice /
/// setSessionDuration / sessionExpiryOf / sessionPrice /
/// sessionDuration` into the diamond at $DIAMOND.
///
/// After cutting, set the knobs (owner-only), e.g. a 1-hour session
/// costing 1 $LH:
///   cast send $DIAMOND "setSessionDuration(uint256)" 3600 ...
///   cast send $DIAMOND "setSessionPrice(uint256)" 1000000000000000000 ...
///
/// The Vercel Edge credit proxy reads `sessionExpiryOf(address)` on
/// every request.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddSessionFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddSessionFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        SessionFacet session = new SessionFacet();

        bytes4[] memory selectors = new bytes4[](6);
        selectors[0] = SessionFacet.openSession.selector;
        selectors[1] = SessionFacet.setSessionPrice.selector;
        selectors[2] = SessionFacet.setSessionDuration.selector;
        selectors[3] = SessionFacet.sessionExpiryOf.selector;
        selectors[4] = SessionFacet.sessionPrice.selector;
        selectors[5] = SessionFacet.sessionDuration.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(session),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- SessionFacet cut ---");
        console.log("diamond:       ", diamond);
        console.log("sessionFacet:  ", address(session));
    }
}
