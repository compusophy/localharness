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

    /// A device's announced presence: the EPHEMERAL signaling key it generated
    /// for this sync session (a throwaway, NOT the master). Siblings discover
    /// each other by reading the owner's roster, then signal to `ephemeral`
    /// (ECIES-sealed to `pubkey`). `ts` lets readers ignore stale entries.
    struct Presence {
        address ephemeral;
        uint64 ts;
        bytes pubkey;
    }

    struct Storage {
        /// recipient device address => its pending inbox (append-only until
        /// the recipient `clearInbox`es). Index in this array is the cursor a
        /// reader tracks off-chain (the `index` returned by `postSignal` /
        /// emitted by `Signaled`).
        mapping(address => Signal[]) inbox;
        /// TOPIC => the ephemeral signaling keys its online peers have
        /// announced. A topic is a SignalingFacet room: `keccak256("devices",
        /// owner)` for one owner's own devices, or `keccak256("team", teamId)`
        /// for an agent TEAM (the consent layer is `TeamFacet`). This is what
        /// makes the same P2P transport serve both "sync my devices" and
        /// "sync with my teammates".
        mapping(bytes32 => Presence[]) roster;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
