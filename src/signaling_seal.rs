//! Pure, native-testable envelope layer for on-chain WebRTC signaling blobs
//! (the P2P teams layer's SDP exchange over `SignalingFacet.postSignal`).
//!
//! Hoisted out of `app::teams_sync` (wasm-only) so the seal/unseal round-trip,
//! tamper rejection, and roster-validation logic run under a native
//! `cargo test --features wallet` — the same pattern as
//! [`crate::sharedfs_reconcile`].
//!
//! ## Why a signed envelope (and not just ECIES)
//! The SDP payload itself is ECIES-sealed to the recipient's announced
//! ephemeral pubkey (`app::encryption::ecies_seal`) — that gives
//! CONFIDENTIALITY (an on-chain observer never sees ICE candidates/topology).
//! But ECIES alone does NOT authenticate the SENDER: `postSignal` is
//! permissionless and the recipient's pubkey is public on the roster, so
//! anyone could seal a malicious SDP to it and claim a legitimate peer's
//! ephemeral address in the blob — the recipient would decrypt it fine and
//! connect to the ATTACKER's WebRTC endpoint (shared-folder theft MITM).
//!
//! This module closes that: every blob is SIGNED by the sender's EPHEMERAL
//! key, and the signature binds the RECIPIENT too (so a blob can't be
//! replayed into a different peer's inbox). The recipient verifies that the
//! recovered signer equals the claimed sender address — and the caller only
//! ever accepts senders that appear on the OWNER-SIGNED roster
//! (`SignalingFacet.announce` is seed-gated), so only a real device of the
//! same owner can produce an acceptable blob. Ephemeral addresses are fresh
//! per sync session, so a stale envelope from a prior session fails the
//! expected-sender match.
//!
//! ## Wire format (v2 — HARD CUT, sealed+signed only)
//! ```text
//! [0..6)    magic   b"lhsdp2"
//! [6..26)   sender  ephemeral address (20 raw bytes)
//! [26..91)  sig     65 bytes r||s||v over keccak256(
//!               "localharness/v0/sdpseal" || sender(20) || recipient(20) || sealed)
//! [91..)    sealed  ECIES blob (ephemeral_pub(33) || IV || ct || tag)
//! ```
//! Pre-v2 blobs (`"<eph_hex>\n<sealed>"`, unsigned) are REJECTED outright —
//! a deliberate hard cut rather than a versioned fallback: the layer has no
//! production users yet, and accepting unsigned legacy blobs would keep the
//! MITM hole open for as long as the fallback existed.

use k256::ecdsa::SigningKey;

/// Envelope magic — bumps with any layout change (hard-cut versioning).
pub const MAGIC: &[u8; 6] = b"lhsdp2";
/// Domain-separation tag for the envelope signature digest.
const DIGEST_TAG: &[u8] = b"localharness/v0/sdpseal";
const ADDR_LEN: usize = 20;
const SIG_LEN: usize = 65;
const HEADER_LEN: usize = MAGIC.len() + ADDR_LEN + SIG_LEN;

/// The 32-byte digest the sender's ephemeral key signs:
/// `keccak256(tag || sender(20) || recipient(20) || sealed)`. Binding the
/// recipient blocks cross-inbox replay; binding the sealed bytes blocks
/// payload substitution under a reused signature.
fn envelope_digest(sender: &[u8; 20], recipient: &[u8; 20], sealed: &[u8]) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut h = Keccak256::new();
    h.update(DIGEST_TAG);
    h.update(sender);
    h.update(recipient);
    h.update(sealed);
    let d = h.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&d);
    out
}

/// Build a signed signaling envelope around an already-ECIES-sealed payload.
/// `sender_key` is THIS device's session-ephemeral signaling key (the one whose
/// address was announced on the roster); `recipient` is the peer's ephemeral
/// address (whose inbox the blob is posted to).
pub fn seal_envelope(sender_key: &SigningKey, recipient: &[u8; 20], sealed: &[u8]) -> Vec<u8> {
    let sender = crate::wallet::address(sender_key);
    let digest = envelope_digest(&sender, recipient, sealed);
    let sig = crate::wallet::sign_hash(sender_key, &digest);
    let mut out = Vec::with_capacity(HEADER_LEN + sealed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&sender);
    out.extend_from_slice(&sig);
    out.extend_from_slice(sealed);
    out
}

/// Verify + unwrap an envelope. Returns the inner ECIES-sealed bytes ONLY if:
/// the magic matches (pre-v2/garbage blobs are rejected — hard cut), the
/// claimed sender equals `expected_sender` (a roster-validated peer), and the
/// signature over `(sender, recipient, sealed)` recovers to that sender.
/// Anything else — tampered payload, substituted sender, replay aimed at a
/// different recipient, forged signature — returns `None`.
pub fn open_envelope(
    blob: &[u8],
    expected_sender: &[u8; 20],
    recipient: &[u8; 20],
) -> Option<Vec<u8>> {
    if blob.len() < HEADER_LEN || &blob[..MAGIC.len()] != MAGIC {
        return None;
    }
    let sender: [u8; 20] = blob[MAGIC.len()..MAGIC.len() + ADDR_LEN].try_into().ok()?;
    if &sender != expected_sender {
        return None;
    }
    let sig: [u8; 65] = blob[MAGIC.len() + ADDR_LEN..HEADER_LEN].try_into().ok()?;
    let sealed = &blob[HEADER_LEN..];
    let digest = envelope_digest(&sender, recipient, sealed);
    let recovered = crate::wallet::recover_address(&sig, &digest).ok()?;
    (recovered == sender).then(|| sealed.to_vec())
}

/// Ethereum address of a compressed/uncompressed SEC1 public key, or `None`
/// if it isn't a valid curve point. Used to check that a roster entry's
/// announced `pubkey` (the ECIES seal target) actually hashes to the
/// `ephemeral` address it was announced under.
pub fn address_of_sec1_pubkey(pubkey_sec1: &[u8]) -> Option<[u8; 20]> {
    use k256::PublicKey;
    use k256::elliptic_curve::sec1::ToEncodedPoint;
    use sha3::{Digest, Keccak256};
    let pk = PublicKey::from_sec1_bytes(pubkey_sec1).ok()?;
    let uncompressed = pk.to_encoded_point(false); // 65 bytes, 0x04 prefix
    let bytes = uncompressed.as_bytes();
    if bytes.len() != 65 {
        return None;
    }
    let digest = Keccak256::digest(&bytes[1..]); // drop the 0x04 tag
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&digest[12..]);
    Some(addr)
}

/// Client-side roster gate for one `peersOf` entry: the announced pubkey must
/// be a valid curve point that HASHES TO the announced ephemeral address
/// (rejects a seal-target swapped in under someone else's address), and the
/// announce timestamp must be within `ttl_secs` of `now` (skips dead sessions
/// and refuses to honour a long-stale entry). Defence-in-depth on top of the
/// facet's owner-signed `announce`.
pub fn roster_entry_valid(
    peer: &[u8; 20],
    ts: u64,
    pubkey_sec1: &[u8],
    now: u64,
    ttl_secs: u64,
) -> bool {
    if pubkey_sec1.is_empty() {
        return false; // nothing to seal to
    }
    // `saturating_sub` tolerates a peer's chain timestamp slightly ahead of
    // our wall clock.
    if now.saturating_sub(ts) > ttl_secs {
        return false;
    }
    address_of_sec1_pubkey(pubkey_sec1).is_some_and(|derived| &derived == peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> (SigningKey, [u8; 20]) {
        let w = crate::wallet::generate();
        let addr = crate::wallet::address(&w.signer);
        (w.signer.clone(), addr)
    }

    #[test]
    fn seal_open_round_trip_binary_payload() {
        // The sealed ECIES payload is BINARY — embed NULs and newlines to prove
        // the envelope is length-framed, not delimiter-framed (the pre-v2
        // format split on '\n').
        let (sender_key, sender) = keypair();
        let (_, recipient) = keypair();
        let sealed: Vec<u8> = vec![0x02, 0x00, b'\n', 0xFF, b'\n', 0x00, 0xAB];
        let blob = seal_envelope(&sender_key, &recipient, &sealed);
        assert_eq!(&blob[..6], MAGIC);
        assert_eq!(&blob[6..26], &sender);
        let opened = open_envelope(&blob, &sender, &recipient).expect("round trip");
        assert_eq!(opened, sealed);
    }

    #[test]
    fn tampering_any_byte_rejects() {
        // Flip every byte position class: magic, claimed sender, signature,
        // payload. All must fail verification (None), never panic.
        let (sender_key, sender) = keypair();
        let (_, recipient) = keypair();
        let sealed = b"sealed-sdp-bytes".to_vec();
        let blob = seal_envelope(&sender_key, &recipient, &sealed);
        for i in 0..blob.len() {
            let mut t = blob.clone();
            t[i] ^= 0x01;
            assert!(
                open_envelope(&t, &sender, &recipient).is_none(),
                "tampered byte {i} must be rejected"
            );
        }
    }

    #[test]
    fn forged_sender_claim_rejects() {
        // An attacker seals to the (public) recipient pubkey and CLAIMS a
        // legitimate peer's address — but can only sign with their OWN key.
        // The recovered signer != claimed sender → rejected. This is the MITM
        // the envelope exists to close.
        let (attacker_key, _) = keypair();
        let (_, legit_sender) = keypair();
        let (_, recipient) = keypair();
        let sealed = b"attacker sdp".to_vec();
        // Build an envelope signed by the attacker but claiming legit_sender.
        let digest = envelope_digest(&legit_sender, &recipient, &sealed);
        let sig = crate::wallet::sign_hash(&attacker_key, &digest);
        let mut blob = Vec::new();
        blob.extend_from_slice(MAGIC);
        blob.extend_from_slice(&legit_sender);
        blob.extend_from_slice(&sig);
        blob.extend_from_slice(&sealed);
        assert!(open_envelope(&blob, &legit_sender, &recipient).is_none());
    }

    #[test]
    fn wrong_expected_sender_rejects() {
        // A valid envelope from A is ignored when the poller is waiting on B
        // (inbox correlation: only the peer we're handshaking with is read).
        let (a_key, a) = keypair();
        let (_, b) = keypair();
        let (_, recipient) = keypair();
        let blob = seal_envelope(&a_key, &recipient, b"sdp");
        assert!(open_envelope(&blob, &b, &recipient).is_none());
        assert!(open_envelope(&blob, &a, &recipient).is_some());
    }

    #[test]
    fn cross_inbox_replay_rejects() {
        // The signature binds the RECIPIENT: a blob sealed for inbox R1 fails
        // verification when presented as if addressed to R2.
        let (sender_key, sender) = keypair();
        let (_, r1) = keypair();
        let (_, r2) = keypair();
        let blob = seal_envelope(&sender_key, &r1, b"sdp");
        assert!(open_envelope(&blob, &sender, &r2).is_none());
        assert!(open_envelope(&blob, &sender, &r1).is_some());
    }

    #[test]
    fn legacy_and_garbage_blobs_reject() {
        // HARD CUT: the pre-v2 "<eph_hex>\n<sealed>" plaintext-prefix format,
        // empty blobs, short blobs, and random bytes are all rejected.
        let (_, sender) = keypair();
        let (_, recipient) = keypair();
        let legacy = b"0x1111111111111111111111111111111111111111\n\x02sealed".to_vec();
        assert!(open_envelope(&legacy, &sender, &recipient).is_none());
        assert!(open_envelope(b"", &sender, &recipient).is_none());
        assert!(open_envelope(b"lhsdp2", &sender, &recipient).is_none()); // header-only
        assert!(open_envelope(&[0xAAu8; 200], &sender, &recipient).is_none());
        // Right length, wrong magic.
        let (k, s) = keypair();
        let mut blob = seal_envelope(&k, &recipient, b"x");
        blob[0] ^= 0xFF;
        assert!(open_envelope(&blob, &s, &recipient).is_none());
    }

    #[test]
    fn empty_sealed_payload_still_authenticates() {
        // Degenerate but well-formed: zero-length payload round-trips (the
        // signature still covers sender+recipient, so it's not forgeable).
        let (sender_key, sender) = keypair();
        let (_, recipient) = keypair();
        let blob = seal_envelope(&sender_key, &recipient, b"");
        assert_eq!(blob.len(), HEADER_LEN);
        assert_eq!(open_envelope(&blob, &sender, &recipient).unwrap(), b"");
    }

    #[test]
    fn envelope_digest_is_domain_separated_and_order_sensitive() {
        // Pin the digest preimage layout: tag || sender || recipient || sealed.
        // Swapping sender/recipient MUST change the digest (replay binding).
        let (_, a) = keypair();
        let (_, b) = keypair();
        assert_ne!(envelope_digest(&a, &b, b"x"), envelope_digest(&b, &a, b"x"));
        assert_ne!(envelope_digest(&a, &b, b"x"), envelope_digest(&a, &b, b"y"));
        use sha3::{Digest, Keccak256};
        let mut pre = Vec::new();
        pre.extend_from_slice(b"localharness/v0/sdpseal");
        pre.extend_from_slice(&a);
        pre.extend_from_slice(&b);
        pre.extend_from_slice(b"x");
        let expect: [u8; 32] = Keccak256::digest(&pre).into();
        assert_eq!(envelope_digest(&a, &b, b"x"), expect);
    }

    #[test]
    fn address_of_sec1_pubkey_matches_wallet_address() {
        // Compressed AND uncompressed encodings of the same key derive the
        // same address, equal to wallet::address; invalid points are None.
        let (key, addr) = keypair();
        let compressed = crate::wallet::pubkey_compressed(&key);
        assert_eq!(address_of_sec1_pubkey(&compressed), Some(addr));
        let uncompressed = k256::ecdsa::VerifyingKey::from(&key)
            .to_encoded_point(false)
            .as_bytes()
            .to_vec();
        assert_eq!(address_of_sec1_pubkey(&uncompressed), Some(addr));
        assert_eq!(address_of_sec1_pubkey(&[0u8; 33]), None);
        assert_eq!(address_of_sec1_pubkey(b""), None);
    }

    #[test]
    fn roster_entry_validation_gates() {
        let (key, addr) = keypair();
        let pubkey = crate::wallet::pubkey_compressed(&key);
        let now = 1_000_000u64;
        let ttl = 600u64;
        // Fresh + self-consistent → valid (incl. a peer slightly ahead of us).
        assert!(roster_entry_valid(&addr, now, &pubkey, now, ttl));
        assert!(roster_entry_valid(&addr, now - ttl, &pubkey, now, ttl)); // boundary
        assert!(roster_entry_valid(&addr, now + 30, &pubkey, now, ttl)); // clock skew
        // Stale → invalid.
        assert!(!roster_entry_valid(&addr, now - ttl - 1, &pubkey, now, ttl));
        // Empty pubkey → invalid (nothing to seal to).
        assert!(!roster_entry_valid(&addr, now, &[], now, ttl));
        // Pubkey under someone ELSE's address → invalid (forged seal target).
        let (_, other) = keypair();
        assert!(!roster_entry_valid(&other, now, &pubkey, now, ttl));
        // Garbage pubkey → invalid.
        assert!(!roster_entry_valid(&addr, now, &[0u8; 33], now, ttl));
    }
}
