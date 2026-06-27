//! In-browser secp256k1 keypair — k256 + sha3 directly.
//!
//! Tried alloy's `signer-local` first (M6 spike), but alloy's
//! `TransactionEnvelope` proc-macro currently trips on serde 1.0.228's
//! `__private` namespace (alloy-consensus 1.0.22). We don't need
//! transaction envelope encoding for identity-only signing, so this
//! module uses k256 + sha3 directly — that's what alloy uses under
//! the hood for the local signer anyway.
//!
//! Surface (intentionally small):
//! - `generate()` — new random keypair
//! - `from_private_key_hex` — restore from a saved hex string
//! - `address(signer)` — derive the 20-byte EVM address
//! - `sign_hash(signer, h)` — produce a 65-byte ECDSA signature
//!   (r ‖ s ‖ v) over a 32-byte prehash
//! - `recover_address` — recover the signer's address from a
//!   signature + prehash (verification)
//!
//! No HTTP, no JS bindings — pure compute. Compiles on every target.

use k256::ecdsa::signature::hazmat::{PrehashSigner, PrehashVerifier};
use k256::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey};
use sha3::{Digest, Keccak256};
use zeroize::Zeroize;

/// A freshly-generated keypair plus its hex-encoded private key.
pub struct GeneratedWallet {
    pub signer: SigningKey,
    pub address: [u8; 20],
    pub private_key_hex: String,
}

impl GeneratedWallet {
    pub fn address_hex(&self) -> String {
        format!("0x{}", hex_encode(&self.address))
    }
}

impl Drop for GeneratedWallet {
    fn drop(&mut self) {
        // `private_key_hex` is a fully-formed, exportable private key in a
        // heap `String`; wipe it so it doesn't linger in freed memory. The
        // `SigningKey` zeroizes its own scalar on drop (k256), and a 20-byte
        // address isn't secret.
        self.private_key_hex.zeroize();
    }
}

/// Generate a new random keypair using the host's CSPRNG.
/// On wasm32 entropy comes from `crypto.getRandomValues` via the
/// `getrandom/js` feature already enabled by this crate.
pub fn generate() -> GeneratedWallet {
    let signer = SigningKey::random(&mut rand_core::OsRng);
    finalize(signer)
}

/// Wrap an EXISTING [`SigningKey`] as a [`GeneratedWallet`] (address + hex
/// derived from it). The reuse counterpart of [`generate`] — e.g. the CLI's
/// idempotent `create`, which re-claims a name with the key it already holds
/// instead of overwriting the key file with a fresh wallet.
pub fn from_signing_key(signer: SigningKey) -> GeneratedWallet {
    finalize(signer)
}

/// Generate a BIP-39 12-word mnemonic (English wordlist) AND the
/// SigningKey derived from its 32-byte seed. We use the seed
/// directly as the private key — no HD derivation path — because
/// this is identity, not a hierarchical wallet. One mnemonic, one
/// key, one address.
pub fn generate_with_mnemonic() -> (bip39::Mnemonic, SigningKey) {
    let mnemonic = bip39::Mnemonic::generate(12).expect("12 is a valid word count");
    let signer = signer_from_mnemonic(&mnemonic);
    (mnemonic, signer)
}

/// Derive the SigningKey from a mnemonic — same path as
/// `generate_with_mnemonic` so the round-trip is stable.
pub fn signer_from_mnemonic(mnemonic: &bip39::Mnemonic) -> SigningKey {
    let mut entropy = mnemonic.to_entropy(); // 16 bytes for 12 words
    // Stretch the entropy into 32 bytes via keccak256 — a single
    // hash is enough for "identity from mnemonic"; this isn't HD
    // derivation territory.
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness/v0/identity");
    hasher.update(&entropy);
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&hasher.finalize());
    let signer = SigningKey::from_slice(&digest)
        .expect("keccak256 output is 32 bytes; SigningKey is infallible for valid scalars");
    // Wipe the transient secret-derived material (the raw entropy and the
    // private-scalar digest) now that the key is built.
    entropy.zeroize();
    digest.zeroize();
    signer
}

/// Parse a 12-word phrase. Whitespace is normalised, case is
/// ignored. Returns the underlying Mnemonic so callers can
/// re-derive the signer.
pub fn mnemonic_from_phrase(phrase: &str) -> Result<bip39::Mnemonic, String> {
    let normalised: String = phrase
        .split_whitespace()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    bip39::Mnemonic::parse_in_normalized(bip39::Language::English, &normalised)
        .map_err(|e| e.to_string())
}

/// Restore from `0x`-prefixed (or bare) hex.
pub fn from_private_key_hex(hex: &str) -> Result<SigningKey, String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    let bytes = hex_decode(trimmed).map_err(|e| format!("invalid hex: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("expected 32-byte private key, got {}", bytes.len()));
    }
    SigningKey::from_slice(&bytes).map_err(|e| format!("invalid scalar: {e}"))
}

/// EVM address = last 20 bytes of keccak256(uncompressed pubkey [1..]).
pub fn address(signer: &SigningKey) -> [u8; 20] {
    let verifying = VerifyingKey::from(signer);
    let encoded = verifying.to_encoded_point(false); // uncompressed (65 bytes, 0x04 prefix)
    let bytes = encoded.as_bytes();
    debug_assert_eq!(bytes.len(), 65);
    let mut hasher = Keccak256::new();
    hasher.update(&bytes[1..]); // drop the 0x04 SEC1 tag
    let digest = hasher.finalize();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&digest[12..]);
    addr
}

/// Sign a 32-byte prehash, returning the 65-byte Ethereum-style
/// `r ‖ s ‖ v` signature with `v` recovery id ∈ {27, 28}.
pub fn sign_hash(signer: &SigningKey, hash: &[u8; 32]) -> [u8; 65] {
    let (sig, rec): (Signature, RecoveryId) = signer
        .sign_prehash(hash)
        .expect("k256 sign_prehash is infallible for a valid SigningKey");
    let sig_bytes = sig.to_bytes(); // 64 bytes: r ‖ s
    let mut out = [0u8; 65];
    out[..64].copy_from_slice(&sig_bytes);
    out[64] = 27 + u8::from(rec); // Ethereum convention
    out
}

/// Compute the Ethereum `personal_sign` digest of a message:
/// `keccak256("\x19Ethereum Signed Message:\n" || ascii(len) || message)`.
/// This is the digest any standard `eth_personalSign` verifier (e.g. the
/// credit proxy's `recoverAddress`) reconstructs.
pub fn personal_sign_digest(message: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(b"\x19Ethereum Signed Message:\n");
    hasher.update(message.len().to_string().as_bytes());
    hasher.update(message);
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

/// Sign `message` as an Ethereum `personal_sign`: the 65-byte `r‖s‖v`
/// signature (v ∈ {27,28}) over the prefixed keccak digest. Used to mint
/// the credit-proxy auth token; verifiable by `recover_address` against
/// `personal_sign_digest(message)`.
pub fn personal_sign(signer: &SigningKey, message: &[u8]) -> [u8; 65] {
    sign_hash(signer, &personal_sign_digest(message))
}

/// Recover the signer's address from a 65-byte signature + the 32-byte
/// prehash that was signed. Used to verify "did this address sign this?"
/// without needing the pubkey shipped alongside.
pub fn recover_address(signature: &[u8; 65], prehash: &[u8; 32]) -> Result<[u8; 20], String> {
    let v = signature[64];
    let rec_id = match v {
        0 | 27 => 0u8,
        1 | 28 => 1u8,
        _ => return Err(format!("invalid v: {v}")),
    };
    let rec = RecoveryId::try_from(rec_id).map_err(|e| e.to_string())?;
    let sig = Signature::from_slice(&signature[..64]).map_err(|e| e.to_string())?;
    // EIP-2 low-s (anti-malleability, audit I3): k256's `normalize_s` returns
    // `Some` only when `s` is in the upper half of the curve order — i.e. the
    // malleable high-s twin of a canonical signature. Reject it so this off-chain
    // verifier agrees with the on-chain HALF_N gate (X402Facet / MultiSignerAccount)
    // and the proxy's `_x402.ts`/`_authcore.ts` — otherwise we'd recover a valid
    // address from a signature the chain (and a payer's intent) would refuse.
    if sig.normalize_s().is_some() {
        return Err("malleable (high-s) signature rejected".to_string());
    }
    let verifying = VerifyingKey::recover_from_prehash(prehash, &sig, rec)
        .map_err(|e| e.to_string())?;

    let encoded = verifying.to_encoded_point(false);
    let bytes = encoded.as_bytes();
    let mut hasher = Keccak256::new();
    hasher.update(&bytes[1..]);
    let digest = hasher.finalize();
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&digest[12..]);
    Ok(addr)
}

/// Verify a 64-byte (r‖s) signature against a known pubkey-derived
/// signer. Mostly useful when we already know which address to expect.
#[allow(dead_code)]
pub fn verify_hash(
    signer: &SigningKey,
    hash: &[u8; 32],
    signature: &[u8; 65],
) -> Result<(), String> {
    let verifying = VerifyingKey::from(signer);
    let sig = Signature::from_slice(&signature[..64]).map_err(|e| e.to_string())?;
    verifying.verify_prehash(hash, &sig).map_err(|e| e.to_string())
}

/// Compressed SEC1 public key (33 bytes, 0x02/0x03 prefix) for a signing
/// key. Used as the recipient identifier in ECIES key-wrapping — the
/// device announces this so the desktop can encrypt to it.
pub fn pubkey_compressed(signer: &SigningKey) -> Vec<u8> {
    let verifying = VerifyingKey::from(signer);
    verifying.to_encoded_point(true).as_bytes().to_vec()
}

/// Generate an ephemeral keypair for one ECIES wrap. Returns
/// `(compressed_pubkey, ephemeral_signer)`.
pub fn ephemeral_keypair() -> (Vec<u8>, SigningKey) {
    let signer = SigningKey::random(&mut rand_core::OsRng);
    (pubkey_compressed(&signer), signer)
}

/// ECDH → a 32-byte symmetric key shared between `my` private key and
/// `their` SEC1 public key. Domain-separated through keccak so the raw
/// curve point never becomes the AES key directly. Both sides derive the
/// same bytes: sealer uses (ephemeral_priv, recipient_pub); opener uses
/// (recipient_priv, ephemeral_pub).
pub fn ecdh_shared_key(my: &SigningKey, their_pubkey_sec1: &[u8]) -> Result<[u8; 32], String> {
    use k256::{PublicKey, SecretKey};
    let their = PublicKey::from_sec1_bytes(their_pubkey_sec1)
        .map_err(|e| format!("bad recipient pubkey: {e}"))?;
    let secret =
        SecretKey::from_bytes(&my.to_bytes()).map_err(|e| format!("bad scalar: {e}"))?;
    let shared = k256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), their.as_affine());
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness/v0/ecies");
    hasher.update(shared.raw_secret_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    Ok(out)
}

/// Derive the 32-byte AES key that seals/opens the on-chain Gemini key,
/// from a master wallet's BIP-39 entropy (tag `localharness/v0/keysync`).
/// Deterministic from the seed, so any device holding it derives the same
/// key — a byte-for-byte cross-device contract. SHARED source of truth for
/// both the apex signer iframe (`app::signer`) and the local-first path in
/// `app::verify` (a subdomain that pulled the seed in via `seed_pull`);
/// they MUST agree, hence one impl here (next to the sibling
/// `localharness/v0/ecies` tag in [`ecdh_shared_key`]) where native tests
/// can pin it. Re-exported through `app::encryption` for app call sites.
pub fn keysync_key_from_entropy(entropy: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness/v0/keysync");
    hasher.update(entropy);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// Derive the 32-byte AES key sealing the cross-subdomain **shared folder**
/// at rest in apex OPFS (`.lh_shared/`, see `app::shared_fs`).
/// Domain-separated from [`keysync_key_from_entropy`] (tag
/// `localharness/v0/sharedfs`) so the shared-folder key and the
/// Gemini-keysync key can never collide. Deterministic from the master
/// seed, so the apex broker — the only origin that holds the seed — always
/// derives the same key across devices.
pub fn sharedfs_key_from_entropy(entropy: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness/v0/sharedfs");
    hasher.update(entropy);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// Derive the 32-byte AES key for **at-rest OPFS encryption** (the
/// `filesystem::EncryptedFilesystem` wrapper) from a master wallet's
/// BIP-39 entropy (tag `localharness/v0/opfs-at-rest`). Deterministic
/// from the seed — every device/origin holding the seed derives the same
/// key, so files sealed on one device decrypt on a linked one. Domain-
/// separated from the sibling `keysync` / `sharedfs` tags so the at-rest
/// key can never collide with the Gemini-key or shared-folder keys.
/// Byte-for-byte pinned by `at_rest_key_pinned_and_distinct`; changing
/// the output orphans every sealed OPFS file.
pub fn at_rest_key_from_entropy(entropy: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness/v0/opfs-at-rest");
    hasher.update(entropy);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}

/// Derive the 32-byte AES key sealing the seed in the QR seed-adoption
/// flow (`?adopt=1#s=<ct>`), from a one-time pairing CODE (tag
/// `localharness/v0/adopt`, code uppercased + trimmed). Deterministic on
/// every device, so the desktop browser can `seal` the mnemonic under it
/// and a second device — a paired phone OR the `localharness link` CLI —
/// derives the SAME key from the typed code and decrypts. The SINGLE
/// source of truth shared by the browser (`app::events::devices`) and the
/// CLI (`bin/localharness::link`); they MUST agree byte-for-byte, hence one
/// impl here next to the sibling key derivations where a native test pins it.
pub fn adopt_code_key(code: &str) -> [u8; 32] {
    // Stretch the low-entropy human pairing code with an iterated keccak KDF: an
    // attacker who captures ONLY the sealed `#s=<ct>` blob (a leaked QR / URL) must
    // pay ADOPT_KDF_ROUNDS keccaks PER candidate code to brute-force it offline.
    // Combined with the higher-entropy code (`events::devices::generate_pair_code`,
    // now 8 chars ≈ 2^40) and the receiver no longer persisting the ciphertext in
    // its history, this puts the offline search out of practical reach. (Keccak is
    // GPU-parallel; a memory-hard KDF would be stronger still — audit follow-up.)
    // The output is shared BYTE-FOR-BYTE by the browser and the `localharness link`
    // CLI (both call this) — changing ADOPT_KDF_ROUNDS or the tags breaks in-flight
    // links and the pinned `adopt_code_key_pinned_*` test.
    const ADOPT_KDF_ROUNDS: u32 = 200_000;
    let mut acc = {
        let mut hasher = Keccak256::new();
        hasher.update(b"localharness/v0/adopt");
        hasher.update(code.trim().to_uppercase().as_bytes());
        hasher.finalize()
    };
    for _ in 0..ADOPT_KDF_ROUNDS {
        let mut hasher = Keccak256::new();
        hasher.update(b"localharness/v0/adopt-kdf");
        hasher.update(&acc[..]);
        acc = hasher.finalize();
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&acc);
    out
}

fn finalize(signer: SigningKey) -> GeneratedWallet {
    let address = address(&signer);
    let private_key_hex = format!("0x{}", hex_encode(&signer.to_bytes()));
    GeneratedWallet {
        signer,
        address,
        private_key_hex,
    }
}

// --- minimal RLP (Ethereum's serialization format for tx envelopes) --

/// RLP-encode a byte string. Used for tx fields and for wrapping the
/// final encoded list.
pub fn rlp_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len() + 9);
    if input.len() == 1 && input[0] < 0x80 {
        out.push(input[0]);
    } else if input.len() <= 55 {
        out.push(0x80 + input.len() as u8);
        out.extend_from_slice(input);
    } else {
        let len_bytes = be_bytes_no_leading_zero(input.len() as u128);
        out.push(0xb7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
        out.extend_from_slice(input);
    }
    out
}

/// RLP-encode a list. `items` is each item already RLP-encoded.
pub fn rlp_list(items: &[Vec<u8>]) -> Vec<u8> {
    let body_len: usize = items.iter().map(|i| i.len()).sum();
    let mut out = Vec::with_capacity(body_len + 9);
    if body_len <= 55 {
        out.push(0xc0 + body_len as u8);
    } else {
        let len_bytes = be_bytes_no_leading_zero(body_len as u128);
        out.push(0xf7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
    }
    for item in items {
        out.extend_from_slice(item);
    }
    out
}

/// Minimal big-endian encoding of a u128: drop leading zero bytes,
/// but if the value is 0 return a single 0 byte. RLP convention is
/// "empty" for zero quantities in some contexts; callers usually
/// wrap via `rlp_uint` which returns `[]` for zero.
fn be_bytes_no_leading_zero(value: u128) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first_non_zero = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len() - 1);
    bytes[first_non_zero..].to_vec()
}

/// RLP-encode a uint: empty bytes for zero, minimal big-endian
/// otherwise. This is the convention legacy/EIP-155 txs use for
/// quantity fields (nonce, gasPrice, gasLimit, value, v, r, s).
pub fn rlp_uint(value: u128) -> Vec<u8> {
    if value == 0 {
        rlp_bytes(&[])
    } else {
        rlp_bytes(&be_bytes_no_leading_zero(value))
    }
}

// --- minimal hex helpers ----------------------------------------------
//
// Thin aliases over the crate-canonical `crate::encoding` codecs (this
// module's hand-rolled nibble loops were a byte-identical third copy).
// `hex_decode` accepts a little MORE than the old local fn did (it also
// trims whitespace / strips an optional `0x`), but every caller here
// pre-strips, so behavior on the wallet paths is unchanged.

use crate::encoding::bytes_to_hex as hex_encode;
use crate::encoding::hex_to_bytes as hex_decode;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_then_restore_round_trips_the_address() {
        let w = generate();
        // 0x + 64 hex chars
        assert_eq!(w.private_key_hex.len(), 66);
        assert!(w.private_key_hex.starts_with("0x"));
        let restored = from_private_key_hex(&w.private_key_hex).unwrap();
        assert_eq!(address(&restored), w.address);
    }

    #[test]
    fn address_is_20_bytes() {
        let w = generate();
        assert_eq!(w.address.len(), 20);
        assert_eq!(w.address_hex().len(), 42); // 0x + 40 hex chars
    }

    #[test]
    fn recover_rejects_high_s_malleable_signature() {
        // EIP-2 low-s gate (audit I3): the malleable high-s twin (r, n-s, v^1)
        // recovers the same key on a permissive verifier but must be rejected here,
        // matching the on-chain HALF_N gate. Our own `sign_hash` emits low-s (proven
        // by the round-trip test below), so the twin is built by replacing s with n-s.
        fn n_minus_s(s: &[u8; 32]) -> [u8; 32] {
            // secp256k1 group order n, big-endian.
            const N: [u8; 32] = [
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0xff, 0xff, 0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2,
                0x5e, 0x8c, 0xd0, 0x36, 0x41, 0x41,
            ];
            let mut out = [0u8; 32];
            let mut borrow = 0i16;
            for i in (0..32).rev() {
                let d = N[i] as i16 - s[i] as i16 - borrow;
                if d < 0 {
                    out[i] = (d + 256) as u8;
                    borrow = 1;
                } else {
                    out[i] = d as u8;
                    borrow = 0;
                }
            }
            out
        }
        let w = generate();
        let hash = [0x42u8; 32];
        let low = sign_hash(&w.signer, &hash);
        assert_eq!(recover_address(&low, &hash).unwrap(), w.address); // low-s verifies
        let mut high = low;
        let s: [u8; 32] = low[32..64].try_into().unwrap();
        high[32..64].copy_from_slice(&n_minus_s(&s));
        high[64] ^= 1; // the malleable twin's flipped recovery bit
        assert!(
            recover_address(&high, &hash).is_err(),
            "high-s (malleable) signature must be rejected"
        );
    }

    #[test]
    fn sign_then_recover_returns_signing_address() {
        let w = generate();
        let hash = [0x42u8; 32];
        let sig = sign_hash(&w.signer, &hash);
        assert_eq!(sig.len(), 65);
        assert!(matches!(sig[64], 27 | 28));
        let recovered = recover_address(&sig, &hash).unwrap();
        assert_eq!(recovered, w.address);
    }

    #[test]
    fn personal_sign_roundtrips_through_recover() {
        // The credit proxy recovers the signer from personal_sign_digest;
        // this guards that our digest + signature match that verifier.
        let w = generate();
        let msg = b"localharness-proxy:0xabc:1717200000";
        let sig = personal_sign(&w.signer, msg);
        assert!(matches!(sig[64], 27 | 28));
        let recovered = recover_address(&sig, &personal_sign_digest(msg)).unwrap();
        assert_eq!(recovered, w.address);
    }

    #[test]
    fn recover_rejects_invalid_v() {
        let w = generate();
        let hash = [0x99u8; 32];
        let mut sig = sign_hash(&w.signer, &hash);
        sig[64] = 99; // bogus recovery id
        assert!(recover_address(&sig, &hash).is_err());
    }

    #[test]
    fn verify_hash_accepts_own_signature() {
        let w = generate();
        let hash = [0x01u8; 32];
        let sig = sign_hash(&w.signer, &hash);
        verify_hash(&w.signer, &hash, &sig).unwrap();
    }

    #[test]
    fn mnemonic_round_trips_through_phrase_to_address() {
        let (m, k1) = generate_with_mnemonic();
        let phrase = m.to_string();
        // 12 space-separated words
        assert_eq!(phrase.split_whitespace().count(), 12);
        let restored = mnemonic_from_phrase(&phrase).unwrap();
        let k2 = signer_from_mnemonic(&restored);
        assert_eq!(address(&k1), address(&k2));
    }

    #[test]
    fn rlp_short_string_round_trip() {
        // Known vectors from the RLP spec.
        // empty string -> 0x80
        assert_eq!(rlp_bytes(&[]), vec![0x80]);
        // single byte < 0x80 -> itself
        assert_eq!(rlp_bytes(&[0x7f]), vec![0x7f]);
        // "dog" -> 0x83 'd' 'o' 'g'
        assert_eq!(rlp_bytes(b"dog"), vec![0x83, b'd', b'o', b'g']);
    }

    #[test]
    fn rlp_long_string_uses_length_prefix() {
        let s = vec![0xaa; 100];
        let enc = rlp_bytes(&s);
        assert_eq!(enc[0], 0xb8); // 0xb7 + 1 byte for length
        assert_eq!(enc[1], 100);
        assert_eq!(&enc[2..], &s[..]);
    }

    #[test]
    fn rlp_uint_zero_is_empty_string() {
        assert_eq!(rlp_uint(0), vec![0x80]);
    }

    #[test]
    fn rlp_uint_small_minimal() {
        // 15 -> single byte
        assert_eq!(rlp_uint(15), vec![0x0f]);
        // 256 -> 0x82 0x01 0x00
        assert_eq!(rlp_uint(256), vec![0x82, 0x01, 0x00]);
    }

    #[test]
    fn rlp_list_known_vector() {
        // ["cat", "dog"] -> 0xc8 0x83 'c' 'a' 't' 0x83 'd' 'o' 'g'
        let cat = rlp_bytes(b"cat");
        let dog = rlp_bytes(b"dog");
        let enc = rlp_list(&[cat, dog]);
        assert_eq!(
            enc,
            vec![0xc8, 0x83, b'c', b'a', b't', 0x83, b'd', b'o', b'g']
        );
    }

    /// PINNED derivation vectors. `signer_from_mnemonic` is a CUSTOM
    /// stretch — `keccak256("localharness/v0/identity" || entropy)` — and
    /// the seed IS the identity: `wallet_store` re-derives the key from the
    /// mnemonic on every load, so ANY change to the tag, the hash, or the
    /// entropy handling silently re-keys every returning user (new address,
    /// orphaned names, lost $LH). The round-trip tests above can't catch
    /// that — a changed derivation still round-trips. Do NOT regenerate
    /// these constants to make the test pass; a mismatch means the identity
    /// derivation CHANGED and existing users would be locked out.
    #[test]
    fn mnemonic_known_vector_pins_identity_derivation() {
        // The standard BIP-39 zero-entropy phrase (entropy = [0u8; 16]).
        let phrase = "abandon abandon abandon abandon abandon abandon \
                      abandon abandon abandon abandon abandon about";
        let m = mnemonic_from_phrase(phrase).unwrap();
        assert_eq!(m.to_entropy(), vec![0u8; 16]);
        let signer = signer_from_mnemonic(&m);
        // Generated ONCE from the live implementation (2026-06-10) — pins
        // the localharness/v0/identity tag + keccak stretch + entropy input.
        assert_eq!(
            format!("0x{}", hex_encode(&address(&signer))),
            "0x4800ae69a4855281a1251f8c3beab064eb7da012",
            "identity derivation changed — this re-keys EVERY returning user"
        );

        // Independent check of the keccak-of-pubkey ADDRESS path: private
        // key 0x…01 has an externally-known address (any EVM tool agrees:
        // 0x7E5F4552091A69125d5DfCb7b8C2659029395Bdf), so this leg doesn't
        // depend on our own impl having been correct when pinned.
        let k1 = from_private_key_hex(
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        assert_eq!(
            format!("0x{}", hex_encode(&address(&k1))),
            "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf",
            "address derivation no longer matches the EVM standard"
        );
    }

    /// PINNED ECDH contract — the ECIES core under mobile seed transport
    /// (`seed_pull`), sealed SDP, and the Gemini-key handoff. Pins: (1)
    /// ECDH symmetry (sealer and opener derive the same bytes), (2) the
    /// exact output (the `localharness/v0/ecies` tag + keccak + input
    /// order — change any and every existing ECIES blob becomes
    /// undecryptable), (3) the 33-byte compressed-pubkey framing that
    /// `ecies_open`'s `split_at(33)` relies on, (4) graceful Err (not a
    /// panic) on garbage pubkey bytes.
    #[test]
    fn ecdh_shared_key_is_symmetric_and_pinned() {
        let a = from_private_key_hex(
            "0x000000000000000000000000000000000000000000000000000000000000000a",
        )
        .unwrap();
        let b = from_private_key_hex(
            "0x000000000000000000000000000000000000000000000000000000000000000b",
        )
        .unwrap();
        let pub_a = pubkey_compressed(&a);
        let pub_b = pubkey_compressed(&b);

        // (1) Symmetry: ECDH(a, pub_b) == ECDH(b, pub_a).
        let k_ab = ecdh_shared_key(&a, &pub_b).unwrap();
        let k_ba = ecdh_shared_key(&b, &pub_a).unwrap();
        assert_eq!(k_ab, k_ba);

        // (2) Generated ONCE from the live implementation (2026-06-10).
        // A mismatch means existing sealed blobs can no longer be opened.
        assert_eq!(
            hex_encode(&k_ab),
            "3225f3c45abcb834b362af592bdcb9b999380d22521627ae7e71f9bbce614e47",
            "ECIES shared-key derivation changed"
        );

        // (3) Compressed SEC1 framing: 33 bytes, 0x02/0x03 prefix.
        assert_eq!(pub_a.len(), 33);
        assert!(matches!(pub_a[0], 0x02 | 0x03));

        // (4) Garbage pubkey is an Err, never a panic.
        assert!(ecdh_shared_key(&a, &[0u8; 33]).is_err());
    }

    /// PINNED AES key derivations with a byte-for-byte CROSS-DEVICE
    /// contract: `keysync` seals the on-chain Gemini key (apex signer
    /// iframe + the subdomain local-first path must agree) and `sharedfs`
    /// seals `.lh_shared/` at rest. A changed output orphans everything
    /// already sealed under the old key. Also pins that the two tags
    /// actually domain-separate (distinct outputs for the same entropy).
    #[test]
    fn keysync_and_sharedfs_keys_pinned_and_distinct() {
        let entropy = [0u8; 16];
        let keysync = keysync_key_from_entropy(&entropy);
        let sharedfs = sharedfs_key_from_entropy(&entropy);
        // Generated ONCE from the live implementation (2026-06-10).
        assert_eq!(
            hex_encode(&keysync),
            "d3ddc0e89ef28726b10fa9aed5fdb086d9dd79aad14b37c9b8fb7b49c9cf77f5",
            "keysync key derivation changed — sealed Gemini keys orphaned"
        );
        assert_eq!(
            hex_encode(&sharedfs),
            "5d0d6e8c644245c728b0248c30ab02f0a2492f982c99c572ce54210592ca739b",
            "sharedfs key derivation changed — sealed shared folders orphaned"
        );
        assert_ne!(keysync, sharedfs);
    }

    /// PINNED at-rest OPFS key derivation (tag `localharness/v0/opfs-at-rest`,
    /// the `filesystem::EncryptedFilesystem` key). Same cross-device contract
    /// as its siblings: a changed output orphans every OPFS file sealed under
    /// the old key — conversation history, system prompt, working files all
    /// become unreadable ciphertext. Also pins domain separation from the
    /// keysync and sharedfs tags.
    #[test]
    fn at_rest_key_pinned_and_distinct() {
        let entropy = [0u8; 16];
        let at_rest = at_rest_key_from_entropy(&entropy);
        // Generated ONCE from the live implementation (2026-06-12).
        assert_eq!(
            hex_encode(&at_rest),
            "a0c9c69ced27af86580487d0e3f487ef7143ecfbf69045335e9ea53809a92ced",
            "at-rest key derivation changed — every sealed OPFS file orphaned"
        );
        assert_ne!(at_rest, keysync_key_from_entropy(&entropy));
        assert_ne!(at_rest, sharedfs_key_from_entropy(&entropy));
    }

    /// PINNED adopt-code key derivation (tag `localharness/v0/adopt`) — the
    /// QR seed-adoption transport key the browser seals the seed under and the
    /// `localharness link` CLI re-derives to open it. The two MUST agree, and
    /// a changed output silently breaks every in-flight adopt link, so pin it.
    /// Also pins the code is normalized (uppercased + trimmed) before hashing,
    /// so a phone typing `abc123` and the CLI passing ` ABC123 ` agree.
    #[test]
    fn adopt_code_key_pinned_and_case_insensitive() {
        // Generated ONCE from the live implementation (2026-06-26, after the
        // KDF-stretch hardening — audit H1: 200k iterated keccak rounds).
        assert_eq!(
            hex_encode(&adopt_code_key("ABC234")),
            "b43a133dacf72b743f1451cdaaf0134a96e6e73ef93d28286ea15916073b6e31",
            "adopt-code key derivation changed — in-flight adopt links break + CLI/browser drift"
        );
        // Case + surrounding whitespace are normalized away (same key).
        assert_eq!(adopt_code_key("abc234"), adopt_code_key("ABC234"));
        assert_eq!(adopt_code_key("  abc234 \n"), adopt_code_key("ABC234"));
        // A different code derives a different key (no collision).
        assert_ne!(adopt_code_key("ABC234"), adopt_code_key("ABC235"));
    }

    #[test]
    fn mnemonic_phrase_is_case_and_whitespace_tolerant() {
        let (m, k1) = generate_with_mnemonic();
        let messy = m
            .to_string()
            .split_whitespace()
            .map(|w| if w.len() > 3 { w.to_uppercase() } else { w.to_string() })
            .collect::<Vec<_>>()
            .join("   ");
        let restored = mnemonic_from_phrase(&messy).unwrap();
        let k2 = signer_from_mnemonic(&restored);
        assert_eq!(address(&k1), address(&k2));
    }
}
