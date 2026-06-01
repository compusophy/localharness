// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {X402Facet} from "../src/facets/X402Facet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys X402Facet and cuts `settle / authorizationState /
/// x402DomainSeparator` into the diamond at $DIAMOND. Settles `$LH`
/// payments — payers must `approve(diamond, ...)` `$LH` once.
///
/// Run with:
///   DIAMOND=0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930 \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddX402Facet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddX402Facet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        X402Facet x402 = new X402Facet();

        bytes4[] memory selectors = new bytes4[](3);
        selectors[0] = X402Facet.settle.selector;
        selectors[1] = X402Facet.authorizationState.selector;
        selectors[2] = X402Facet.x402DomainSeparator.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(x402),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- X402Facet cut ---");
        console.log("diamond:    ", diamond);
        console.log("x402Facet:  ", address(x402));
    }
}
