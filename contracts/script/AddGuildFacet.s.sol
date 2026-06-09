// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {GuildFacet} from "../src/facets/GuildFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys GuildFacet and cuts the persistent-organization layer — Rung 3 of
/// design/agent-coordination.md — into the diamond. `createGuild(name)`
/// registers `name` as a normal identity owned by the caller and seats the
/// caller as the first Admin; members are added by Officer-invite +
/// invitee-accept (the TeamFacet consent pattern); the guild's pooled `$LH`
/// treasury is a facet-balance escrow (`fundGuild` credits, Admin-only
/// `spendTreasury` debits — the BountyFacet CEI pattern). The guild's own
/// address is `tokenBoundAccount(guildId)`, which (being a contract account)
/// can itself be a member of ANOTHER guild — guilds-of-guilds for free
/// (Part 4 recursion).
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). No
/// post-cut config: the credits token is read from the shared CreditsFacet
/// storage slot, and the registrar + worker-TBA resolver are the diamond
/// itself (LocalharnessRegistryFacet + TbaFacet must already be cut, which
/// they are on the live diamond).
///
/// SELECTOR COLLISION NOTE (checked against the live diamond's `facets()`
/// before finalizing): the design ABI's `membersOf(uint256)` collides with
/// TeamFacet's already-cut `membersOf(uint256)` (selector 0x0e2aa455) — a
/// diamond can't share a selector — so the member-enumeration view is cut as
/// `guildMembersOf(uint256)` (0xe0467389). Every other selector in the
/// design ABI is collision-free on the live diamond.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddGuildFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddGuildFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        GuildFacet f = new GuildFacet();

        bytes4[] memory selectors = new bytes4[](16);
        // --- lifecycle / state transitions ---
        selectors[0] = GuildFacet.createGuild.selector;
        selectors[1] = GuildFacet.inviteToGuild.selector;
        selectors[2] = GuildFacet.acceptGuildInvite.selector;
        selectors[3] = GuildFacet.leaveGuild.selector;
        selectors[4] = GuildFacet.setRole.selector;
        selectors[5] = GuildFacet.fundGuild.selector;
        selectors[6] = GuildFacet.spendTreasury.selector;
        // --- views ---
        selectors[7] = GuildFacet.guildMembersOf.selector; // NOT membersOf (collision)
        selectors[8] = GuildFacet.roleOf.selector;
        selectors[9] = GuildFacet.isGuildMember.selector;
        selectors[10] = GuildFacet.treasuryBalanceOf.selector;
        selectors[11] = GuildFacet.guildAddress.selector;
        selectors[12] = GuildFacet.guildName.selector;
        selectors[13] = GuildFacet.guildsOf.selector;
        selectors[14] = GuildFacet.isGuild.selector;
        selectors[15] = GuildFacet.guildCount.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- GuildFacet cut ---");
        console.log("diamond:     ", diamond);
        console.log("guildFacet:  ", address(f));
    }
}
