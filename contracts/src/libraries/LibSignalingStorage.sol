// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated storage for the WebRTC signaling mailbox. Diamond storage
///      pattern — fresh slot. Add new fields ONLY at the end of the struct.
library LibSignalingStorage {
    bytes32 constant POSITION = keccak256("localharness.signaling.storage.v1");

    /// One signaling message: who posted it, when, and the opaque blob (an
    /// SDP offer/answer or an ICE bundle, app-encrypted to the recipient's
    /// device key — the chain never sees plaintext).
    struct Signal {
        address from;
        uint64 ts;
        bytes blob;
    }

    struct Storage {
        /// recipient device address => its pending inbox (append-only until
        /// the recipient `clearInbox`es). Index in this array is the cursor a
        /// reader tracks off-chain (the `index` returned by `postSignal` /
        /// emitted by `Signaled`).
        mapping(address => Signal[]) inbox;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
