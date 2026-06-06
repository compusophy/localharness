// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {TeamFacet} from "../src/facets/TeamFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys + cuts the TeamFacet (agent teams by mutual invite + accept) into
/// the diamond. A team becomes a signaling topic its members sync within, so
/// P2P sync / x402 / call_agent all flow along the same consented peer set.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddTeamFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddTeamFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        TeamFacet f = new TeamFacet();

        bytes4[] memory selectors = new bytes4[](11);
        selectors[0] = TeamFacet.createTeam.selector;
        selectors[1] = TeamFacet.invite.selector;
        selectors[2] = TeamFacet.accept.selector;
        selectors[3] = TeamFacet.decline.selector;
        selectors[4] = TeamFacet.leave.selector;
        selectors[5] = TeamFacet.membersOf.selector;
        selectors[6] = TeamFacet.teamsOf.selector;
        selectors[7] = TeamFacet.isMember.selector;
        selectors[8] = TeamFacet.isInvited.selector;
        selectors[9] = TeamFacet.teamName.selector;
        selectors[10] = TeamFacet.nextTeamId.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- TeamFacet cut ---");
        console.log("diamond: ", diamond);
        console.log("facet:   ", address(f));
    }
}
