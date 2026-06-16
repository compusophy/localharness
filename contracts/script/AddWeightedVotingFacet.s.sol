// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {WeightedVotingFacet} from "../src/facets/WeightedVotingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys WeightedVotingFacet and cuts a SHARE-WEIGHTED governance board into
/// the diamond — a cap-table layer that runs IN PARALLEL to VotingFacet (the
/// one-member-one-vote DAO, Rung 4) over the SAME guild treasury. A guild Admin
/// `setShares`; a member `proposeWeighted`s a treasury spend; members
/// `voteWeighted` (weight == their shares); a passed measure `executeWeighted`s,
/// debiting the SAME GuildFacet treasury ledger (via the inherited internal
/// `_spendCore`) and paying `to`. Quorum is MORE THAN HALF of a total-shares
/// snapshot; threshold is a strict majority of cast shares.
///
/// ALL-NEW SELECTORS (a fresh facet — Add only, no Replace/Remove). Every
/// external is `weighted`/`Weighted`/`shares`-named to dodge VotingFacet's live
/// `propose`/`vote`/`execute`/`getProposal`/`proposalsOf`/`hasVoted`/`tallyOf`/
/// `proposalCount`/`proposalMemoOf` selectors. `setShares`/`sharesOf`/
/// `totalSharesOf` are new names — VERIFY collision-free against the live
/// diamond's selector set (DiamondLoupe `facets()`) before broadcasting.
///
/// IMPORTANT: WeightedVotingFacet INHERITS GuildFacet (so `executeWeighted` can
/// call the inherited `_spendCore` against the shared LibGuildStorage slot —
/// the single-accounting-source design). We register ONLY this facet's OWN 12
/// selectors here; the inherited GuildFacet externals are ALREADY cut in and
/// routed to the live GuildFacet (0xfE806FD0…) and MUST NOT be re-added (a
/// diamond can't map one selector to two facets — diamondCut Add would revert).
/// GuildFacet AND VotingFacet must already be cut, which they are on the live
/// diamond.
///
/// No post-cut config — membership + treasury are read from the shared
/// LibGuildStorage slot and the credits token from the shared CreditsFacet
/// slot.
///
/// Run with (FORGE-VERIFY ONLY — do not broadcast without review):
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddWeightedVotingFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddWeightedVotingFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        WeightedVotingFacet f = new WeightedVotingFacet();

        bytes4[] memory selectors = new bytes4[](12);
        // --- state transitions (cap table + the lifecycle) ---
        selectors[0] = WeightedVotingFacet.setShares.selector;
        selectors[1] = WeightedVotingFacet.proposeWeighted.selector;
        selectors[2] = WeightedVotingFacet.voteWeighted.selector;
        selectors[3] = WeightedVotingFacet.executeWeighted.selector;
        // --- views (the governance surface) ---
        selectors[4] = WeightedVotingFacet.sharesOf.selector;
        selectors[5] = WeightedVotingFacet.totalSharesOf.selector;
        selectors[6] = WeightedVotingFacet.weightedProposal.selector;
        selectors[7] = WeightedVotingFacet.weightedProposalMemoOf.selector;
        selectors[8] = WeightedVotingFacet.weightedProposalsOf.selector;
        selectors[9] = WeightedVotingFacet.hasVotedWeighted.selector;
        selectors[10] = WeightedVotingFacet.weightedTallyOf.selector;
        selectors[11] = WeightedVotingFacet.weightedProposalCount.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- WeightedVotingFacet cut ---");
        console.log("diamond:              ", diamond);
        console.log("weightedVotingFacet:  ", address(f));
    }
}
