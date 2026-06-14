//! SessionRoom op sealing + per-room key derivation (GitHub #22).
//!
//! A SessionRoom is an append-only on-chain log of OPAQUE blobs ([`SessionRoom`
//! facet] stores `(writer, ts, blob)`); the chain never sees plaintext. This
//! module is the off-chain crypto that turns a [`crate::kv_reduce::KvOp`] into a
//! room blob and back:
//!
//! - **Confidentiality** — the op plaintext is AES-256-GCM-sealed under the
//!   room key `K_room`. Only holders of `K_room` can read.
//! - **Authenticity + room-binding** — the ciphertext is then wrapped in a
//!   [`crate::signaling_seal`] envelope signed by the writer's identity key and
//!   bound to a per-room recipient address. A reader verifies the envelope
//!   against the on-chain `Op.writer` (which is `msg.sender` at append time), so
//!   a forged or cross-room-replayed blob is rejected before decryption.
//!
//! ## Key distribution (v1: single-identity rooms)
//! `K_room` is **deterministically derived** from the owner's identity secret +
//! the room id ([`derive_room_key`]), so every device/session of the SAME
//! identity computes the same key with NO on-chain key exchange — the primary
//! #22 use case (an agent persisting shared state across turns/devices instead
//! of re-sending it).
//!
//! ## Key distribution (phase 2: multi-identity rooms)
//! To share a room with OTHER identities, the creator instead generates a
//! **random** `K_room`, ECIES-seals it to each member's identity public key
//! ([`key_grant_seal`]), and posts those grants (e.g. alongside `roomAddMember`).
//! Each member recovers the same `K_room` with their own identity key
//! ([`key_grant_open`]) and reads/writes ops exactly as in v1 — the op format
//! is unchanged, only the source of `K_room` differs (derived vs granted). The
//! grant mirrors the [`crate::app::encryption::ecies_seal`] layout but uses the
//! synchronous `aes_gcm` path (native-testable, no WebCrypto), wrapping the
//! 32-byte key under an ECDH-derived AES key (`crate::wallet::ecdh_shared_key`,
//! tag `localharness/v0/ecies` — keccak over the shared-secret x-coordinate).
//! Phase 2 is purely additive: it does NOT touch the facet or v1 derivation.

use crate::kv_reduce::KvOp;
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

const OP_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;
/// Compressed SEC1 pubkey length — the ephemeral key prefixing a key grant.
const EPH_PUB_LEN: usize = 33;
/// AES-256-GCM authentication tag length.
const GCM_TAG_LEN: usize = 16;

/// Derive the symmetric room key from an identity secret (the 32-byte k256
/// scalar, e.g. `signing_key.to_bytes()`) and the room id. Pure keccak with a
/// pinned domain tag — same discipline as the seed-derived keys in
/// `crate::wallet`. Every device of the same identity derives the same key.
pub fn derive_room_key(identity_secret: &[u8; 32], room_id: u64) -> [u8; 32] {
    let mut h = Keccak256::new();
    h.update(b"localharness/sessionroom/v1/key");
    h.update(identity_secret);
    h.update(room_id.to_be_bytes());
    h.finalize().into()
}

/// The 20-byte pseudo-address an op envelope is bound to, derived from the room
/// id. Binding the `signaling_seal` recipient to this means a blob lifted from
/// room A and replayed into room B fails to open (recipient mismatch).
pub fn room_recipient(room_id: u64) -> [u8; 20] {
    let mut h = Keccak256::new();
    h.update(b"localharness/sessionroom/v1/recipient");
    h.update(room_id.to_be_bytes());
    let d = h.finalize();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&d[12..]);
    addr
}

/// Serialize a `KvOp` to its stable plaintext framing (before sealing):
/// `version | key_len(u16) | key | val_tag(u8) | [val_len(u32) | val] |
/// lamport(u64) | writer(20) | ts(u64)` — all big-endian.
pub fn encode_op(op: &KvOp) -> Vec<u8> {
    let key = op.key.as_bytes();
    let mut out = Vec::with_capacity(1 + 2 + key.len() + 1 + 4 + 8 + 20 + 8);
    out.push(OP_VERSION);
    out.extend_from_slice(&(key.len() as u16).to_be_bytes());
    out.extend_from_slice(key);
    match &op.value {
        Some(v) => {
            out.push(1);
            out.extend_from_slice(&(v.len() as u32).to_be_bytes());
            out.extend_from_slice(v);
        }
        None => out.push(0),
    }
    out.extend_from_slice(&op.lamport.to_be_bytes());
    out.extend_from_slice(&op.writer);
    out.extend_from_slice(&op.ts.to_be_bytes());
    out
}

/// Parse [`encode_op`] bytes back into a `KvOp`. `None` on any malformed input.
pub fn decode_op(bytes: &[u8]) -> Option<KvOp> {
    let mut i = 0usize;
    let take = |i: &mut usize, n: usize| -> Option<&[u8]> {
        let end = i.checked_add(n)?;
        let s = bytes.get(*i..end)?;
        *i = end;
        Some(s)
    };
    if *take(&mut i, 1)?.first()? != OP_VERSION {
        return None;
    }
    let key_len = u16::from_be_bytes(take(&mut i, 2)?.try_into().ok()?) as usize;
    let key = String::from_utf8(take(&mut i, key_len)?.to_vec()).ok()?;
    let value = match take(&mut i, 1)?[0] {
        0 => None,
        1 => {
            let val_len = u32::from_be_bytes(take(&mut i, 4)?.try_into().ok()?) as usize;
            Some(take(&mut i, val_len)?.to_vec())
        }
        _ => return None,
    };
    let lamport = u64::from_be_bytes(take(&mut i, 8)?.try_into().ok()?);
    let writer: [u8; 20] = take(&mut i, 20)?.try_into().ok()?;
    let ts = u64::from_be_bytes(take(&mut i, 8)?.try_into().ok()?);
    if i != bytes.len() {
        return None; // trailing garbage
    }
    Some(KvOp {
        key,
        value,
        lamport,
        writer,
        ts,
    })
}

/// Seal an op into a room blob: `signaling_seal( AES-256-GCM_{K_room}(op) )`.
/// `writer_key` is the writer's identity key — its address MUST be the
/// `msg.sender` that appends the blob on-chain, so readers can authenticate it
/// against the stored `Op.writer`. Returns `None` only on AES init failure.
pub fn seal_op(
    op: &KvOp,
    k_room: &[u8; 32],
    writer_key: &SigningKey,
    room_id: u64,
) -> Option<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(k_room).ok()?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher.encrypt(&nonce, encode_op(op).as_slice()).ok()?;
    let mut sealed = Vec::with_capacity(NONCE_LEN + ct.len());
    sealed.extend_from_slice(nonce.as_slice());
    sealed.extend_from_slice(&ct);
    Some(crate::signaling_seal::seal_envelope(
        writer_key,
        &room_recipient(room_id),
        &sealed,
    ))
}

/// Open a room blob written by `writer_addr` (the on-chain `Op.writer`). Returns
/// the op ONLY if: the envelope verifies as signed by `writer_addr` and bound to
/// this room, the AES tag checks out under `K_room`, AND the decoded op's
/// `writer` matches `writer_addr` (the claimed author is bound to the signer).
/// `None` on any tamper / wrong-key / wrong-writer / cross-room replay.
pub fn open_op(
    blob: &[u8],
    k_room: &[u8; 32],
    writer_addr: &[u8; 20],
    room_id: u64,
) -> Option<KvOp> {
    let sealed = crate::signaling_seal::open_envelope(blob, writer_addr, &room_recipient(room_id))?;
    if sealed.len() < NONCE_LEN {
        return None;
    }
    let (nonce_bytes, ct) = sealed.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(k_room).ok()?;
    let plaintext = cipher.decrypt(Nonce::from_slice(nonce_bytes), ct).ok()?;
    let op = decode_op(&plaintext)?;
    (op.writer == *writer_addr).then_some(op)
}

// ── Phase 2: multi-identity key grants (ECIES-wrap of a random K_room) ──────

/// Domain-separation tag bound into the key-grant AEAD as associated data,
/// alongside the ephemeral pubkey. Pins the grant to this exact construction.
const GRANT_AAD_TAG: &[u8] = b"localharness/sessionroom/v1/keygrant";

/// ECIES-seal a random room key `k_room` to a member's identity public key
/// (compressed/uncompressed SEC1). The member recovers it with
/// [`key_grant_open`]. Output layout:
/// `ephemeral_pub(33) || nonce(12) || AES-256-GCM_{K}(k_room) || tag(16)`,
/// where `K = ecdh_shared_key(ephemeral_priv, recipient_pub)` — the same
/// shared key the recipient derives from `(recipient_priv, ephemeral_pub)`. So
/// the 32-byte room key is readable ONLY by the holder of the recipient's
/// identity secret; the creator never needs the member's private key. Mirrors
/// the [`crate::app::encryption::ecies_seal`] wire format with the synchronous
/// `aes_gcm` path, and additionally **binds the ephemeral pubkey (+ a domain
/// tag) into the GCM associated data**. That AAD binding matters: k256 ECDH
/// derives the shared key from only the x-coordinate of the shared point, so a
/// point and its negation (the compressed-prefix parity bit, byte 0) yield the
/// SAME key — without authenticating the ephemeral bytes, that one bit would be
/// silently malleable. With it, any change to the ephemeral pubkey fails the
/// tag. Returns `None` on a bad recipient pubkey or AES init failure.
///
/// This is ADDITIVE: it grants access to a creator-chosen random `K_room` for
/// multi-identity rooms; v1 single-identity rooms keep using [`derive_room_key`]
/// and need no grant. The on-chain facet is untouched.
pub fn key_grant_seal(k_room: &[u8; 32], recipient_pubkey_sec1: &[u8]) -> Option<Vec<u8>> {
    let (eph_pub, eph_signer) = crate::wallet::ephemeral_keypair();
    let key = crate::wallet::ecdh_shared_key(&eph_signer, recipient_pubkey_sec1).ok()?;
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let aad = grant_aad(&eph_pub);
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: k_room.as_slice(),
                aad: &aad,
            },
        )
        .ok()?;
    let mut out = Vec::with_capacity(eph_pub.len() + NONCE_LEN + ct.len());
    out.extend_from_slice(&eph_pub);
    out.extend_from_slice(nonce.as_slice());
    out.extend_from_slice(&ct);
    Some(out)
}

/// Recover the `K_room` from a grant sealed by [`key_grant_seal`] using the
/// recipient's identity signing key. Expects
/// `ephemeral_pub(33) || nonce(12) || ct || tag(16)`. Returns `None` on any
/// malformed input, a wrong key (ECDH yields a different AES key → tag fails),
/// tampered bytes (GCM tag fails, including any change to the ephemeral pubkey
/// via the bound AAD), or a recovered payload that isn't exactly 32 bytes.
/// Never panics.
pub fn key_grant_open(sealed: &[u8], recipient_key: &SigningKey) -> Option<[u8; 32]> {
    // 33 (ephemeral pub) + 12 (nonce) + 16 (GCM tag) is the minimum; the
    // plaintext is a fixed 32-byte key, so a well-formed grant is exactly
    // EPH_PUB_LEN + NONCE_LEN + 32 + GCM_TAG_LEN, but we tolerate any length
    // that decrypts to 32 bytes rather than hard-coding the ciphertext size.
    if sealed.len() < EPH_PUB_LEN + NONCE_LEN + GCM_TAG_LEN {
        return None;
    }
    let (eph_pub, rest) = sealed.split_at(EPH_PUB_LEN);
    let (nonce_bytes, ct) = rest.split_at(NONCE_LEN);
    let key = crate::wallet::ecdh_shared_key(recipient_key, eph_pub).ok()?;
    let cipher = Aes256Gcm::new_from_slice(&key).ok()?;
    let aad = grant_aad(eph_pub);
    let plaintext = cipher
        .decrypt(
            Nonce::from_slice(nonce_bytes),
            Payload { msg: ct, aad: &aad },
        )
        .ok()?;
    plaintext.try_into().ok()
}

/// Associated data bound into a key grant's AEAD: `tag || ephemeral_pub`.
fn grant_aad(eph_pub: &[u8]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(GRANT_AAD_TAG.len() + eph_pub.len());
    aad.extend_from_slice(GRANT_AAD_TAG);
    aad.extend_from_slice(eph_pub);
    aad
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(b: u8) -> SigningKey {
        SigningKey::from_slice(&[b; 32]).unwrap()
    }

    fn sample(writer: [u8; 20]) -> KvOp {
        KvOp {
            key: "score".into(),
            value: Some(b"42".to_vec()),
            lamport: 7,
            writer,
            ts: 1234,
        }
    }

    #[test]
    fn encode_decode_round_trip_value_and_tombstone() {
        let v = sample([9u8; 20]);
        assert_eq!(decode_op(&encode_op(&v)).unwrap(), v);
        let t = KvOp {
            value: None,
            ..sample([3u8; 20])
        };
        assert_eq!(decode_op(&encode_op(&t)).unwrap(), t);
        // trailing garbage and truncation are rejected.
        let mut buf = encode_op(&v);
        buf.push(0xff);
        assert!(decode_op(&buf).is_none());
        assert!(decode_op(&encode_op(&v)[..3]).is_none());
    }

    #[test]
    fn derive_room_key_deterministic_and_room_unique() {
        let secret = [5u8; 32];
        assert_eq!(derive_room_key(&secret, 1), derive_room_key(&secret, 1));
        assert_ne!(derive_room_key(&secret, 1), derive_room_key(&secret, 2));
        assert_ne!(derive_room_key(&secret, 1), derive_room_key(&[6u8; 32], 1));
        assert_ne!(room_recipient(1), room_recipient(2));
    }

    #[test]
    fn seal_open_round_trip() {
        let wk = key(1);
        let waddr = crate::wallet::address(&wk);
        let k = derive_room_key(&[7u8; 32], 42);
        let op = sample(waddr);
        let blob = seal_op(&op, &k, &wk, 42).unwrap();
        assert_eq!(open_op(&blob, &k, &waddr, 42).unwrap(), op);
    }

    #[test]
    fn wrong_key_rejected() {
        let wk = key(1);
        let waddr = crate::wallet::address(&wk);
        let op = sample(waddr);
        let blob = seal_op(&op, &derive_room_key(&[7u8; 32], 42), &wk, 42).unwrap();
        // Different K_room → AES tag fails.
        assert!(open_op(&blob, &derive_room_key(&[8u8; 32], 42), &waddr, 42).is_none());
    }

    #[test]
    fn cross_room_replay_rejected() {
        let wk = key(1);
        let waddr = crate::wallet::address(&wk);
        let k = derive_room_key(&[7u8; 32], 42);
        let blob = seal_op(&sample(waddr), &k, &wk, 42).unwrap();
        // Same blob, opened as if it belonged to room 43 → recipient mismatch.
        assert!(open_op(&blob, &k, &waddr, 43).is_none());
    }

    #[test]
    fn wrong_writer_rejected() {
        let wk = key(1);
        let waddr = crate::wallet::address(&wk);
        let k = derive_room_key(&[7u8; 32], 42);
        let blob = seal_op(&sample(waddr), &k, &wk, 42).unwrap();
        // A different claimed writer address fails the envelope sender check.
        let other = crate::wallet::address(&key(2));
        assert!(open_op(&blob, &k, &other, 42).is_none());
    }

    #[test]
    fn tampered_blob_rejected() {
        let wk = key(1);
        let waddr = crate::wallet::address(&wk);
        let k = derive_room_key(&[7u8; 32], 42);
        let mut blob = seal_op(&sample(waddr), &k, &wk, 42).unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0xff;
        assert!(open_op(&blob, &k, &waddr, 42).is_none());
    }

    // ── Phase 2: key-grant crypto ──────────────────────────────────────────

    #[test]
    fn key_grant_round_trip() {
        // Creator seals a random K_room to a member's identity pubkey; the
        // member recovers exactly those 32 bytes with their own secret key.
        let member = key(11);
        let member_pub = crate::wallet::pubkey_compressed(&member);
        let k_room = [0x5Au8; 32];
        let grant = key_grant_seal(&k_room, &member_pub).unwrap();
        // Layout sanity: ephemeral pub prefix is a valid compressed point and
        // is NOT the recipient's pubkey (fresh ephemeral every seal).
        assert_eq!(grant.len(), EPH_PUB_LEN + NONCE_LEN + 32 + GCM_TAG_LEN);
        assert_ne!(&grant[..EPH_PUB_LEN], member_pub.as_slice());
        assert_eq!(key_grant_open(&grant, &member).unwrap(), k_room);
    }

    #[test]
    fn key_grant_works_with_uncompressed_recipient_pubkey() {
        // ecdh_shared_key accepts either SEC1 encoding; a grant sealed to the
        // uncompressed form still opens.
        use k256::ecdsa::VerifyingKey;
        let member = key(12);
        let uncompressed = VerifyingKey::from(&member)
            .to_encoded_point(false)
            .as_bytes()
            .to_vec();
        let k_room = [0x7Cu8; 32];
        let grant = key_grant_seal(&k_room, &uncompressed).unwrap();
        assert_eq!(key_grant_open(&grant, &member).unwrap(), k_room);
    }

    #[test]
    fn key_grant_each_seal_uses_fresh_ephemeral() {
        // Two grants of the SAME key to the SAME member differ entirely (fresh
        // ephemeral + fresh nonce) yet both open to the same K_room — no nonce
        // reuse under a shared AES key.
        let member = key(13);
        let member_pub = crate::wallet::pubkey_compressed(&member);
        let k_room = [0x33u8; 32];
        let g1 = key_grant_seal(&k_room, &member_pub).unwrap();
        let g2 = key_grant_seal(&k_room, &member_pub).unwrap();
        assert_ne!(g1, g2);
        assert_ne!(&g1[..EPH_PUB_LEN], &g2[..EPH_PUB_LEN]); // distinct ephemerals
        assert_eq!(key_grant_open(&g1, &member).unwrap(), k_room);
        assert_eq!(key_grant_open(&g2, &member).unwrap(), k_room);
    }

    #[test]
    fn key_grant_wrong_key_rejected() {
        // A grant sealed to member A cannot be opened by member B: B's ECDH
        // yields a different AES key → the GCM tag fails.
        let member_a = key(14);
        let member_b = key(15);
        let a_pub = crate::wallet::pubkey_compressed(&member_a);
        let grant = key_grant_seal(&[0x01u8; 32], &a_pub).unwrap();
        assert!(key_grant_open(&grant, &member_b).is_none());
        assert!(key_grant_open(&grant, &member_a).is_some());
    }

    #[test]
    fn key_grant_tampered_any_byte_rejected() {
        // Flip every byte position class — ephemeral pubkey, nonce, ciphertext,
        // tag. All must fail (None), never panic. The ephemeral pubkey is bound
        // into the GCM AAD, so even its parity bit (byte 0: 0x02<->0x03, which
        // negates the point but keeps the x-only ECDH key identical) fails the
        // tag rather than silently opening — the AAD closes that malleability.
        let member = key(16);
        let member_pub = crate::wallet::pubkey_compressed(&member);
        let grant = key_grant_seal(&[0x9Eu8; 32], &member_pub).unwrap();
        for i in 0..grant.len() {
            let mut t = grant.clone();
            t[i] ^= 0x01;
            assert!(
                key_grant_open(&t, &member).is_none(),
                "tampered grant byte {i} must be rejected"
            );
        }
    }

    #[test]
    fn key_grant_malformed_inputs_rejected() {
        // Empty, too-short, and header-only blobs are rejected without panic.
        let member = key(17);
        assert!(key_grant_open(b"", &member).is_none());
        assert!(key_grant_open(&[0u8; EPH_PUB_LEN], &member).is_none());
        assert!(key_grant_open(&[0u8; EPH_PUB_LEN + NONCE_LEN], &member).is_none());
        // Minimum length but garbage (invalid ephemeral point / bad tag).
        assert!(key_grant_open(&[0u8; EPH_PUB_LEN + NONCE_LEN + GCM_TAG_LEN], &member).is_none());
    }

    #[test]
    fn key_grant_bad_recipient_pubkey_returns_none() {
        // Sealing to a non-point pubkey fails cleanly rather than panicking.
        assert!(key_grant_seal(&[0x11u8; 32], &[0u8; 33]).is_none());
        assert!(key_grant_seal(&[0x11u8; 32], b"").is_none());
    }

    #[test]
    fn granted_key_decrypts_room_ops_end_to_end() {
        // The full phase-2 flow: a creator picks a RANDOM K_room (not derived),
        // grants it to a member, the member opens the grant and uses the
        // recovered key to read an op the WRITER sealed under that same K_room.
        // Proves the granted key is interchangeable with a v1 derived key for
        // the op layer.
        let creator = crate::wallet::generate().signer.clone();
        let member = key(18);
        let member_pub = crate::wallet::pubkey_compressed(&member);
        // Random room key (what phase-2 creators use instead of derive).
        let k_room: [u8; 32] = {
            let mut k = [0u8; 32];
            for (i, b) in k.iter_mut().enumerate() {
                *b = (i as u8).wrapping_mul(7).wrapping_add(3);
            }
            k
        };
        let grant = key_grant_seal(&k_room, &member_pub).unwrap();
        let recovered = key_grant_open(&grant, &member).unwrap();
        assert_eq!(recovered, k_room);

        // A writer (here the creator) seals an op under K_room; the member
        // opens it with the granted key.
        let waddr = crate::wallet::address(&creator);
        let op = sample(waddr);
        let blob = seal_op(&op, &k_room, &creator, 99).unwrap();
        assert_eq!(open_op(&blob, &recovered, &waddr, 99).unwrap(), op);
        // A non-member (no grant, wrong key) cannot read the op.
        assert!(open_op(&blob, &derive_room_key(&[0u8; 32], 99), &waddr, 99).is_none());
    }

    #[test]
    fn key_grant_does_not_leak_k_room_plaintext() {
        // The 32-byte K_room must never appear verbatim in the grant bytes.
        let member = key(19);
        let member_pub = crate::wallet::pubkey_compressed(&member);
        let k_room = [0xC4u8; 32];
        let grant = key_grant_seal(&k_room, &member_pub).unwrap();
        assert!(
            !grant.windows(k_room.len()).any(|w| w == k_room),
            "raw K_room leaked into the grant"
        );
    }
}
