// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ScheduleFacet} from "../src/facets/ScheduleFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Drift-correction upgrade (#46 A): deploys the new ScheduleFacet whose
/// `recordRun` anchors the next fire to the SCHEDULE GRID (firstSlot +
/// k*interval) instead of `now + interval`, so a late/slow keeper tick no
/// longer permanently shifts an alarm later ("fires 3 minutes late, then
/// drifts further"). Pure REPLACE of all 17 currently-live selectors to the
/// new address — `completeJob` is ALREADY cut, so this is a blanket Replace
/// with NO Add (unlike UpgradeScheduleFacet.s.sol, which added it). Storage
/// (LibScheduleStorage) is byte-for-byte unchanged — every live job, escrow,
/// the scheduler role, counters, and child metadata survive the cut.
///
/// Run (TESTNET first, then mainnet):
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x... \
///   forge script script/UpgradeScheduleFacetDriftFix.s.sol \
///       --rpc-url tempo_moderato --broadcast
///   # mainnet: DIAMOND=0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77 --rpc-url tempo_mainnet
contract UpgradeScheduleFacetDriftFix is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ScheduleFacet f = new ScheduleFacet();

        // All 17 currently-live selectors (16 base/hardening + completeJob).
        // Signatures are unchanged, so `.selector` off the new impl equals the
        // live selector — a clean in-place Replace.
        bytes4[] memory replaced = new bytes4[](17);
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
        replaced[16] = ScheduleFacet.completeJob.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: replaced
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ScheduleFacet upgraded (drift-corrected recordRun) ---");
        console.log("diamond:       ", diamond);
        console.log("scheduleFacet: ", address(f));
    }
}
