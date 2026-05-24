// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {DiamondInit} from "../src/upgradeInitializers/DiamondInit.sol";
import {LocalharnessRegistryFacet} from "../src/facets/LocalharnessRegistryFacet.sol";
import {ERC721Facet} from "../src/facets/ERC721Facet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Adds the ERC-721 facet to the live diamond and re-cuts the
/// registry facet (its register() bytecode changed: now bumps
/// balanceOf + emits Transfer for ERC-721 conformance; the old
/// `transfer(uint256,address)` selector is dropped in favour of
/// the ERC-721 transferFrom).
///
/// Run with:
///   DIAMOND=0xed7a2d170ab2d41721c9bd7368adbff6df0c656d \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddErc721Facet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddErc721Facet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        // Deploy fresh facets + a one-shot init for the new interface bits.
        LocalharnessRegistryFacet newRegistry = new LocalharnessRegistryFacet();
        ERC721Facet erc721 = new ERC721Facet();
        DiamondInit newInit = new DiamondInit();

        // Old registry selectors to drop (Remove with facetAddress=0).
        bytes4[] memory oldSelectors = new bytes4[](11);
        oldSelectors[0]  = bytes4(keccak256("register(string)"));
        oldSelectors[1]  = bytes4(keccak256("transfer(uint256,address)"));
        oldSelectors[2]  = bytes4(keccak256("setMetadata(uint256,bytes32,bytes)"));
        oldSelectors[3]  = bytes4(keccak256("isTaken(string)"));
        oldSelectors[4]  = bytes4(keccak256("ownerOfName(string)"));
        oldSelectors[5]  = bytes4(keccak256("ownerOfId(uint256)"));
        oldSelectors[6]  = bytes4(keccak256("idOfName(string)"));
        oldSelectors[7]  = bytes4(keccak256("nameOfId(uint256)"));
        oldSelectors[8]  = bytes4(keccak256("idOf(address)"));
        oldSelectors[9]  = bytes4(keccak256("nextId()"));
        oldSelectors[10] = bytes4(keccak256("metadata(uint256,bytes32)"));

        // New registry selectors (10 — no `transfer` anymore).
        bytes4[] memory newSelectors = new bytes4[](10);
        newSelectors[0] = LocalharnessRegistryFacet.register.selector;
        newSelectors[1] = LocalharnessRegistryFacet.setMetadata.selector;
        newSelectors[2] = LocalharnessRegistryFacet.isTaken.selector;
        newSelectors[3] = LocalharnessRegistryFacet.ownerOfName.selector;
        newSelectors[4] = LocalharnessRegistryFacet.ownerOfId.selector;
        newSelectors[5] = LocalharnessRegistryFacet.idOfName.selector;
        newSelectors[6] = LocalharnessRegistryFacet.nameOfId.selector;
        newSelectors[7] = LocalharnessRegistryFacet.idOf.selector;
        newSelectors[8] = LocalharnessRegistryFacet.nextId.selector;
        newSelectors[9] = LocalharnessRegistryFacet.metadata.selector;

        bytes4[] memory erc721Selectors = _erc721Selectors();

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](3);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(0),
            action: IDiamond.FacetCutAction.Remove,
            functionSelectors: oldSelectors
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(newRegistry),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: newSelectors
        });
        cuts[2] = IDiamond.FacetCut({
            facetAddress: address(erc721),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: erc721Selectors
        });

        IDiamondCut(diamond).diamondCut(
            cuts,
            address(newInit),
            abi.encodeWithSelector(DiamondInit.initErc721.selector)
        );

        vm.stopBroadcast();

        console.log("--- ERC-721 facet cut ---");
        console.log("diamond:     ", diamond);
        console.log("newRegistry: ", address(newRegistry));
        console.log("erc721:      ", address(erc721));
        console.log("newInit:     ", address(newInit));
    }

    function _erc721Selectors() internal pure returns (bytes4[] memory s) {
        s = new bytes4[](12);
        s[0]  = ERC721Facet.balanceOf.selector;
        s[1]  = ERC721Facet.ownerOf.selector;
        s[2]  = ERC721Facet.approve.selector;
        s[3]  = ERC721Facet.getApproved.selector;
        s[4]  = ERC721Facet.setApprovalForAll.selector;
        s[5]  = ERC721Facet.isApprovedForAll.selector;
        s[6]  = ERC721Facet.transferFrom.selector;
        // The two safeTransferFrom variants have different selectors.
        s[7]  = bytes4(keccak256("safeTransferFrom(address,address,uint256)"));
        s[8]  = bytes4(keccak256("safeTransferFrom(address,address,uint256,bytes)"));
        s[9]  = ERC721Facet.name.selector;
        s[10] = ERC721Facet.symbol.selector;
        s[11] = ERC721Facet.tokenURI.selector;
    }
}
