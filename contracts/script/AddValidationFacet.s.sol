// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ValidationFacet} from "../src/facets/ValidationFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys ValidationFacet and cuts ERC-8004-style VALIDATION STAKING — the
/// money-backed half of the reputation system (ReputationFacet attestations
/// are the free-signal half) — into the diamond. A validator ESCROWS `$LH`
/// behind a verdict about a subject's `workRef`; a challenger counter-stakes
/// the opposite verdict; the work's bounty POSTER (or the diamond owner as
/// arbiter fallback) resolves and the loser's stake pays the winner.
/// Unchallenged stakes reclaim after the challenge window; unresolved
/// challenges auto-draw (both refunded) after the resolve window — escrow-
/// conservation throughout (supply-neutral; no minting).
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). No
/// post-cut config: the credits token (CreditsFacet slot), identity owners
/// (registry slot), bounty posters (bounty slot — the resolver coupling),
/// and the diamond owner (LibDiamond) are all shared storage already
/// populated on the live diamond.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddValidationFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddValidationFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ValidationFacet f = new ValidationFacet();

        bytes4[] memory selectors = new bytes4[](13);
        // --- state transitions (the escrow lifecycle) ---
        selectors[0] = ValidationFacet.stakeValidation.selector;
        selectors[1] = ValidationFacet.challengeValidation.selector;
        selectors[2] = ValidationFacet.resolveValidation.selector;
        selectors[3] = ValidationFacet.reclaimStake.selector;
        selectors[4] = ValidationFacet.reclaimUnresolved.selector;
        // --- views (the discovery surface) ---
        selectors[5] = ValidationFacet.getValidation.selector;
        selectors[6] = ValidationFacet.validationResolverOf.selector;
        selectors[7] = ValidationFacet.hasValidated.selector;
        selectors[8] = ValidationFacet.validationsOfWork.selector;
        selectors[9] = ValidationFacet.validationsOf.selector;
        selectors[10] = ValidationFacet.validationCount.selector;
        selectors[11] = ValidationFacet.validationStakedOf.selector;
        selectors[12] = ValidationFacet.activeValidationCountOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ValidationFacet cut ---");
        console.log("diamond:          ", diamond);
        console.log("validationFacet:  ", address(f));
    }
}
