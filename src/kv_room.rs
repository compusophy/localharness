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
//! of re-sending it). Multi-identity rooms (ECIES-granting `K_room` to other
//! members enrolled via the facet's `roomAddMember`) are a clean phase 2 on top
//! of this same op format.

use crate::kv_reduce::KvOp;
use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use k256::ecdsa::SigningKey;
use sha3::{Digest, Keccak256};

const OP_VERSION: u8 = 1;
const NONCE_LEN: usize = 12;

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
}
