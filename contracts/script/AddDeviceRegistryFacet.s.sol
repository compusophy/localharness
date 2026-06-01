// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {DeviceRegistryFacet} from "../src/facets/DeviceRegistryFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys DeviceRegistryFacet and cuts `linkDevice / unlinkDevice /
/// devicesOf / isDeviceLinked` into the diamond at $DIAMOND — the
/// enumerable linked-device index (read in one call, no log scraping).
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddDeviceRegistryFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddDeviceRegistryFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        DeviceRegistryFacet reg = new DeviceRegistryFacet();

        bytes4[] memory selectors = new bytes4[](4);
        selectors[0] = DeviceRegistryFacet.linkDevice.selector;
        selectors[1] = DeviceRegistryFacet.unlinkDevice.selector;
        selectors[2] = DeviceRegistryFacet.devicesOf.selector;
        selectors[3] = DeviceRegistryFacet.isDeviceLinked.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(reg),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- DeviceRegistryFacet cut ---");
        console.log("diamond:             ", diamond);
        console.log("deviceRegistryFacet: ", address(reg));
    }
}
