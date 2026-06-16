// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {TitheFacet} from "../src/facets/TitheFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys TitheFacet and cuts the OPT-IN auto-tithe layer into the diamond —
/// the revenue→treasury automation that makes a GuildFacet treasury (Rung 3 of
/// design/agent-coordination.md) self-funding from its members' earnings
/// without a tab and without a per-contribution signature.
///
/// THE CONSENT MODEL: `setTithe(guildId, bps)` / `revokeTithe()` are SELF-ONLY
/// (keyed on msg.sender — an agent's TBA only ever configures itself); a
/// PERMISSIONLESS `collectTithe(account)` reads ONLY that account's own stored
/// `(guildId, bps)`, pulls `bps/10000` of its `$LH` balance (capped by its
/// allowance to the diamond), and credits `LibGuildStorage.guildBalance`
/// exactly as `fundGuild`. The caller picks WHEN; the account already picked
/// WHO and HOW MUCH, so permissionless triggering can't redirect or inflate.
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). No
/// post-cut config: the credits token is read from the shared CreditsFacet
/// storage slot, and the guild ledger from the shared GuildFacet slot
/// (GuildFacet + CreditsFacet must already be cut, which they are on the live
/// diamond).
///
/// SELECTOR COLLISION NOTE (check against the live diamond's `facets()` before
/// finalizing): `setTithe(uint256,uint256)`, `revokeTithe()`,
/// `collectTithe(address)`, and `titheOf(address)` are all tithe-prefixed /
/// uniquely-shaped and collision-free on the live diamond.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddTitheFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddTitheFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        TitheFacet f = new TitheFacet();

        bytes4[] memory selectors = new bytes4[](4);
        selectors[0] = TitheFacet.setTithe.selector;
        selectors[1] = TitheFacet.revokeTithe.selector;
        selectors[2] = TitheFacet.collectTithe.selector;
        selectors[3] = TitheFacet.titheOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- TitheFacet cut ---");
        console.log("diamond:     ", diamond);
        console.log("titheFacet:  ", address(f));
    }
}
