// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Wallet-primary billing, phase 1: ADD `CreditMeterFacet.chargeFromWallet` to
/// the diamond. MINIMAL additive cut — deploys a fresh CreditMeterFacet and
/// wires ONLY the new selector; every existing CreditMeter selector keeps its
/// current facet address, so no live behavior changes. Storage
/// (LibCreditMeterStorage) is shared by slot, so the new function sees the same
/// ledger + meter key. `chargeFromWallet` stays UNUSED until the proxy switches
/// its default-billing path to it (phase 2), so this cut cannot alter any
/// in-flight billing.
///
///   DIAMOND=0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddChargeFromWallet.s.sol --rpc-url tempo_mainnet --broadcast
contract AddChargeFromWallet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        CreditMeterFacet meter = new CreditMeterFacet();

        bytes4[] memory added = new bytes4[](1);
        added[0] = CreditMeterFacet.chargeFromWallet.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(meter),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: added
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- chargeFromWallet added ---");
        console.log("diamond:          ", diamond);
        console.log("creditMeterFacet: ", address(meter));
    }
}
