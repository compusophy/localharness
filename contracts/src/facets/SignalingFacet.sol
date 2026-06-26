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

    // --- Presence / discovery (ephemeral-key model, per TOPIC) ------------
    // Peers can't always address each other by identity (an owner's devices
    // share one master address). So each peer generates an EPHEMERAL signaling
    // key per session and announces it under a TOPIC — a SignalingFacet room:
    // `keccak256("localharness.devices", owner)` for your own devices, or
    // `keccak256("localharness.team", teamId)` for an agent team
    // (membership/consent lives in `TeamFacet`). Peers read the topic to
    // discover each other, then `postSignal` to the ephemeral address.
    //
    // AUTH (devices topic — the live "sync my devices" path). An UNGATED
    // `announce` is a MITM hole: the devices topic is PUBLIC (anyone can derive
    // `keccak256("localharness.devices" || owner)` from an address), so an
    // attacker could announce a self-chosen pubkey under the victim's roster,
    // receive the victim's SDP offer sealed to the ATTACKER's key, complete the
    // WebRTC handshake, and pull the whole shared folder over the (peer-unauthed)
    // union protocol. We close it by binding the announcement to the OWNER's
    // seed key: since device-linking shares ONE seed across the user's devices,
    // only the seed holder can produce a valid signature, so only a real device
    // can join the roster. `announce` requires:
    //   (a) topic == keccak256(abi.encodePacked("localharness.devices", owner))
    //       — recomputed on-chain to match `registry::devices_topic`; and
    //   (b) recover(keccak256(abi.encodePacked(block.chainid, address(this),
    //       topic, ephemeral, pubkey)), sig) == owner  (the owner's key signed
    //       THIS announcement). The chainId + diamond address bind the
    //       signature to THIS deployment (no cross-chain / cross-diamond
    //       replay), mirroring the x402 EIP-712 domain separator.
    // Reject high-s (EIP-2), mirroring X402Facet._isValidSignature.
    //
    // TEAM topics: the analogous gate (sig by a team MEMBER, verified against
    // `TeamFacet.isMember(teamId, signer)`) is OUT OF THIS SCOPE — teams aren't
    // live-used yet. For now a team-topic announce (any topic that is NOT the
    // caller-supplied owner's devices topic) MUST still be self-consistent:
    // `recover(...) == ephemeral` (the ephemeral key signed its own
    // announcement). That kills the trivial "announce a victim's address with my
    // pubkey" substitution; full member-gating is a follow-up when teams ship.

    event Announced(bytes32 indexed topic, address indexed ephemeral);

    error BadTopic();
    error Unauthorized();

    // secp256k1n / 2 — reject high-s signatures (EIP-2 malleability).
    uint256 private constant HALF_N =
        0x7FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF5D576E7357A4501DDFE92F46681B20A0;

    /// Announce (or refresh) `ephemeral` + its `pubkey` under `topic`, signed by
    /// `owner` (the seed holder). Idempotent upsert keyed by `ephemeral`.
    ///
    /// Auth (see the block comment above):
    ///   - DEVICES topic: `topic` MUST equal
    ///     `keccak256(abi.encodePacked("localharness.devices", owner))` and `sig`
    ///     MUST recover to `owner` over
    ///     `keccak256(abi.encodePacked(block.chainid, address(this), topic, ephemeral, pubkey))`.
    ///   - any OTHER topic (team / future): `sig` must recover to `ephemeral`
    ///     (self-consistency floor; full member-gating is a follow-up).
    ///
    /// The digest binds `block.chainid` + the diamond address so a captured
    /// signature can't be replayed against another chain or deployment.
    function announce(
        bytes32 topic,
        address owner,
        address ephemeral,
        bytes calldata pubkey,
        bytes calldata sig
    ) external {
        bytes32 digest =
            keccak256(abi.encodePacked(block.chainid, address(this), topic, ephemeral, pubkey));
        bytes32 devicesTopic =
            keccak256(abi.encodePacked("localharness.devices", owner));
        if (topic == devicesTopic) {
            // Owner-gated: only the seed holder can populate the devices roster.
            if (owner == address(0)) revert Unauthorized();
            if (_recover(digest, sig) != owner) revert Unauthorized();
        } else {
            // Non-devices (team / future) topic: self-consistency floor — the
            // ephemeral key must have signed its own announcement. Full
            // member-gating (vs TeamFacet.isMember) is out of scope here.
            if (_recover(digest, sig) != ephemeral) revert Unauthorized();
        }

        LibSignalingStorage.Presence[] storage r = LibSignalingStorage.load().roster[topic];
        for (uint256 i = 0; i < r.length; i++) {
            if (r[i].ephemeral == ephemeral) {
                r[i].ts = uint64(block.timestamp);
                r[i].pubkey = pubkey;
                emit Announced(topic, ephemeral);
                return;
            }
        }
        r.push(
            LibSignalingStorage.Presence({
                ephemeral: ephemeral,
                ts: uint64(block.timestamp),
                pubkey: pubkey
            })
        );
        emit Announced(topic, ephemeral);
    }

    /// ecrecover an `r‖s‖v` (65-byte) signature over `digest`; rejects high-s
    /// (EIP-2) and bad `v`. Returns `address(0)` on any malformed/failed
    /// recovery (callers compare against the expected signer). Mirrors
    /// `X402Facet._isValidSignature` (EOA path).
    function _recover(bytes32 digest, bytes calldata sig) internal pure returns (address) {
        if (sig.length != 65) return address(0);
        bytes32 r;
        bytes32 s;
        uint8 v;
        assembly {
            r := calldataload(sig.offset)
            s := calldataload(add(sig.offset, 32))
            v := byte(0, calldataload(add(sig.offset, 64)))
        }
        if (uint256(s) > HALF_N) return address(0); // reject high-s
        if (v < 27) v += 27;
        if (v != 27 && v != 28) return address(0);
        return ecrecover(digest, v, r, s);
    }

    /// The ephemeral signaling keys announced under `topic`. Readers filter
    /// stale entries by `ts` off-chain. View — no gas.
    function peersOf(bytes32 topic)
        external
        view
        returns (LibSignalingStorage.Presence[] memory)
    {
        return LibSignalingStorage.load().roster[topic];
    }

    /// Drop a no-longer-online `ephemeral` from `topic`'s roster (swap-pop).
    ///
    /// Auth — mirrors `announce` (an UNGATED `leave` is the same MITM hole in
    /// reverse: anyone could evict any device from any roster, defeating the
    /// owner-gated integrity property `announce` establishes — e.g. kick the
    /// victim's real device out so only an attacker's lingering entry remains).
    /// `sig` is taken over a DOMAIN-SEPARATED digest
    /// `keccak256(abi.encodePacked("localharness.leave", block.chainid,
    /// address(this), topic, ephemeral))` — the `localharness.leave` prefix
    /// prevents replaying an `announce` signature, and the chainId + diamond
    /// address bind it to THIS deployment (no cross-chain replay):
    ///   - DEVICES topic: `topic` MUST equal
    ///     `keccak256(abi.encodePacked("localharness.devices", owner))` and `sig`
    ///     MUST recover to `owner` (only the seed holder edits the roster).
    ///   - any OTHER topic (team / future): `sig` must recover to `ephemeral`
    ///     (self-control floor — a device removes only itself), matching
    ///     `announce`'s non-devices branch.
    /// Reject high-s (EIP-2) via `_recover`, like `announce`.
    function leave(bytes32 topic, address owner, address ephemeral, bytes calldata sig) external {
        bytes32 digest = keccak256(
            abi.encodePacked("localharness.leave", block.chainid, address(this), topic, ephemeral)
        );
        bytes32 devicesTopic =
            keccak256(abi.encodePacked("localharness.devices", owner));
        if (topic == devicesTopic) {
            if (owner == address(0)) revert Unauthorized();
            if (_recover(digest, sig) != owner) revert Unauthorized();
        } else {
            if (_recover(digest, sig) != ephemeral) revert Unauthorized();
        }

        LibSignalingStorage.Presence[] storage r = LibSignalingStorage.load().roster[topic];
        uint256 n = r.length;
        for (uint256 i = 0; i < n; i++) {
            if (r[i].ephemeral == ephemeral) {
                if (i != n - 1) {
                    r[i] = r[n - 1];
                }
                r.pop();
                return;
            }
        }
    }
}
