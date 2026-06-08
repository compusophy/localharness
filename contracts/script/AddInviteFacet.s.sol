// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {InviteFacet} from "../src/facets/InviteFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys InviteFacet and cuts the user-funded invite system into the
/// diamond (design/invites.md). ANY holder escrows their own `$LH` to back
/// a shareable invite code; the escrow pays out to whoever accepts, or
/// refunds the funder 100% after expiry if unclaimed. The GROWTH primitive.
///
/// No post-cut config — the credits token is read from the shared
/// CreditsFacet storage slot (set once via `setCreditsToken`).
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddInviteFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddInviteFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        InviteFacet f = new InviteFacet();

        bytes4[] memory selectors = new bytes4[](5);
        selectors[0] = InviteFacet.createInvite.selector;
        selectors[1] = InviteFacet.acceptInvite.selector;
        selectors[2] = InviteFacet.reclaimInvite.selector;
        selectors[3] = InviteFacet.getInvite.selector;
        selectors[4] = InviteFacet.escrowedOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- InviteFacet cut ---");
        console.log("diamond:      ", diamond);
        console.log("inviteFacet:  ", address(f));
    }
}
