// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {PartyFacet} from "../src/facets/PartyFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys PartyFacet and cuts the agent-economy PARTY rung — ad-hoc squads,
/// Rung 2 of design/shipped/agent-coordination.md — into the diamond. A
/// creator FORMS a squad of member identities with a pre-agreed bps split;
/// each member's owner CONSENTS (joinParty); anyone FUNDS the pot; the
/// creator COMPLETES → the pot splits to the members' TBAs by shares (the
/// remainder to the last member, escrow-exact); disband / TTL expiry refunds
/// every funder their exact contribution.
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). Every
/// view is `party`-prefixed (the bountyTaskOf-vs-taskOf lesson — a diamond
/// can't share a selector). No post-cut config: the credits token is read
/// from the shared CreditsFacet storage slot, and the member-TBA resolver is
/// the diamond itself (TbaFacet must already be cut, which it is on the live
/// diamond).
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddPartyFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddPartyFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        PartyFacet f = new PartyFacet();

        bytes4[] memory selectors = new bytes4[](15);
        // --- state transitions (the lifecycle) ---
        selectors[0] = PartyFacet.formParty.selector;
        selectors[1] = PartyFacet.joinParty.selector;
        selectors[2] = PartyFacet.fundParty.selector;
        selectors[3] = PartyFacet.completeParty.selector;
        selectors[4] = PartyFacet.disbandParty.selector;
        // --- views (the squad-board surface) ---
        selectors[5] = PartyFacet.getParty.selector;
        selectors[6] = PartyFacet.partyMembersOf.selector;
        selectors[7] = PartyFacet.partySharesOf.selector;
        selectors[8] = PartyFacet.partyConsentOf.selector;
        selectors[9] = PartyFacet.partyFundersOf.selector;
        selectors[10] = PartyFacet.partyContributionOf.selector;
        selectors[11] = PartyFacet.partiesOf.selector;
        selectors[12] = PartyFacet.partyCount.selector;
        selectors[13] = PartyFacet.activePartyCountOf.selector;
        selectors[14] = PartyFacet.liveParties.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- PartyFacet cut ---");
        console.log("diamond:     ", diamond);
        console.log("partyFacet:  ", address(f));
    }
}
