// SPDX-License-Identifier: MIT
pragma solidity ^0.8.20;

import {Test} from "forge-std/Test.sol";
import {SignalingFacet} from "../src/facets/SignalingFacet.sol";
import {LibSignalingStorage} from "../src/libraries/LibSignalingStorage.sol";

/// Owner-signed `announce` auth tests. The facet is exercised directly —
/// `LibSignalingStorage.load()` resolves against THIS deployment's storage, so
/// `peersOf` reads back exactly what `announce` wrote (same pattern as
/// InviteFacet.t.sol). We prove:
///   - a valid OWNER-signed announce lands in `peersOf` (devices topic)
///   - a WRONG signer (attacker) reverts `Unauthorized`
///   - a mismatched topic (not the owner's devices topic) reverts
///   - a high-s signature reverts (EIP-2)
///   - format edges: bad length / bad v / re-announce upsert / leave
contract SignalingAuthTest is Test {
    SignalingFacet sig;

    uint256 ownerPk;
    address owner;
    uint256 attackerPk;
    address attacker;

    // secp256k1 group order N (for crafting a high-s twin: s' = N - s).
    uint256 constant N =
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141;

    function setUp() public {
        sig = new SignalingFacet();
        ownerPk = 0xA11CE;
        owner = vm.addr(ownerPk);
        attackerPk = 0xB0B;
        attacker = vm.addr(attackerPk);
    }

    /// The on-chain devices topic for `who` — MUST match
    /// `SignalingFacet.announce`'s `keccak256(abi.encodePacked("localharness.devices", who))`
    /// AND `registry::devices_topic` (b"localharness.devices" || addr).
    function _devicesTopic(address who) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked("localharness.devices", who));
    }

    /// The digest a signer authorizes: keccak256(topic || ephemeral || pubkey),
    /// matching the facet AND `registry::announce_digest`.
    function _digest(bytes32 topic, address eph, bytes memory pubkey)
        internal
        pure
        returns (bytes32)
    {
        return keccak256(abi.encodePacked(topic, eph, pubkey));
    }

    /// The digest a signer authorizes to evict `eph` from `topic`:
    /// keccak256("localharness.leave" || topic || ephemeral) — DOMAIN-SEPARATED
    /// from `_digest` so an `announce` sig can't be replayed as a `leave`.
    function _leaveDigest(bytes32 topic, address eph) internal pure returns (bytes32) {
        return keccak256(abi.encodePacked("localharness.leave", topic, eph));
    }

    /// Sign `digest` with `pk` and pack r‖s‖v (65 bytes), v in {27,28} — the
    /// layout `_recover` (and `wallet::sign_hash`) use.
    function _sign(uint256 pk, bytes32 digest) internal pure returns (bytes memory) {
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(pk, digest);
        return abi.encodePacked(r, s, v);
    }

    function _pubkey(address eph) internal pure returns (bytes memory) {
        // A 33-byte compressed-pubkey-shaped blob; content is opaque to the
        // facet (it only signs/recovers over the bytes).
        return abi.encodePacked(hex"02", bytes32(uint256(uint160(eph))));
    }

    // --- happy path ------------------------------------------------------

    function test_valid_owner_sig_announces_and_appears() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0xE9E9);
        bytes memory pubkey = _pubkey(eph);
        bytes memory s = _sign(ownerPk, _digest(topic, eph, pubkey));

        sig.announce(topic, owner, eph, pubkey, s);

        LibSignalingStorage.Presence[] memory r = sig.peersOf(topic);
        assertEq(r.length, 1, "one roster entry");
        assertEq(r[0].ephemeral, eph, "ephemeral stored");
        assertEq(r[0].pubkey, pubkey, "pubkey stored");
        assertEq(r[0].ts, uint64(block.timestamp), "ts stamped");
    }

    // --- attacker (wrong signer) ----------------------------------------

    function test_wrong_signer_reverts() public {
        bytes32 topic = _devicesTopic(owner); // victim's PUBLIC devices topic
        address eph = address(0xBAD);
        bytes memory pubkey = _pubkey(eph);
        // Attacker signs the same digest with THEIR key — recovers to attacker,
        // not owner.
        bytes memory s = _sign(attackerPk, _digest(topic, eph, pubkey));

        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.announce(topic, owner, eph, pubkey, s);

        assertEq(sig.peersOf(topic).length, 0, "roster untouched");
    }

    /// Attacker tries to pass `owner = attacker` (so the sig recovers), but then
    /// the topic no longer matches `devices_topic(attacker)` for the VICTIM's
    /// roster — they can only ever announce under their OWN topic, never the
    /// victim's. Proven by: announcing the victim's topic with owner=attacker
    /// fails the topic check (falls to the non-devices branch, needs sig==eph).
    function test_attacker_cannot_claim_victim_topic_via_own_owner() public {
        bytes32 victimTopic = _devicesTopic(owner);
        address eph = vm.addr(0xC0FFEE);
        bytes memory pubkey = _pubkey(eph);
        // owner field = attacker; sign with attacker so it'd recover to attacker.
        // victimTopic != devices_topic(attacker) → non-devices branch →
        // requires sig==ephemeral, which attacker's sig is NOT.
        bytes memory s = _sign(attackerPk, _digest(victimTopic, eph, pubkey));

        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.announce(victimTopic, attacker, eph, pubkey, s);
        assertEq(sig.peersOf(victimTopic).length, 0, "victim roster untouched");
    }

    // --- mismatched topic -----------------------------------------------

    function test_mismatched_topic_reverts() public {
        // A garbage topic that is NOT owner's devices topic. Owner signs it, so
        // the sig recovers to owner — but owner != ephemeral, so the
        // non-devices self-consistency branch rejects it.
        bytes32 topic = keccak256("not-a-devices-topic");
        address eph = address(0x1234);
        bytes memory pubkey = _pubkey(eph);
        bytes memory s = _sign(ownerPk, _digest(topic, eph, pubkey));

        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.announce(topic, owner, eph, pubkey, s);
    }

    // --- high-s (EIP-2) --------------------------------------------------

    function test_high_s_reverts() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0xACE);
        bytes memory pubkey = _pubkey(eph);
        bytes32 digest = _digest(topic, eph, pubkey);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(ownerPk, digest);
        // Malleate to the high-s twin: s' = N - s, v flips.
        bytes32 sHigh = bytes32(N - uint256(s));
        uint8 vFlip = v == 27 ? 28 : 27;
        bytes memory sigHigh = abi.encodePacked(r, sHigh, vFlip);

        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.announce(topic, owner, eph, pubkey, sigHigh);
    }

    // --- format edges ----------------------------------------------------

    function test_bad_length_reverts() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0x5);
        bytes memory pubkey = _pubkey(eph);
        bytes memory tooShort = hex"deadbeef"; // 4 bytes, not 65
        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.announce(topic, owner, eph, pubkey, tooShort);
    }

    function test_bad_v_reverts() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0x6);
        bytes memory pubkey = _pubkey(eph);
        (, bytes32 r, bytes32 s) = vm.sign(ownerPk, _digest(topic, eph, pubkey));
        // v = 5 (invalid; _recover normalizes <27 by +27 → 32, still invalid).
        bytes memory bad = abi.encodePacked(r, s, uint8(5));
        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.announce(topic, owner, eph, pubkey, bad);
    }

    function test_reannounce_upserts_not_duplicates() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0x7);
        bytes memory pk1 = _pubkey(eph);
        sig.announce(topic, owner, eph, pk1, _sign(ownerPk, _digest(topic, eph, pk1)));

        // Re-announce same ephemeral with a fresh pubkey + later timestamp.
        bytes memory pk2 = abi.encodePacked(hex"03", bytes32(uint256(0xFEED)));
        vm.warp(block.timestamp + 100);
        sig.announce(topic, owner, eph, pk2, _sign(ownerPk, _digest(topic, eph, pk2)));

        LibSignalingStorage.Presence[] memory r = sig.peersOf(topic);
        assertEq(r.length, 1, "upsert, not duplicate");
        assertEq(r[0].pubkey, pk2, "pubkey refreshed");
        assertEq(r[0].ts, uint64(block.timestamp), "ts refreshed");
    }

    // --- leave auth (devices topic) -------------------------------------

    /// The owner can evict an entry from their OWN devices roster with a valid
    /// owner-signed leave digest.
    function test_leave_owner_signed_removes_entry() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0x8);
        bytes memory pubkey = _pubkey(eph);
        sig.announce(topic, owner, eph, pubkey, _sign(ownerPk, _digest(topic, eph, pubkey)));
        assertEq(sig.peersOf(topic).length, 1, "announced");

        sig.leave(topic, owner, eph, _sign(ownerPk, _leaveDigest(topic, eph)));
        assertEq(sig.peersOf(topic).length, 0, "owner-signed leave removed it");
    }

    /// The bug this fixes: an attacker (no seed, wrong sig) must NOT be able to
    /// evict a victim's real device from the victim's devices roster — an
    /// unauthenticated `leave` would let them break the announce integrity
    /// property by kicking the legitimate device out.
    function test_leave_attacker_sig_reverts_and_roster_intact() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0xBADBAD);
        bytes memory pubkey = _pubkey(eph);
        sig.announce(topic, owner, eph, pubkey, _sign(ownerPk, _digest(topic, eph, pubkey)));
        assertEq(sig.peersOf(topic).length, 1, "victim device announced");

        // Attacker signs the leave digest with THEIR key → recovers to attacker,
        // not owner → must revert, roster untouched.
        bytes memory bad = _sign(attackerPk, _leaveDigest(topic, eph));
        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.leave(topic, owner, eph, bad);
        assertEq(sig.peersOf(topic).length, 1, "roster intact after blocked leave");
    }

    /// An `announce` signature must NOT be replayable as a `leave` (domain
    /// separation): the owner-signed announce digest does not authorize a leave.
    function test_leave_cannot_replay_announce_sig() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0x9);
        bytes memory pubkey = _pubkey(eph);
        bytes memory announceSig = _sign(ownerPk, _digest(topic, eph, pubkey));
        sig.announce(topic, owner, eph, pubkey, announceSig);

        // Reuse the announce sig for leave — recovers to owner over the WRONG
        // (announce) digest, so vs the leave digest it does NOT recover to owner.
        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.leave(topic, owner, eph, announceSig);
        assertEq(sig.peersOf(topic).length, 1, "announce sig can't drive a leave");
    }

    /// High-s leave signature is rejected (EIP-2), like announce.
    function test_leave_high_s_reverts() public {
        bytes32 topic = _devicesTopic(owner);
        address eph = address(0xA1);
        bytes memory pubkey = _pubkey(eph);
        sig.announce(topic, owner, eph, pubkey, _sign(ownerPk, _digest(topic, eph, pubkey)));

        bytes32 ld = _leaveDigest(topic, eph);
        (uint8 v, bytes32 r, bytes32 s) = vm.sign(ownerPk, ld);
        bytes32 sHigh = bytes32(N - uint256(s));
        uint8 vFlip = v == 27 ? 28 : 27;
        bytes memory sigHigh = abi.encodePacked(r, sHigh, vFlip);

        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.leave(topic, owner, eph, sigHigh);
        assertEq(sig.peersOf(topic).length, 1, "high-s leave blocked");
    }

    // --- leave auth (non-devices / team topic) --------------------------

    /// On a team (non-devices) topic, a device may remove ITSELF: a sig
    /// recovering to the ephemeral authorizes its own leave. An unrelated
    /// signer is rejected.
    function test_leave_team_self_signed_ok_others_revert() public {
        bytes32 teamTopic = keccak256(abi.encodePacked("localharness.team", uint256(7)));
        uint256 ephPk = 0xEEEE;
        address eph = vm.addr(ephPk);
        bytes memory pubkey = _pubkey(eph);
        sig.announce(teamTopic, address(0), eph, pubkey, _sign(ephPk, _digest(teamTopic, eph, pubkey)));
        assertEq(sig.peersOf(teamTopic).length, 1, "team peer announced");

        // Attacker can't evict it.
        vm.expectRevert(SignalingFacet.Unauthorized.selector);
        sig.leave(teamTopic, address(0), eph, _sign(attackerPk, _leaveDigest(teamTopic, eph)));
        assertEq(sig.peersOf(teamTopic).length, 1, "attacker can't evict team peer");

        // The ephemeral removes itself.
        sig.leave(teamTopic, address(0), eph, _sign(ephPk, _leaveDigest(teamTopic, eph)));
        assertEq(sig.peersOf(teamTopic).length, 0, "self-signed team leave removed it");
    }

    /// A team (non-devices) topic accepts a SELF-signed announce (sig ==
    /// ephemeral) — the documented floor until member-gating ships.
    function test_team_topic_self_signed_ok() public {
        bytes32 teamTopic = keccak256(abi.encodePacked("localharness.team", uint256(42)));
        uint256 ephPk = 0xEEEE;
        address eph = vm.addr(ephPk);
        bytes memory pubkey = _pubkey(eph);
        // Signed by the EPHEMERAL key itself → recovers to ephemeral.
        bytes memory s = _sign(ephPk, _digest(teamTopic, eph, pubkey));
        sig.announce(teamTopic, address(0), eph, pubkey, s);
        assertEq(sig.peersOf(teamTopic).length, 1, "team self-sign accepted");
    }
}
