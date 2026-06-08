// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ScheduleFacet} from "../src/facets/ScheduleFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Deploys ScheduleFacet and cuts the durable agent-scheduling job
/// registry into the diamond (design/agent-scheduling.md). Holders
/// escrow `$LH` to back recurring jobs; the off-chain worker fires due
/// jobs and records runs.
///
/// AFTER cutting, set the worker (the credit proxy's scheduler key) as
/// the authorized scheduler:
///   cast send $DIAMOND "setScheduler(address)" 0x<proxySchedulerAddr> ...
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddScheduleFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddScheduleFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ScheduleFacet f = new ScheduleFacet();

        bytes4[] memory selectors = new bytes4[](13);
        selectors[0] = ScheduleFacet.scheduleJob.selector;
        selectors[1] = ScheduleFacet.recordRun.selector;
        selectors[2] = ScheduleFacet.cancelJob.selector;
        selectors[3] = ScheduleFacet.pauseJob.selector;
        selectors[4] = ScheduleFacet.resumeJob.selector;
        selectors[5] = ScheduleFacet.topUpJob.selector;
        selectors[6] = ScheduleFacet.setScheduler.selector;
        selectors[7] = ScheduleFacet.jobsDue.selector;
        selectors[8] = ScheduleFacet.getJob.selector;
        selectors[9] = ScheduleFacet.taskOf.selector;
        selectors[10] = ScheduleFacet.jobsOf.selector;
        selectors[11] = ScheduleFacet.jobCount.selector;
        selectors[12] = ScheduleFacet.schedulerAddress.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- ScheduleFacet cut ---");
        console.log("diamond:        ", diamond);
        console.log("scheduleFacet:  ", address(f));
        console.log("NEXT: setScheduler(<proxy worker key>)");
    }
}
