//! The signer postMessage protocol — the ONE definition shared by both
//! sides: the apex signer service (`signer.rs`, hosted at
//! `localharness.xyz/?signer=1`) and its subdomain client (`verify.rs`).
//!
//! Before this module the message-type strings were free literals on each
//! side and the challenge preimage was implemented TWICE, joined only by
//! "MUST stay byte-for-byte identical" comments. Both now live here so the
//! two sides cannot drift: a typo'd message type fails to compile instead
//! of silently never matching, and the owner-proof preimage has a single
//! definition the signer signs and the verifier recovers against.

use sha3::{Digest, Keccak256};

// ---- Request message types (parent → signer) -------------------------------

/// Sign an owner-proof challenge (auto-approved; read-only verification).
pub(crate) const MSG_SIGN_CHALLENGE: &str = "lh-sign-challenge";
/// Sign a sponsored Tempo tx from its STRUCTURED fields (allowlisted).
pub(crate) const MSG_SIGN_DIGEST: &str = "lh-sign-digest";
/// Ensure/overwrite the master wallet (overwrite is apex-origin only).
pub(crate) const MSG_CREATE_WALLET: &str = "lh-create-wallet";
/// Reveal the master mnemonic (apex-origin only).
pub(crate) const MSG_REVEAL_SEED: &str = "lh-reveal-seed";
/// Import a seed phrase, replacing the master wallet (apex-origin only).
pub(crate) const MSG_IMPORT_SEED: &str = "lh-import-seed";
/// Run the full apex claim flow for a name (long-running).
pub(crate) const MSG_CLAIM_NAME: &str = "lh-claim-name";
/// Seal a plaintext (the Gemini key) under the seed-derived key.
pub(crate) const MSG_SEAL_KEY: &str = "lh-seal-key";
/// Open seed-sealed ciphertext back to plaintext.
pub(crate) const MSG_OPEN_KEY: &str = "lh-open-key";

// ---- Reply / lifecycle message types (signer → parent) ---------------------

/// The single reply type for every request above: `{type, id, ...}` on
/// success, `{type, id, error}` on failure.
pub(crate) const MSG_SIGN_RESPONSE: &str = "lh-sign-response";
/// Readiness ping the signer posts once its listener is installed, so the
/// client gates on it instead of a fixed sleep.
pub(crate) const MSG_SIGNER_READY: &str = "lh-signer-ready";

// ---- Owner-proof challenge preimage ----------------------------------------

/// Domain-separation tag for owner-proof challenges, so a captured
/// signature can't be replayed as a real tx.
const DOMAIN_TAG: &[u8] = b"localharness-auth-v0:";

/// `keccak256("localharness-auth-v0:" || name || ":" || nonce)` — the
/// owner-proof prehash. Binds the proof to BOTH the subdomain `name` and a
/// random nonce, so a signature proving ownership of one name can't be
/// replayed as proof for a different name held by the same address. The
/// signer signs exactly this; the verifier recovers against exactly this.
pub(crate) fn challenge_prehash(name: &str, nonce: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak256::new();
    hasher.update(DOMAIN_TAG);
    hasher.update(name.as_bytes());
    hasher.update(b":");
    hasher.update(nonce);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hasher.finalize());
    out
}
