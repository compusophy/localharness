// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibSignalingStorage} from "../libraries/LibSignalingStorage.sol";

/// @title SignalingFacet
/// @notice On-chain WebRTC SIGNALING mailbox — exchange SDP offers/answers +
///         ICE candidates WITHOUT a signaling server. A device posts an
///         opaque, peer-encrypted blob addressed to another device; the
///         recipient polls its inbox (via the `Signaled` event or `inboxOf`),
///         decrypts, and replies. This is the transport that makes the
///         cross-device shared-folder sync (WebRTC P2P) serverless for
///         signaling — the only remaining external dependency is a public STUN
///         server for reflexive ICE candidates (free + ubiquitous), and
///         optionally TURN for the ~20-30% of NATs that need a relay.
///
///         DISCOVERY is deliberately NOT here: the peer set is already
///         `DeviceRegistryFacet.devicesOf(mainId)` (the owner's linked
///         devices). This facet is purely the offer/answer/ICE channel between
///         two device addresses that already know each other.
///
///         PRACTICALITY: keep a connection to ~2 posts by using NON-TRICKLE
///         ICE — gather candidates locally and fold them into ONE SDP, so a
///         link costs ~2 sponsored txs (offer + answer), not one-per-candidate.
///         Blobs are opaque + recipient-encrypted, so no auth gate is needed
///         (the recipient validates the sender out-of-band against
///         DeviceRegistry); spam is bounded by gas.
///
///         CUTTING IT (diamond owner; mirror script/AddTbaFacet.s.sol):
///         deploy + diamondCut Add [postSignal(address,bytes),
///         inboxOf(address,uint256), inboxLength(address), clearInbox()].
contract SignalingFacet {
    /// Emitted on every post so a recipient can react without polling state.
    /// `index` is the position in the recipient's inbox (the reader's cursor);
    /// a post with `index == type(uint256).max` is a `clearInbox` marker.
    event Signaled(address indexed to, address indexed from, uint256 index);

    /// Post an opaque signaling blob (an SDP offer/answer or ICE bundle,
    /// app-encrypted to `to`'s device key) addressed to `to`. Returns the
    /// blob's index in the recipient's inbox.
    function postSignal(address to, bytes calldata blob) external returns (uint256 index) {
        LibSignalingStorage.Signal[] storage box = LibSignalingStorage.load().inbox[to];
        index = box.length;
        box.push(
            LibSignalingStorage.Signal({from: msg.sender, ts: uint64(block.timestamp), blob: blob})
        );
        emit Signaled(to, msg.sender, index);
    }

    /// Read `peer`'s signals from `fromIndex` onward (the reader tracks its own
    /// cursor off-chain). View — no gas, no tx.
    function inboxOf(address peer, uint256 fromIndex)
        external
        view
        returns (LibSignalingStorage.Signal[] memory out)
    {
        LibSignalingStorage.Signal[] storage box = LibSignalingStorage.load().inbox[peer];
        uint256 n = box.length;
        if (fromIndex >= n) {
            return new LibSignalingStorage.Signal[](0);
        }
        out = new LibSignalingStorage.Signal[](n - fromIndex);
        for (uint256 i = fromIndex; i < n; i++) {
            out[i - fromIndex] = box[i];
        }
    }

    function inboxLength(address peer) external view returns (uint256) {
        return LibSignalingStorage.load().inbox[peer].length;
    }

    /// Recipient-only: drop the whole inbox to reclaim storage (gas refund)
    /// once everything is read + applied. Re-poll from index 0 afterward.
    function clearInbox() external {
        delete LibSignalingStorage.load().inbox[msg.sender];
        emit Signaled(msg.sender, msg.sender, type(uint256).max);
    }
}
