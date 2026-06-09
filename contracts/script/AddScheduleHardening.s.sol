// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ScheduleFacet} from "../src/facets/ScheduleFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Typed surface of the LIVE ScheduleFacet selectors this cut touches.
/// Used ONLY to read selectors via `.selector` (no magic 4-byte
/// literals) — `IScheduleLive.scheduleJob.selector` is identical to the
/// deployed selector because the function signature is identical. The
/// three REPLACED functions kept their signatures (only their internal
/// logic changed: the activeJobsOf counter), so a Replace re-points the
/// existing selectors to the new facet deployment.
interface IScheduleLive {
    // --- REPLACED: same signature, new logic (per-owner cap counter) ---
    function scheduleJob(uint256 targetId, bytes calldata task, uint64 interval, uint128 budgetWei, uint32 maxRuns)
        external
        returns (uint256 id);
    function recordRun(uint256 id, uint64 expectedNextRun, uint128 spentWei) external returns (uint64 newNextRun);
    function cancelJob(uint256 id) external;
    // --- ADDED: brand-new functions (recursion + new views) -----------
    function scheduleChildJob(
        uint256 parentJobId,
        uint256 targetId,
        bytes calldata task,
        uint64 interval,
        uint128 budgetWei,
        uint32 maxRuns
    ) external returns (uint256 childJobId);
    function childMetaOf(uint256 id) external view returns (uint256 parentId, uint64 depth, uint256 rootId);
    function activeJobCountOf(address owner) external view returns (uint256);
}

/// Hardens the LIVE ScheduleFacet
/// (`0x231A33C67Fc11CC3ebEe38F6A45462f4C707283A`) with two changes:
///   #3 — a per-owner active-job cap (`MAX_ACTIVE_JOBS_PER_OWNER = 32`)
///        via a NEW appended `activeJobsOf` mapping. `scheduleJob`
///        increments + reverts `TooManyActiveJobs` at the cap; `recordRun`
///        (on exhaust) and `cancelJob` decrement. (Forward-looking: live
///        jobs predate the counter → it starts at 0; see the storage doc.)
///   #4 — `scheduleChildJob` (scheduler-only): MOVES budget out of a
///        parent job's escrow into a fresh child (no mint / no transfer —
///        the `$LH` is already in the diamond), so the ROOT's original
///        budget caps the whole recursive tree. Child-tree metadata lives
///        in a NEW appended `childMeta` mapping (NOT new `Job` fields).
///
/// STORAGE IS APPEND-ONLY: the two new members (`activeJobsOf`,
/// `childMeta`) sit at the END of `LibScheduleStorage.Storage`; the live
/// `Job` struct + every prior member are byte-for-byte unchanged, so the
/// existing jobs' storage is not corrupted.
///
/// CUT SHAPE: deploy ONE new facet; Replace the three selectors whose
/// LOGIC changed (`scheduleJob`/`recordRun`/`cancelJob`) so the whole
/// facet stays coherent at one address, and Add the new selectors
/// (`scheduleChildJob` + the two new views). The untouched view/control
/// selectors (`pauseJob`/`resumeJob`/`topUpJob`/`setScheduler`/`jobsDue`/
/// `getJob`/`taskOf`/`jobsOf`/`jobCount`/`schedulerAddress`) keep pointing
/// at the prior deployment — they are NOT re-cut here (a blanket Replace
/// of all 13 would be fine too, but Replacing only the changed three keeps
/// the diff minimal). All new code paths are inside the freshly-deployed
/// facet, so the Replaced functions reach the new `_writeJob` helper.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddScheduleHardening.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract AddScheduleHardening is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ScheduleFacet f = new ScheduleFacet();

        // REPLACE the three selectors whose logic changed (same sigs,
        // read via the typed interface — no magic literals).
        bytes4[] memory replaced = new bytes4[](3);
        replaced[0] = IScheduleLive.scheduleJob.selector;
        replaced[1] = IScheduleLive.recordRun.selector;
        replaced[2] = IScheduleLive.cancelJob.selector;

        // ADD the new selectors (recursion + the two new views).
        bytes4[] memory added = new bytes4[](3);
        added[0] = IScheduleLive.scheduleChildJob.selector;
        added[1] = IScheduleLive.childMetaOf.selector;
        added[2] = IScheduleLive.activeJobCountOf.selector;

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

        console.log("--- ScheduleFacet hardening cut ---");
        console.log("diamond:           ", diamond);
        console.log("newScheduleFacet:  ", address(f));
        console.log("REPLACED (logic changed for activeJobsOf counter):");
        console.logBytes4(replaced[0]); // scheduleJob
        console.logBytes4(replaced[1]); // recordRun
        console.logBytes4(replaced[2]); // cancelJob
        console.log("ADDED (recursion + views):");
        console.logBytes4(added[0]); // scheduleChildJob
        console.logBytes4(added[1]); // childMetaOf
        console.logBytes4(added[2]); // activeJobCountOf
    }
}
