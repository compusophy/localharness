// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {PairingFacet} from "../src/facets/PairingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys PairingFacet and cuts `announcePairing(bytes32)` into the
/// diamond at $DIAMOND.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddPairingFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddPairingFacet is Script {
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

        console.log("--- PairingFacet cut ---");
        console.log("diamond:       ", diamond);
        console.log("pairingFacet:  ", address(pairing));
        console.log("selector:      ");
        console.logBytes4(selectors[0]);
    }
}
