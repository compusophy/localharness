// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {LibSessionRoomStorage} from "../libraries/LibSessionRoomStorage.sol";

/// @title SessionRoomFacet
/// @notice Member-gated, append-only logs of ENCRYPTED key/value ops — the
///         on-chain substrate for shared agent state (GitHub #22). An agent
///         persists state across turns/devices (or shares it with enrolled
///         members) by appending sealed ops here instead of re-sending full
///         context; peers fold the log into a converged map off-chain
///         (`src/kv_reduce.rs`). The chain stores only opaque ciphertext
///         (`src/kv_room.rs` seals/opens it) and enforces who may write.
///
///         Self-contained: a room has a creator and a member set, managed by
///         the creator. Reads are OPEN (blobs are undecryptable without the
///         off-chain room key); authorization that matters is on the WRITES.
///
///         Selectors are `room`-prefixed where a bare name would collide with
///         another facet on the diamond (e.g. TeamFacet/GuildFacet `membersOf`)
///         — a diamond cannot share a selector.
contract SessionRoomFacet {
    event RoomCreated(uint256 indexed roomId, address indexed creator);
    event MemberAdded(uint256 indexed roomId, address indexed member);
    event MemberRemoved(uint256 indexed roomId, address indexed member);
    event OpAppended(uint256 indexed roomId, address indexed writer, uint256 index);
    event RoomCleared(uint256 indexed roomId, uint64 epoch);

    error NoSuchRoom();
    error NotRoomCreator();
    error NotRoomMember();
    error EmptyBlob();
    error BlobTooLarge(uint256 length);

    /// Same cap as FeedbackFacet — KV state is small shared state by design;
    /// the per-byte gas cost is the real spam filter.
    uint256 private constant MAX_BLOB = 2048;

    // --- room lifecycle -----------------------------------------------------

    /// Create a room. Caller is the creator and first member. Returns the id.
    function createRoom() external returns (uint256 roomId) {
        LibSessionRoomStorage.Storage storage s = LibSessionRoomStorage.load();
        roomId = ++s.nextRoomId;
        s.rooms[roomId] = LibSessionRoomStorage.Room({creator: msg.sender, exists: true, epoch: 0});
        s.isMember[roomId][msg.sender] = true;
        s.memberList[roomId].push(msg.sender);
        emit RoomCreated(roomId, msg.sender);
        emit MemberAdded(roomId, msg.sender);
    }

    /// Creator-only: enroll `member` as a writer (idempotent).
    function roomAddMember(uint256 roomId, address member) external {
        LibSessionRoomStorage.Storage storage s = LibSessionRoomStorage.load();
        _onlyCreator(s, roomId);
        if (!s.isMember[roomId][member]) {
            s.isMember[roomId][member] = true;
            s.memberList[roomId].push(member);
            emit MemberAdded(roomId, member);
        }
    }

    /// Creator-only: revoke `member`'s write access. The creator cannot be
    /// removed (a room always has its creator). On-chain revocation stops
    /// future writes; rotating the off-chain key is a separate concern.
    function roomRemoveMember(uint256 roomId, address member) external {
        LibSessionRoomStorage.Storage storage s = LibSessionRoomStorage.load();
        _onlyCreator(s, roomId);
        if (member == s.rooms[roomId].creator) revert NotRoomCreator();
        if (s.isMember[roomId][member]) {
            s.isMember[roomId][member] = false;
            address[] storage list = s.memberList[roomId];
            uint256 n = list.length;
            for (uint256 i = 0; i < n; i++) {
                if (list[i] == member) {
                    list[i] = list[n - 1];
                    list.pop();
                    break;
                }
            }
            emit MemberRemoved(roomId, member);
        }
    }

    // --- writes (member-gated) ---------------------------------------------

    /// Append a sealed op to the room log. Caller must be a member. Returns the
    /// op's index (the reader's cursor).
    function appendOp(uint256 roomId, bytes calldata blob) external returns (uint256 index) {
        LibSessionRoomStorage.Storage storage s = LibSessionRoomStorage.load();
        if (!s.rooms[roomId].exists) revert NoSuchRoom();
        if (!s.isMember[roomId][msg.sender]) revert NotRoomMember();
        uint256 len = blob.length;
        if (len == 0) revert EmptyBlob();
        if (len > MAX_BLOB) revert BlobTooLarge(len);
        LibSessionRoomStorage.Op[] storage box = s.ops[roomId];
        index = box.length;
        box.push(LibSessionRoomStorage.Op({writer: msg.sender, ts: uint64(block.timestamp), blob: blob}));
        emit OpAppended(roomId, msg.sender, index);
    }

    /// Creator-only: drop the whole op log (reclaim storage / gas refund) and
    /// bump the epoch so readers re-poll from 0. This is the reclamation a
    /// reused signaling inbox can't give a synthetic room address.
    function clearRoom(uint256 roomId) external returns (uint64 epoch) {
        LibSessionRoomStorage.Storage storage s = LibSessionRoomStorage.load();
        _onlyCreator(s, roomId);
        delete s.ops[roomId];
        epoch = ++s.rooms[roomId].epoch;
        emit RoomCleared(roomId, epoch);
    }

    // --- reads (open; blobs are ciphertext) --------------------------------

    /// `roomId`'s ops from `fromIndex` onward (reader tracks its own cursor).
    function opsOf(uint256 roomId, uint256 fromIndex)
        external
        view
        returns (LibSessionRoomStorage.Op[] memory out)
    {
        LibSessionRoomStorage.Op[] storage box = LibSessionRoomStorage.load().ops[roomId];
        uint256 n = box.length;
        if (fromIndex >= n) return new LibSessionRoomStorage.Op[](0);
        out = new LibSessionRoomStorage.Op[](n - fromIndex);
        for (uint256 i = fromIndex; i < n; i++) {
            out[i - fromIndex] = box[i];
        }
    }

    function opCount(uint256 roomId) external view returns (uint256) {
        return LibSessionRoomStorage.load().ops[roomId].length;
    }

    function roomEpoch(uint256 roomId) external view returns (uint64) {
        return LibSessionRoomStorage.load().rooms[roomId].epoch;
    }

    function roomCreator(uint256 roomId) external view returns (address) {
        return LibSessionRoomStorage.load().rooms[roomId].creator;
    }

    function roomIsMember(uint256 roomId, address who) external view returns (bool) {
        return LibSessionRoomStorage.load().isMember[roomId][who];
    }

    function roomMembersOf(uint256 roomId) external view returns (address[] memory) {
        return LibSessionRoomStorage.load().memberList[roomId];
    }

    // --- internal ----------------------------------------------------------

    function _onlyCreator(LibSessionRoomStorage.Storage storage s, uint256 roomId) private view {
        if (!s.rooms[roomId].exists) revert NoSuchRoom();
        if (s.rooms[roomId].creator != msg.sender) revert NotRoomCreator();
    }
}
