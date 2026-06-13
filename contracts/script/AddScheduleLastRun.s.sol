// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {ScheduleFacet} from "../src/facets/ScheduleFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// GitHub #52: surface a job's LAST-RUN timestamp + status so the `jobs` /
/// `status` UIs can show "last run: <when> [status]" instead of leaving the
/// owner unable to tell whether a due cron fire actually ran or silently
/// failed. `recordRun` now stamps a NEW appended `lastRunRecord` mapping
/// (packed `ts << 8 | status`); the new `lastRunOf` view reads it.
///
/// STORAGE APPEND-ONLY: `lastRunRecord` sits at the END of
/// `LibScheduleStorage.Storage` (after the hardening cut's `activeJobsOf` /
/// `childMeta`); the live `Job` struct + every prior member are byte-for-byte
/// unchanged, so existing jobs' storage is untouched.
///
/// CUT SHAPE: deploy ONE new ScheduleFacet; REPLACE `recordRun` (same
/// signature, new logic — it now writes `lastRunRecord`) and ADD `lastRunOf`.
/// The untouched selectors keep pointing at the prior deployment; storage is
/// the shared `LibScheduleStorage` slot, so the split is coherent (same
/// minimal-Replace discipline as AddScheduleHardening).
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddScheduleLastRun.s.sol --rpc-url tempo_moderato --broadcast
interface IScheduleLive {
    // REPLACED: same signature, new logic (now stamps lastRunRecord).
    function recordRun(uint256 id, uint64 expectedNextRun, uint128 spentWei) external returns (uint64 newNextRun);
    // ADDED: the new last-run view.
    function lastRunOf(uint256 id) external view returns (uint64 timestamp, uint8 status);
}

contract AddScheduleLastRun is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        ScheduleFacet f = new ScheduleFacet();

        bytes4[] memory replaced = new bytes4[](1);
        replaced[0] = IScheduleLive.recordRun.selector;

        bytes4[] memory added = new bytes4[](1);
        added[0] = IScheduleLive.lastRunOf.selector;

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

        console.log("--- ScheduleFacet #52 last-run cut ---");
        console.log("diamond:          ", diamond);
        console.log("newScheduleFacet: ", address(f));
        console.log("REPLACED recordRun:");
        console.logBytes4(replaced[0]);
        console.log("ADDED lastRunOf:");
        console.logBytes4(added[0]);
    }
}
