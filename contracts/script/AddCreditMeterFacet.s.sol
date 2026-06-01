// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys CreditMeterFacet and cuts `depositCredits / meter / setMeter /
/// creditOf / meterAddress` into the diamond at $DIAMOND.
///
/// After cutting, set the proxy's metering key as the authorized meter:
///   cast send $DIAMOND "setMeter(address)" 0x<proxyMeterAddr> ...
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddCreditMeterFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddCreditMeterFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        CreditMeterFacet meter = new CreditMeterFacet();

        bytes4[] memory selectors = new bytes4[](5);
        selectors[0] = CreditMeterFacet.depositCredits.selector;
        selectors[1] = CreditMeterFacet.meter.selector;
        selectors[2] = CreditMeterFacet.setMeter.selector;
        selectors[3] = CreditMeterFacet.creditOf.selector;
        selectors[4] = CreditMeterFacet.meterAddress.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(meter),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- CreditMeterFacet cut ---");
        console.log("diamond:          ", diamond);
        console.log("creditMeterFacet: ", address(meter));
    }
}
