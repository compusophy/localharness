// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";
import {MainIdentityFacet} from "../src/facets/MainIdentityFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Combined re-cut: swaps `LocalharnessRegistryFacet` (adds
/// treasuryBalance + withdrawTreasury) AND `MainIdentityFacet` (adds
/// setMainCost + mainCost + body change to registerMain). Done in
/// one `diamondCut` call so the diamond never observes a partial
/// upgrade.
///
/// Leaves the MAIN cost at zero (gate off) — owner can ramp later
/// via `setMainCost`. Registration cost stays at whatever was set
/// previously (50 LH at last cut).
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/SwapTreasuryAndMainCost.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract SwapTreasuryAndMainCost is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        LocalharnessRegistryFacet newRegistry = new LocalharnessRegistryFacet();
        MainIdentityFacet newMain = new MainIdentityFacet();

        // --- Registry facet: Remove 12 old + Add 14 new ---
        bytes4[] memory oldReg = new bytes4[](12);
        oldReg[0] = bytes4(keccak256("register(string)"));
        oldReg[1] = bytes4(keccak256("setMetadata(uint256,bytes32,bytes)"));
        oldReg[2] = bytes4(keccak256("isTaken(string)"));
        oldReg[3] = bytes4(keccak256("ownerOfName(string)"));
        oldReg[4] = bytes4(keccak256("ownerOfId(uint256)"));
        oldReg[5] = bytes4(keccak256("idOfName(string)"));
        oldReg[6] = bytes4(keccak256("nameOfId(uint256)"));
        oldReg[7] = bytes4(keccak256("idOf(address)"));
        oldReg[8] = bytes4(keccak256("nextId()"));
        oldReg[9] = bytes4(keccak256("metadata(uint256,bytes32)"));
        oldReg[10] = bytes4(keccak256("setRegistrationCost(uint256)"));
        oldReg[11] = bytes4(keccak256("registrationCost()"));

        bytes4[] memory newReg = new bytes4[](14);
        newReg[0] = LocalharnessRegistryFacet.register.selector;
        newReg[1] = LocalharnessRegistryFacet.setMetadata.selector;
        newReg[2] = LocalharnessRegistryFacet.isTaken.selector;
        newReg[3] = LocalharnessRegistryFacet.ownerOfName.selector;
        newReg[4] = LocalharnessRegistryFacet.ownerOfId.selector;
        newReg[5] = LocalharnessRegistryFacet.idOfName.selector;
        newReg[6] = LocalharnessRegistryFacet.nameOfId.selector;
        newReg[7] = LocalharnessRegistryFacet.idOf.selector;
        newReg[8] = LocalharnessRegistryFacet.nextId.selector;
        newReg[9] = LocalharnessRegistryFacet.metadata.selector;
        newReg[10] = LocalharnessRegistryFacet.setRegistrationCost.selector;
        newReg[11] = LocalharnessRegistryFacet.registrationCost.selector;
        newReg[12] = LocalharnessRegistryFacet.treasuryBalance.selector;
        newReg[13] = LocalharnessRegistryFacet.withdrawTreasury.selector;

        // --- MainIdentityFacet: Remove 5 old + Add 7 new ---
        bytes4[] memory oldMain = new bytes4[](5);
        oldMain[0] = bytes4(keccak256("registerMain(uint256)"));
        oldMain[1] = bytes4(keccak256("clearMain()"));
        oldMain[2] = bytes4(keccak256("mainOf(address)"));
        oldMain[3] = bytes4(keccak256("mainNameOf(address)"));
        oldMain[4] = bytes4(keccak256("isMain(uint256)"));

        bytes4[] memory newMainSel = new bytes4[](7);
        newMainSel[0] = MainIdentityFacet.registerMain.selector;
        newMainSel[1] = MainIdentityFacet.clearMain.selector;
        newMainSel[2] = MainIdentityFacet.mainOf.selector;
        newMainSel[3] = MainIdentityFacet.mainNameOf.selector;
        newMainSel[4] = MainIdentityFacet.isMain.selector;
        newMainSel[5] = MainIdentityFacet.setMainCost.selector;
        newMainSel[6] = MainIdentityFacet.mainCost.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](4);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(0),
            action: IDiamond.FacetCutAction.Remove,
            functionSelectors: oldReg
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(newRegistry),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: newReg
        });
        cuts[2] = IDiamond.FacetCut({
            facetAddress: address(0),
            action: IDiamond.FacetCutAction.Remove,
            functionSelectors: oldMain
        });
        cuts[3] = IDiamond.FacetCut({
            facetAddress: address(newMain),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: newMainSel
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- treasury + main-cost re-cut ---");
        console.log("diamond:        ", diamond);
        console.log("newRegistry:    ", address(newRegistry));
        console.log("newMain:        ", address(newMain));
    }
}
