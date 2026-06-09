// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {VotingFacet} from "../src/facets/VotingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Re-cut VotingFacet with the QUORUM-SNAPSHOT-AT-PROPOSE governance-
/// robustness fix. Closes the membership-churn class where members could
/// join/leave between propose and execute to move the quorum bar — shrink it
/// (members leave so a thin minority meets the smaller quorum and drains the
/// treasury; a voter could even vote FOR then leave, their ballot still
/// counted while the denominator dropped under it) or inflate it (sybil-flood
/// new members to push the live quorum above the honest cast votes and sink a
/// legitimately-passing measure). The fix freezes the guild's `memberCount`
/// into `Proposal.snapshotMemberCount` at propose; `_passed` / `tallyOf` read
/// the snapshot instead of the live count.
///
/// SELECTORS UNCHANGED — every signature (`propose` / `vote` / `execute` /
/// `getProposal` / `proposalMemoOf` / `proposalsOf` / `hasVoted` / `tallyOf` /
/// `proposalCount`) is byte-for-byte identical; ONLY the bytecode changes
/// (the snapshot read/write + the extra `Proposal` field). So this is a pure
/// REPLACE of all 9 VotingFacet selectors onto a freshly deployed facet — no
/// Add/Remove.
///
/// STORAGE-LAYOUT SAFE: `snapshotMemberCount` was added at the END of the
/// `Proposal` struct (the diamond append-only rule). Any proposal created
/// under the OLD bytecode reads `snapshotMemberCount == 0`, which `_quorum`
/// maps to 1 — so a pre-fix in-flight proposal still requires at least one
/// vote and can never silently auto-pass (it just uses the degenerate quorum
/// of 1 rather than its original live count; re-cut promptly if any vote is
/// mid-flight, or simply let in-flight proposals resolve first). The inherited
/// GuildFacet externals are NOT touched — they remain routed to the live
/// GuildFacet.
///
/// Run with (FORGE-VERIFY ONLY — do not broadcast without review):
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/ReplaceVotingFacetSnapshot.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract ReplaceVotingFacetSnapshot is Script {
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
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- VotingFacet re-cut (quorum snapshot-at-propose fix) ---");
        console.log("diamond:      ", diamond);
        console.log("votingFacet:  ", address(f));
    }
}
