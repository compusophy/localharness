// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {RedeemFacet} from "../src/facets/RedeemFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys RedeemFacet and cuts `addRedeemCodes / redeem /
/// redeemAmountOf / isRedeemed` into the diamond at $DIAMOND.
///
/// Prereq: the diamond must already hold ISSUER_ROLE on the credits
/// token and have it configured via `setCreditsToken` (CreditsFacet) —
/// redeem mints `$LH` through that role.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddRedeemFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddRedeemFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        RedeemFacet redeem = new RedeemFacet();

        bytes4[] memory selectors = new bytes4[](5);
        selectors[0] = RedeemFacet.addRedeemCodes.selector;
        selectors[1] = RedeemFacet.disableRedeemCodes.selector;
        selectors[2] = RedeemFacet.redeem.selector;
        selectors[3] = RedeemFacet.redeemAmountOf.selector;
        selectors[4] = RedeemFacet.isRedeemed.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(redeem),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- RedeemFacet cut ---");
        console.log("diamond:      ", diamond);
        console.log("redeemFacet:  ", address(redeem));
    }
}
