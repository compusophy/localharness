// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {BountyFacet} from "../src/facets/BountyFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys BountyFacet and cuts the agent-economy bounty board — the
/// demand-side marketplace, Rung 1 of design/agent-coordination.md — into
/// the diamond. An agent POSTS a task and escrows a `$LH` reward; another
/// CLAIMS it, does the work, and SUBMITS a result; the poster ACCEPTS →
/// the reward settles to the worker's TBA. Escrow is InviteFacet's
/// state-machine with a "poster confirms the result" release condition.
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). No
/// post-cut config: the credits token is read from the shared CreditsFacet
/// storage slot (set once via `setCreditsToken`), and the worker-TBA
/// resolver is the diamond itself (TbaFacet must already be cut, which it
/// is on the live diamond).
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddBountyFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddBountyFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        BountyFacet f = new BountyFacet();

        bytes4[] memory selectors = new bytes4[](13);
        // --- state transitions (the lifecycle) ---
        selectors[0] = BountyFacet.postBounty.selector;
        selectors[1] = BountyFacet.claimBounty.selector;
        selectors[2] = BountyFacet.submitResult.selector;
        selectors[3] = BountyFacet.acceptResult.selector;
        selectors[4] = BountyFacet.cancelBounty.selector;
        selectors[5] = BountyFacet.reclaimExpired.selector;
        // --- views (the discovery surface) ---
        selectors[6] = BountyFacet.getBounty.selector;
        selectors[7] = BountyFacet.bountyTaskOf.selector;
        selectors[8] = BountyFacet.resultOf.selector;
        selectors[9] = BountyFacet.openBounties.selector;
        selectors[10] = BountyFacet.bountiesOf.selector;
        selectors[11] = BountyFacet.bountyCount.selector;
        selectors[12] = BountyFacet.activeBountyCountOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- BountyFacet cut ---");
        console.log("diamond:      ", diamond);
        console.log("bountyFacet:  ", address(f));
    }
}
