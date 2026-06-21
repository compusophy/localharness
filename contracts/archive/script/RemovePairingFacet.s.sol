// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {PairingFacet} from "../src/facets/PairingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Removes the dormant PairingFacet's routing from the diamond at $DIAMOND.
///
/// The facet was event-only (`announcePairing(bytes32,bytes)`) and the
/// device-pairing flow it served was superseded by QR seed-adoption
/// (Option A — the seed IS the identity); its last client helpers were
/// deleted from the bundle/CLI as dead code. A Remove cut deletes only the
/// ROUTING (selector → facet); the facet contract stays on-chain inert and
/// can be re-cut via its address if ever needed (DiamondLoupe is the
/// reversibility guarantee). Callers of announcePairing after this cut get
/// a "Diamond: function not found" revert — correct for a deprecated path.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/RemovePairingFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract RemovePairingFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        bytes4[] memory selectors = new bytes4[](1);
        selectors[0] = PairingFacet.announcePairing.selector;

        // A Remove cut MUST pass facetAddress == 0 (EIP-2535).
        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(0),
            action: IDiamond.FacetCutAction.Remove,
            functionSelectors: selectors
        });

        vm.startBroadcast(pk);
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");
        vm.stopBroadcast();

        console.log("--- PairingFacet routing removed ---");
        console.log("diamond:  ", diamond);
        console.log("selector: ");
        console.logBytes4(selectors[0]);
    }
}
