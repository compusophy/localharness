// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

/// @dev Isolated diamond storage for SessionRoom (GitHub #22): member-gated,
///      append-only logs of ENCRYPTED key/value ops. Fresh slot. Add new fields
///      ONLY at the end of the struct.
///
///      The facet is a dumb, opaque op-log: it stores ciphertext blobs and
///      enforces WHO may append/clear. All KV/CRDT/crypto semantics live
///      off-chain (`src/kv_reduce.rs` + `src/kv_room.rs`); the chain never sees
///      plaintext or key material. `Op` is `(address, uint64, bytes)` —
///      identical to SignalingFacet's `Signal`, so the same off-chain decoder
///      reads it.
library LibSessionRoomStorage {
    bytes32 constant POSITION = keccak256("localharness.sessionroom.storage.v1");

    /// One appended op: who wrote it (`msg.sender`, the off-chain envelope binds
    /// to this), when, and the opaque sealed blob.
    struct Op {
        address writer;
        uint64 ts;
        bytes blob;
    }

    /// A room's metadata. `epoch` bumps on every `clearRoom` so off-chain
    /// readers detect a reset and re-poll from index 0.
    struct Room {
        address creator;
        bool exists;
        uint64 epoch;
    }

    struct Storage {
        /// Monotonic room id counter; first room is id 1 (0 == "no room").
        uint256 nextRoomId;
        /// roomId => room metadata.
        mapping(uint256 => Room) rooms;
        /// roomId => member => can-write. Creator is a member on create.
        mapping(uint256 => mapping(address => bool)) isMember;
        /// roomId => enumerable member list (for `roomMembersOf`).
        mapping(uint256 => address[]) memberList;
        /// roomId => append-only op log. Array index is the reader's cursor.
        mapping(uint256 => Op[]) ops;
    }

    function load() internal pure returns (Storage storage s) {
        bytes32 position = POSITION;
        assembly {
            s.slot := position
        }
    }
}
