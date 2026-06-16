// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {MintGateFacet} from "../src/facets/MintGateFacet.sol";
import {CreditMeterFacet} from "../src/facets/CreditMeterFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Cuts the fiat on-ramp mint gate into the diamond at $DIAMOND and upgrades
/// CreditMeterFacet to the lock-aware version (withdraw/meter honour
/// `fiatLocked`). Then applies the owner one-time MintGate config from env.
///
/// IMPORTANT — the C1 token-wide rolling cap lives on `LocalharnessCredits`
/// itself (`tightenMintWindow`), NOT on the diamond, so it is set by the deploy
/// runbook against the FRESH mainnet `$LH` token, not here. Cutting against the
/// existing testnet `$LH` (which predates the cap) exercises the lock/clawback
/// flow but not C1 — C1 is proved in `test/MintGateFacet.t.sol`.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   FIAT_ISSUER_SIGNER=0x... CLAWBACKER=0x... \
///   FIAT_LOCK_SECS=7776000 PER_RECEIPT_MAX_WEI=0 \
///   FIAT_WINDOW_CAP_WEI=0 FIAT_WINDOW_SECS=86400 \
///   forge script script/AddMintGateFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddMintGateFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);
        address mintGate = address(new MintGateFacet());
        address meter = address(new CreditMeterFacet());
        _cut(diamond, mintGate, meter);
        _configure(diamond);
        vm.stopBroadcast();

        console.log("--- MintGateFacet cut + CreditMeter upgrade ---");
        console.log("mintGateFacet:   ", mintGate);
        console.log("creditMeterFacet:", meter);
    }

    function _cut(address diamond, address mintGate, address meter) internal {
        bytes4[] memory mg = new bytes4[](17);
        mg[0] = MintGateFacet.mintFromFiat.selector;
        mg[1] = MintGateFacet.clawbackFiatMint.selector;
        mg[2] = MintGateFacet.setFiatIssuerSigner.selector;
        mg[3] = MintGateFacet.setClawbacker.selector;
        mg[4] = MintGateFacet.setPerReceiptMaxWei.selector;
        mg[5] = MintGateFacet.setFiatLockSecs.selector;
        mg[6] = MintGateFacet.setFiatMintWindow.selector;
        mg[7] = MintGateFacet.fiatIssuerSigner.selector;
        mg[8] = MintGateFacet.clawbacker.selector;
        mg[9] = MintGateFacet.perReceiptMaxWei.selector;
        mg[10] = MintGateFacet.fiatLockSecs.selector;
        mg[11] = MintGateFacet.fiatLockedOf.selector;
        mg[12] = MintGateFacet.receiptUsed.selector;
        mg[13] = MintGateFacet.receiptInfo.selector;
        mg[14] = MintGateFacet.fiatMintWindow.selector;
        mg[15] = MintGateFacet.circulatingSupply.selector;
        mg[16] = MintGateFacet.fiatMintDomainSeparator.selector;

        bytes4[] memory meterReplace = new bytes4[](2);
        meterReplace[0] = CreditMeterFacet.withdrawCredits.selector;
        meterReplace[1] = CreditMeterFacet.meter.selector;

        bytes4[] memory meterAdd = new bytes4[](1);
        meterAdd[0] = CreditMeterFacet.withdrawableOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](3);
        cuts[0] = IDiamond.FacetCut(mintGate, IDiamond.FacetCutAction.Add, mg);
        cuts[1] = IDiamond.FacetCut(meter, IDiamond.FacetCutAction.Replace, meterReplace);
        cuts[2] = IDiamond.FacetCut(meter, IDiamond.FacetCutAction.Add, meterAdd);
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");
    }

    function _configure(address diamond) internal {
        MintGateFacet g = MintGateFacet(diamond);
        address fiatSigner = vm.envOr("FIAT_ISSUER_SIGNER", address(0));
        address clawbacker = vm.envOr("CLAWBACKER", address(0));
        if (fiatSigner != address(0)) g.setFiatIssuerSigner(fiatSigner);
        if (clawbacker != address(0)) g.setClawbacker(clawbacker);
        g.setFiatLockSecs(vm.envOr("FIAT_LOCK_SECS", uint256(90 days)));
        g.setPerReceiptMaxWei(vm.envOr("PER_RECEIPT_MAX_WEI", uint256(0)));
        g.setFiatMintWindow(vm.envOr("FIAT_WINDOW_CAP_WEI", uint256(0)), vm.envOr("FIAT_WINDOW_SECS", uint256(1 days)));
    }
}
