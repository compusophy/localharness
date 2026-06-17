// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {VotingFacet} from "../src/facets/VotingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Re-cut VotingFacet with the ADMIN-GATE privilege-escalation fix (VOTE-1).
/// A passing 1m1v treasury spend now MUST carry >= 1 Admin FOR vote
/// (`Proposal.forAdminVotes > 0`). Closes the sybil-flood escalation: a rogue
/// Officer (Officer+ may `inviteToGuild`, accept is free) could mint N sybil
/// Members it controls, propose a self-serving drain, and pass it on a bare
/// majority — bypassing the Admin-only `spendTreasury` gate. Sybil Members
/// carry no Admin weight, so the drain can no longer pass without Admin consent;
/// the last-Admin guard guarantees every guild always has an Admin, so honest
/// Admin-backed measures are unaffected. Single-facet, localized to VotingFacet.
///
/// SELECTORS UNCHANGED — every signature (`propose` / `vote` / `execute` /
/// `getProposal` / `proposalMemoOf` / `proposalsOf` / `hasVoted` / `tallyOf` /
/// `proposalCount`) is byte-for-byte identical; ONLY the bytecode changes (the
/// Admin-FOR tally in `vote` + the `forAdminVotes > 0` check in `_passed` + the
/// appended `Proposal` field). A pure REPLACE of all 9 VotingFacet selectors
/// onto a freshly deployed facet — no Add/Remove.
///
/// STORAGE-LAYOUT SAFE: `forAdminVotes` was appended at the END of the
/// `Proposal` struct (after `snapshotMemberCount`) — the diamond append-only
/// rule. A proposal created under the OLD bytecode reads `forAdminVotes == 0`,
/// so a pre-fix in-flight proposal can no longer pass until re-proposed —
/// FAIL-SAFE (never a silent spend). Re-cut promptly, or let in-flight
/// proposals resolve first. The inherited GuildFacet externals are NOT touched.
///
/// Run with (FORGE-VERIFY ONLY — do not broadcast without review):
///   DIAMOND=0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77 \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/ReplaceVotingFacetAdminGate.s.sol \
///       --rpc-url tempo_mainnet --broadcast
contract ReplaceVotingFacetAdminGate is Script {
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

        console.log("--- VotingFacet re-cut (admin-gate escalation fix, VOTE-1) ---");
        console.log("diamond:      ", diamond);
        console.log("votingFacet:  ", address(f));
    }
}
