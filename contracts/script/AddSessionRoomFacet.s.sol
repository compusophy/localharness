// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Script, console} from "forge-std/Script.sol";

import {SessionRoomFacet} from "../src/facets/SessionRoomFacet.sol";
import {IDiamond} from "../src/interfaces/IDiamond.sol";
import {IDiamondCut} from "../src/interfaces/IDiamondCut.sol";

/// GitHub #22: cut SessionRoomFacet — member-gated, append-only logs of
/// encrypted key/value ops (shared agent state; CRDT folds off-chain in
/// `src/kv_reduce.rs`, sealing in `src/kv_room.rs`). Self-contained: no
/// dependency on other facets' storage. Selectors are `room`-prefixed where a
/// bare name would collide (TeamFacet/GuildFacet `membersOf`).
///
/// Run with:
///   DIAMOND=0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c \
///   EVM_PRIVATE_KEY=0x<diamond owner key> \
///   forge script script/AddSessionRoomFacet.s.sol --rpc-url tempo_moderato --broadcast
contract AddSessionRoomFacet is Script {
    function run() external {
        address diamond = vm.envAddress("DIAMOND");
        uint256 pk = vm.envUint("EVM_PRIVATE_KEY");

        vm.startBroadcast(pk);

        SessionRoomFacet f = new SessionRoomFacet();

        bytes4[] memory selectors = new bytes4[](11);
        selectors[0] = SessionRoomFacet.createRoom.selector;
        selectors[1] = SessionRoomFacet.roomAddMember.selector;
        selectors[2] = SessionRoomFacet.roomRemoveMember.selector;
        selectors[3] = SessionRoomFacet.appendOp.selector;
        selectors[4] = SessionRoomFacet.clearRoom.selector;
        selectors[5] = SessionRoomFacet.opsOf.selector;
        selectors[6] = SessionRoomFacet.opCount.selector;
        selectors[7] = SessionRoomFacet.roomEpoch.selector;
        selectors[8] = SessionRoomFacet.roomCreator.selector;
        selectors[9] = SessionRoomFacet.roomIsMember.selector;
        selectors[10] = SessionRoomFacet.roomMembersOf.selector;

        IDiamond.FacetCut[] memory cuts = new IDiamond.FacetCut[](1);
        cuts[0] = IDiamond.FacetCut({
            facetAddress: address(f),
            action: IDiamond.FacetCutAction.Add,
            functionSelectors: selectors
        });
        IDiamondCut(diamond).diamondCut(cuts, address(0), "");

        vm.stopBroadcast();

        console.log("--- SessionRoomFacet cut ---");
        console.log("diamond:           ", diamond);
        console.log("sessionRoomFacet:  ", address(f));
    }
}
