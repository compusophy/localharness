// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {VotingFacet} from "../src/facets/VotingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys VotingFacet and cuts the DAO governance layer — Rung 4 of
/// design/agent-coordination.md, the apex — into the diamond. A guild
/// MEMBER `propose`s a treasury spend; members `vote` one-member-one-vote;
/// a passed measure `execute`s, debiting the SAME GuildFacet treasury
/// ledger (via the inherited internal `_spend`) and paying `to`. Turns a
/// guild from Admin-controlled into member-governed.
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). The
/// generic names `propose`/`vote`/`execute`/`getProposal`/`proposalsOf`/
/// `hasVoted`/`tallyOf`/`proposalCount`/`proposalMemoOf` were verified
/// COLLISION-FREE against the live diamond's selector set (no clash, so no
/// `gov`/`proposal` prefix was needed — unlike BountyFacet's `bountyTaskOf`
/// or GuildFacet's `guildMembersOf`).
///
/// IMPORTANT: VotingFacet INHERITS GuildFacet (so `execute` can call the
/// inherited `_spend` against the shared LibGuildStorage slot — the
/// single-accounting-source design). We register ONLY VotingFacet's OWN 9
/// selectors here; the inherited GuildFacet externals are ALREADY cut in and
/// routed to the live GuildFacet (0xfE806FD0…) and MUST NOT be re-added (a
/// diamond can't map one selector to two facets — diamondCut Add would
/// revert). GuildFacet must already be cut, which it is on the live diamond.
///
/// No post-cut config — membership + treasury are read from the shared
/// LibGuildStorage slot and the credits token from the shared CreditsFacet
/// slot.
///
/// Run with (FORGE-VERIFY ONLY — do not broadcast without review):
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddVotingFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddVotingFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        VotingFacet f = new VotingFacet();

        bytes4[] memory selectors = new bytes4[](9);
        // --- state transitions (the lifecycle) ---
        selectors[0] = VotingFacet.propose.selector;
        selectors[1] = VotingFacet.vote.selector;
        selectors[2] = VotingFacet.execute.selector;
        // --- views (the governance surface) ---
        selectors[3] = VotingFacet.getProposal.selector;
        selectors[4] = VotingFacet.proposalMemoOf.selector;
        selectors[5] = VotingFacet.proposalsOf.selector;
        selectors[6] = VotingFacet.hasVoted.selector;
        selectors[7] = VotingFacet.tallyOf.selector;
        selectors[8] = VotingFacet.proposalCount.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- VotingFacet cut ---");
        console.log("diamond:      ", diamond);
        console.log("votingFacet:  ", address(f));
    }
}
