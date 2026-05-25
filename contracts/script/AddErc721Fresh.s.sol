// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {DiamondInit} from "../src/upgradeInitializers/DiamondInit.sol";
import {ERC721Facet} from "../src/facets/ERC721Facet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// One-shot variant of `AddErc721Facet.s.sol` that targets a FRESHLY-
/// deployed diamond (one created by `DeployDiamond.s.sol`). Skips the
/// "remove old registry selectors" migration step — the fresh diamond
/// has no old selectors to remove — and just cuts the ERC-721 facet
/// in with a fresh DiamondInit to set the ERC-721 interface flags.
///
/// Run with:
///   DIAMOND=0x... \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/AddErc721Fresh.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddErc721Fresh is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ERC721Facet erc721 = new ERC721Facet();
        DiamondInit newInit = new DiamondInit();

        bytes4[] memory erc721Selectors = new bytes4[](12);
        erc721Selectors[0]  = ERC721Facet.balanceOf.selector;
        erc721Selectors[1]  = ERC721Facet.ownerOf.selector;
        erc721Selectors[2]  = ERC721Facet.approve.selector;
        erc721Selectors[3]  = ERC721Facet.getApproved.selector;
        erc721Selectors[4]  = ERC721Facet.setApprovalForAll.selector;
        erc721Selectors[5]  = ERC721Facet.isApprovedForAll.selector;
        erc721Selectors[6]  = ERC721Facet.transferFrom.selector;
        erc721Selectors[7]  = bytes4(keccak256("safeTransferFrom(address,address,uint256)"));
        erc721Selectors[8]  = bytes4(keccak256("safeTransferFrom(address,address,uint256,bytes)"));
        erc721Selectors[9]  = ERC721Facet.name.selector;
        erc721Selectors[10] = ERC721Facet.symbol.selector;
        erc721Selectors[11] = ERC721Facet.tokenURI.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
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

        console.log("--- ERC-721 facet cut (fresh diamond) ---");
        console.log("diamond:     ", diamond);
        console.log("erc721:      ", address(erc721));
        console.log("init:        ", address(newInit));
    }
}
