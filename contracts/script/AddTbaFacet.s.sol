// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ERC6551Registry} from "../src/erc6551/ERC6551Registry.sol";
import {ERC6551Account} from "../src/erc6551/ERC6551Account.sol";
import {TbaFacet} from "../src/facets/TbaFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys ERC-6551 registry + account implementation, then cuts a
/// TbaFacet into the diamond and configures it to point at the new
/// 6551 contracts.
///
/// Run with:
///   DIAMOND=0xed7a2d170ab2d41721c9bd7368adbff6df0c656d \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddTbaFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddTbaFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ERC6551Registry registry = new ERC6551Registry();
        ERC6551Account accountImpl = new ERC6551Account();
        TbaFacet tba = new TbaFacet();

        bytes4[] memory selectors = new bytes4[](6);
        selectors[0] = TbaFacet.setTbaConfig.selector;
        selectors[1] = TbaFacet.tbaRegistry.selector;
        selectors[2] = TbaFacet.tbaAccountImpl.selector;
        selectors[3] = TbaFacet.tokenBoundAccount.selector;
        selectors[4] = TbaFacet.tokenBoundAccountByName.selector;
        selectors[5] = TbaFacet.createTokenBoundAccount.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(tba),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        // Wire the facet to point at the freshly-deployed registry +
        // account impl. Owner-only — broadcaster is the diamond owner.
        TbaFacet(diamond).setTbaConfig(address(registry), address(accountImpl));

        vm.stopBroadcast();

        console.log("--- TBA facet cut + configured ---");
        console.log("diamond:     ", diamond);
        console.log("registry:    ", address(registry));
        console.log("accountImpl: ", address(accountImpl));
        console.log("tbaFacet:    ", address(tba));
    }
}
