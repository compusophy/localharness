// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ReleaseFacet} from "../src/facets/ReleaseFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Security fix: a GUILD identity is minted as a normal registry NFT held by
/// its founder (GuildFacet.createGuild), so `releaseName(guildId)` would burn
/// it — but `_burn` only clears LibRegistryStorage, leaving the guild's
/// escrowed `$LH` treasury (guildBalance) and membership rows in
/// LibGuildStorage permanently stranded. The new ReleaseFacet reverts
/// `CannotReleaseGuild` for any tokenId where the guild exists.
///
/// CUT SHAPE: deploy ONE new ReleaseFacet; REPLACE only `releaseName` (same
/// signature, new guard). The admin reset selectors (adminBurnNames /
/// adminResetAll) are intentionally left pointing at their existing
/// deployment — they are the diamond-owner testnet clean-slate path and keep
/// their unconditional-burn behavior.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/ReplaceReleaseGuildGuard.s.sol --rpc-url tempo_moderato --broadcast
contract ReplaceReleaseGuildGuard is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ReleaseFacet rel = new ReleaseFacet();

        bytes4[] memory selectors = new bytes4[](1);
        selectors[0] = ReleaseFacet.releaseName.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(rel),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ReleaseFacet guild-guard REPLACE ---");
        console.log("diamond:           ", diamond);
        console.log("newReleaseFacet:   ", address(rel));
        console.log("REPLACED releaseName selector:");
        console.logBytes4(selectors[0]);
    }
}
