// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {SignalingFacet} from "../src/facets/SignalingFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// Old `leave` interface (signature CHANGED) — used only to derive the 4-byte
/// selector that must be REMOVED from the diamond. Keeping it as a typed
/// interface (instead of a hardcoded `bytes4` literal) makes the preimage
/// self-documenting and drift-proof. Mirrors `ReplaceSignalingAnnounce.s.sol`.
interface IOldSignalingLeave {
    function leave(bytes32 topic, address ephemeral) external;
}

/// Migrate `SignalingFacet.leave` from the OLD UNAUTHENTICATED 2-arg form to the
/// owner/self-signed 4-arg form (`leave(bytes32,address,address,bytes)`) — issue
/// #94: an ungated `leave` let anyone evict any device from any roster,
/// defeating the owner-gated integrity property `announce` establishes. Because
/// the selector CHANGES, this is a Remove (old selector) + Add (new selector)
/// cut; we also Replace the 6 unchanged selectors onto the freshly deployed
/// facet so ALL of SignalingFacet lives in one deployment afterward (no stale
/// facet left half-wired) — same shape as the announce migration.
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/ReplaceSignalingLeave.s.sol \
///       --rpc-url tempo_moderato --broadcast
contract ReplaceSignalingLeave is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        SignalingFacet f = new SignalingFacet();

        // 1) REMOVE the old (unauthenticated) leave selector. Remove cuts MUST
        //    carry facetAddress == address(0).
        bytes4[] memory removeSel = new bytes4[](1);
        removeSel[0] = IOldSignalingLeave.leave.selector;

        // 2) ADD the new auth-gated leave selector.
        bytes4[] memory addSel = new bytes4[](1);
        addSel[0] = SignalingFacet.leave.selector;

        // 3) REPLACE the 6 unchanged selectors onto the new facet deployment so
        //    the whole facet is consolidated (their bytecode is identical).
        bytes4[] memory replaceSel = new bytes4[](6);
        replaceSel[0] = SignalingFacet.postSignal.selector;
        replaceSel[1] = SignalingFacet.inboxOf.selector;
        replaceSel[2] = SignalingFacet.inboxLength.selector;
        replaceSel[3] = SignalingFacet.clearInbox.selector;
        replaceSel[4] = SignalingFacet.announce.selector;
        replaceSel[5] = SignalingFacet.peersOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](3);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(0),
            action: IDiamond.FacetCutAction.Remove,
            functionSelectors: removeSel
        });
        cuts[1] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: addSel
        });
        cuts[2] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Replace,
            functionSelectors: replaceSel
        });

        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- SignalingFacet.leave migrated to auth-gated ---");
        console.log("diamond:        ", diamond);
        console.log("new facet:      ", address(f));
        console.logBytes4(removeSel[0]); // old leave selector removed
        console.logBytes4(addSel[0]);    // new leave selector added
    }
}
