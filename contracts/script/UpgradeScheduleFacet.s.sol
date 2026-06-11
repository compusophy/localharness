// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ScheduleFacet} from "../src/facets/ScheduleFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Upgrades ScheduleFacet in place: deploys the new implementation (which
/// adds `completeJob` — the scheduler-only goal-completion exit that lets a
/// `/goal` ralph-loop job END ITSELF when the agent declares the goal met,
/// refunding the unspent escrow to the owner), REPLACEs all sixteen live
/// selectors to the new address (13 from AddScheduleFacet + 3 from
/// AddScheduleHardening — a blanket Replace so the whole facet is coherent
/// at one address again), and ADDs the new one. Storage
/// (LibScheduleStorage) is untouched — live jobs, escrowed budgets, the
/// scheduler role, per-owner counters, and child metadata all survive the
/// cut.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/UpgradeScheduleFacet.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract UpgradeScheduleFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ScheduleFacet f = new ScheduleFacet();

        // Every selector currently cut: the original 13 (AddScheduleFacet)
        // + the 3 hardening additions (AddScheduleHardening). Signatures
        // are unchanged, so `.selector` off the new implementation equals
        // the live selector.
        bytes4[] memory replaced = new bytes4[](16);
        replaced[0] = ScheduleFacet.scheduleJob.selector;
        replaced[1] = ScheduleFacet.recordRun.selector;
        replaced[2] = ScheduleFacet.cancelJob.selector;
        replaced[3] = ScheduleFacet.pauseJob.selector;
        replaced[4] = ScheduleFacet.resumeJob.selector;
        replaced[5] = ScheduleFacet.topUpJob.selector;
        replaced[6] = ScheduleFacet.setScheduler.selector;
        replaced[7] = ScheduleFacet.jobsDue.selector;
        replaced[8] = ScheduleFacet.getJob.selector;
        replaced[9] = ScheduleFacet.taskOf.selector;
        replaced[10] = ScheduleFacet.jobsOf.selector;
        replaced[11] = ScheduleFacet.jobCount.selector;
        replaced[12] = ScheduleFacet.schedulerAddress.selector;
        replaced[13] = ScheduleFacet.scheduleChildJob.selector;
        replaced[14] = ScheduleFacet.childMetaOf.selector;
        replaced[15] = ScheduleFacet.activeJobCountOf.selector;

        bytes4[] memory added = new bytes4[](1);
        added[0] = ScheduleFacet.completeJob.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](2);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: replaced
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: added
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ScheduleFacet upgraded (completeJob) ---");
        console.log("diamond:        ", diamond);
        console.log("scheduleFacet:  ", address(f));
        console.log("ADDED completeJob selector:");
        console.logBytes4(added[0]);
    }
}
