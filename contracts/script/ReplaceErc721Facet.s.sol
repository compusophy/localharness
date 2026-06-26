// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ERC721Facet} from "../src/facets/ERC721Facet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Replace ERC721Facet to pick up the audit L14 fix: an ownership transfer now
/// clears the MAIN pointer when the transferred token == `mainOf(from)`,
/// mirroring what the burn path already does. Without it `mainOf` dangled for a
/// former owner, so the per-MAIN Gemini-key-sync read (and `isMain`) misfired
/// after a name changed hands. The 12 ERC-721 selectors are UNCHANGED — this is a
/// pure Replace onto a freshly deployed facet (same selector list as
/// AddErc721Facet.s.sol::_erc721Selectors).
///
/// Run with (per chain):
///   DIAMOND=<diamond> EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/ReplaceErc721Facet.s.sol --rpc-url <tempo_moderato|tempo_mainnet> --broadcast
contract ReplaceErc721Facet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ERC721Facet erc721 = new ERC721Facet();

        bytes4[] memory s = new bytes4[](12);
        s[0] = ERC721Facet.balanceOf.selector;
        s[1] = ERC721Facet.ownerOf.selector;
        s[2] = ERC721Facet.approve.selector;
        s[3] = ERC721Facet.getApproved.selector;
        s[4] = ERC721Facet.setApprovalForAll.selector;
        s[5] = ERC721Facet.isApprovedForAll.selector;
        s[6] = ERC721Facet.transferFrom.selector;
        // The two safeTransferFrom variants have different selectors.
        s[7] = bytes4(keccak256("safeTransferFrom(address,address,uint256)"));
        s[8] = bytes4(keccak256("safeTransferFrom(address,address,uint256,bytes)"));
        s[9] = ERC721Facet.name.selector;
        s[10] = ERC721Facet.symbol.selector;
        s[11] = ERC721Facet.tokenURI.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(erc721),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: s
        });

        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ERC721Facet replaced (L14: clear MAIN on transfer) ---");
        console.log("diamond:", diamond);
        console.log("erc721: ", address(erc721));
    }
}
