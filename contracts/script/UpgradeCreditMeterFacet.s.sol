// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Upgrades CreditMeterFacet in place: deploys the new implementation
/// (which adds `withdrawCredits` — the meter->wallet bridge that makes
/// unspent chat credits spendable on x402 agent calls), REPLACEs the five
/// existing selectors to the new address, and ADDs the new one. Storage
/// (LibCreditMeterStorage) is untouched — ledger balances and the meter
/// key survive the cut.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/UpgradeCreditMeterFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract UpgradeCreditMeterFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        CreditMeterFacet meter = new CreditMeterFacet();

        bytes4[] memory replaced = new bytes4[](5);
        replaced[0] = CreditMeterFacet.depositCredits.selector;
        replaced[1] = CreditMeterFacet.meter.selector;
        replaced[2] = CreditMeterFacet.setMeter.selector;
        replaced[3] = CreditMeterFacet.creditOf.selector;
        replaced[4] = CreditMeterFacet.meterAddress.selector;

        bytes4[] memory added = new bytes4[](1);
        added[0] = CreditMeterFacet.withdrawCredits.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](2);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(meter),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: replaced
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(meter),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: added
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- CreditMeterFacet upgraded (withdrawCredits) ---");
        console.log("diamond:          ", diamond);
        console.log("creditMeterFacet: ", address(meter));
    }
}
