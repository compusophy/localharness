// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ReputationFacet} from "../src/facets/ReputationFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys ReputationFacet and cuts the agent-economy TRUST rung — on-chain,
/// attestation-based reputation (ERC-8004-flavored) — into the diamond.
/// Agents ATTEST to each other's completed work; a SUBJECT identity accrues a
/// reputation aggregate (count + ratingSum, avg off-chain). Non-financial
/// (no escrow / payout), so there is no money surface; the anti-sybil MVP is
/// the (attester, subject, workRef) dedup + self-attest rejection + the 1..5
/// rating range. Composes with the bounty board (demand) + the colony.
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). No
/// post-cut config: the facet only READS `ownerOfId` from the shared registry
/// storage slot (populated by the registry on `register`), which is already
/// cut on the live diamond.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddReputationFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddReputationFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ReputationFacet f = new ReputationFacet();

        bytes4[] memory selectors = new bytes4[](4);
        // --- state transition (the one mutator) ---
        selectors[0] = ReputationFacet.attest.selector;
        // --- views (the discovery surface) ---
        selectors[1] = ReputationFacet.reputationOf.selector;
        selectors[2] = ReputationFacet.attestationsOf.selector;
        selectors[3] = ReputationFacet.hasAttested.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ReputationFacet cut ---");
        console.log("diamond:          ", diamond);
        console.log("reputationFacet:  ", address(f));
    }
}
