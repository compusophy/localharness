// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {WipeFacet} from "../src/facets/WipeFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys WipeFacet and cuts its `wipeRegistry(uint256)` selector into
/// the diamond at $DIAMOND. Owner-only at runtime — the broadcaster must
/// be the diamond owner.
///
/// Run with:
///   DIAMOND=0xed7a2d170ab2d41721c9bd7368adbff6df0c656d \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddWipeFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
///
/// After cut, call `wipeRegistry(0)` from the same key to nuke all
/// existing registrations:
///   cast send $DIAMOND "wipeRegistry(uint256)" 0 \
///       --rpc-url tempo_moderato --private-key $EVM_PRIVATE_KEY
contract AddWipeFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        WipeFacet wipe = new WipeFacet();

        bytes4[] memory selectors = new bytes4[](1);
        selectors[0] = WipeFacet.wipeRegistry.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(wipe),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- WipeFacet cut ---");
        console.log("diamond:    ", diamond);
        console.log("wipeFacet:  ", address(wipe));
        console.log("selector:   ");
        console.logBytes4(selectors[0]);
    }
}
