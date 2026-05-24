// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {Diamond} from "../src/Diamond.sol";
import {DiamondInit} from "../src/upgradeInitializers/DiamondInit.sol";
import {DiamondCutFacet} from "../src/facets/DiamondCutFacet.sol";
import {DiamondLoupeFacet} from "../src/facets/DiamondLoupeFacet.sol";
import {OwnershipFacet} from "../src/facets/OwnershipFacet.sol";
import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploy the full LocalharnessRegistry diamond. Run with:
///   forge script script/DeployDiamond.s.sol \
///     --rpc-url tempo_moderato \
///     --private-key $EVM_PRIVATE_KEY \
///     --broadcast
///
/// Prints the diamond address. That's the value to bake into
/// `src/app/registry.rs::REGISTRY_ADDRESS` in the wasm bundle.
contract DeployDiamond is Script {
    function run() external returns (address diamondAddr) {
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");
        address deployer = vm.addr(pk);

        vm.startBroadcast(pk);

        // 1. Deploy each facet.
        DiamondCutFacet cutFacet = new DiamondCutFacet();
        DiamondLoupeFacet loupeFacet = new DiamondLoupeFacet();
        OwnershipFacet ownershipFacet = new OwnershipFacet();
        LocalharnessRegistryFacet registryFacet = new LocalharnessRegistryFacet();
        DiamondInit diamondInit = new DiamondInit();

        // 2. Construct the Diamond with the cut facet pre-installed.
        //    Everything else is added in a follow-up diamondCut so the
        //    init can run after all selectors are wired.
        IDiamond.FacetCut[] memory initialCut = new IDiamond.FacetCut[](1);
        initialCut[0] = IDiamond.FacetCut({
            facetAddress: address(cutFacet),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: _cutSelectors()
        });
        Diamond diamond = new Diamond(deployer, initialCut);
        diamondAddr = address(diamond);

        // 3. Single batched cut for loupe + ownership + registry. The
        //    init delegatecall runs DiamondInit.init() inside the
        //    diamond's storage context.
        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](3);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(loupeFacet),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: _loupeSelectors()
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(ownershipFacet),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: _ownershipSelectors()
        });
        cuts[2] = IDiamond.FacetCut({
            facetAddress: address(registryFacet),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: _registrySelectors()
        });
        IDiamondCut(diamondAddr).diamondCut(
            cuts,
            address(diamondInit),
            abi.encodeWithSelector(DiamondInit.init.selector)
        );

        vm.stopBroadcast();

        console.log("--- Localharness Diamond deployed ---");
        console.log("diamond:     ", diamondAddr);
        console.log("owner:       ", deployer);
        console.log("cutFacet:    ", address(cutFacet));
        console.log("loupeFacet:  ", address(loupeFacet));
        console.log("ownerFacet:  ", address(ownershipFacet));
        console.log("regFacet:    ", address(registryFacet));
        console.log("init:        ", address(diamondInit));
    }

    function _cutSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](1);
        s[0] = IDiamondCut.diamondCut.selector;
    }

    function _loupeSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](5);
        s[0] = DiamondLoupeFacet.facets.selector;
        s[1] = DiamondLoupeFacet.facetFunctionSelectors.selector;
        s[2] = DiamondLoupeFacet.facetAddresses.selector;
        s[3] = DiamondLoupeFacet.facetAddress.selector;
        s[4] = DiamondLoupeFacet.supportsInterface.selector;
    }

    function _ownershipSelectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](2);
        s[0] = OwnershipFacet.transferOwnership.selector;
        s[1] = OwnershipFacet.owner.selector;
    }

    function _registrySelectors() internal pure returns (bytes4[] memory s) {
        // `transfer(uint256,address)` was dropped — ERC-721
        // transferFrom is the canonical path now (lives in ERC721Facet).
        s = new bytes4[](10);
        s[0] = LocalharnessRegistryFacet.register.selector;
        s[1] = LocalharnessRegistryFacet.setMetadata.selector;
        s[2] = LocalharnessRegistryFacet.isTaken.selector;
        s[3] = LocalharnessRegistryFacet.ownerOfName.selector;
        s[4] = LocalharnessRegistryFacet.ownerOfId.selector;
        s[5] = LocalharnessRegistryFacet.idOfName.selector;
        s[6] = LocalharnessRegistryFacet.nameOfId.selector;
        s[7] = LocalharnessRegistryFacet.idOf.selector;
        s[8] = LocalharnessRegistryFacet.nextId.selector;
        s[9] = LocalharnessRegistryFacet.metadata.selector;
    }
}
