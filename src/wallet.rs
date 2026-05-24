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

/// Generate a new random keypair using the host's CSPRNG.
/// On wasm32 entropy comes from `crypto.getRandomValues` via the
/// `getrandom/js` feature already enabled by this crate.
pub fn generate() -> GeneratedWallet {
    let signer = SigningKey::random(&mut rand_core::OsRng);
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
    let entropy = mnemonic.to_entropy(); // 16 bytes for 12 words
    // Stretch the entropy into 32 bytes via keccak256 — a single
    // hash is enough for "identity from mnemonic"; this isn't HD
    // derivation territory.
    let mut hasher = Keccak256::new();
    hasher.update(b"localharness/v0/identity");
    hasher.update(&entropy);
    let digest = hasher.finalize();
    SigningKey::from_slice(&digest)
        .expect("keccak256 output is 32 bytes; SigningKey is infallible for valid scalars")
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

// --- minimal hex helpers (avoids pulling in the `hex` crate) ---------

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(nibble(b >> 4));
        out.push(nibble(b & 0x0f));
    }
    out
}

fn hex_decode(s: &str) -> Result<Vec<u8>, &'static str> {
    if s.len() % 2 != 0 {
        return Err("odd length");
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = unnibble(bytes[i])?;
        let lo = unnibble(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn nibble(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + (n - 10)) as char,
        _ => unreachable!(),
    }
}

fn unnibble(b: u8) -> Result<u8, &'static str> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err("non-hex char"),
    }
}

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
