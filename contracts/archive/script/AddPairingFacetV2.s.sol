// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {PairingFacet} from "../src/facets/PairingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Cuts the v2 pairing surface — `announcePairing(bytes32,bytes)` — into
/// the diamond. The blob now carries the device's compressed pubkey so
/// the desktop can ECIES-wrap the Gemini key to it. The old
/// `announcePairing(bytes32)` selector is left cut (harmless orphan); the
/// bundle only calls the v2 selector, so this is a zero-downtime Add.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddPairingFacetV2.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddPairingFacetV2 is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        PairingFacet pairing = new PairingFacet();

        bytes4[] memory selectors = new bytes4[](1);
        selectors[0] = PairingFacet.announcePairing.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(pairing),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- PairingFacet v2 cut ---");
        console.log("diamond:       ", diamond);
        console.log("pairingFacet:  ", address(pairing));
        console.log("selector announcePairing(bytes32,bytes):");
        console.logBytes4(selectors[0]);
    }
}
