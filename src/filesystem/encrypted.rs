//! Transparent at-rest encryption wrapper over any [`Filesystem`].
//!
//! [`EncryptedFilesystem`] wraps an inner filesystem (OPFS in the browser
//! app, [`super::NativeFilesystem`] in the unit tests) and seals every
//! `write_atomic` with AES-256-GCM under a caller-supplied 32-byte key —
//! in the browser app that key is derived from the master wallet seed via
//! [`crate::wallet::at_rest_key_from_entropy`] (tag
//! `localharness/v0/opfs-at-rest`), so a stolen browser profile yields
//! only ciphertext for agent data (conversation history, system prompt,
//! lessons, working files).
//!
//! ## File format
//!
//! ```text
//! "LHE1" (4 bytes) || nonce (12 bytes) || AES-256-GCM ciphertext+tag (n+16)
//! ```
//!
//! ## Transparent migration — plaintext stays readable FOREVER
//!
//! `read` sniffs the magic: present → decrypt (GCM auth failure is a
//! **clear error**, never silent garbage); absent → the bytes pass through
//! unchanged. Pre-existing plaintext files therefore keep working with no
//! flag-day, and re-encrypt naturally on their next write. The one edge:
//! a legacy *plaintext* file that happens to start with the 4 bytes
//! `LHE1` AND is ≥ 32 bytes long would be misread as ciphertext (and
//! error). No localharness-written file matches that shape.
//!
//! ## Exemptions — the identity/boot files are NEVER encrypted
//!
//! [`EXEMPT_FILES`] (matched on the file name, path-independent) skip
//! encryption on write. `.lh_wallet` is the decryption ROOT — sealing it
//! under a key derived from itself bricks the identity (the 2026-06-05
//! reset-brick class of bug), and the boot path must read `.lh_owner` /
//! `.lh_linked_owner` / `.lh_device_key` before a wallet exists. The two
//! local-model artifacts are public CDN downloads (~550 MB of Gemma
//! weights) — nothing secret, and far too large to round-trip through an
//! in-memory AEAD on every read.
//!
//! ## Threat model
//!
//! Confidentiality of OPFS contents at rest (stolen profile directory,
//! disk inspection, OPFS-scoped export/extension channels). It does NOT
//! defend against code running in the origin (which can load the seed and
//! derive the key), and the GCM tag authenticates file *contents*, not
//! file *names* — ciphertexts can be swapped between paths by an attacker
//! with write access (out of scope; write access also allows deletion).

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use async_trait::async_trait;

use super::{file_name, DirEntry, Filesystem, Metadata, SharedFilesystem, WalkEntry};
use crate::error::{Error, Result};

/// Magic prefix marking a sealed file. Version byte folded into the tag
/// (`LHE1` = localharness encryption, format v1).
pub const MAGIC: [u8; 4] = *b"LHE1";

/// AES-GCM nonce length (96-bit, the GCM standard).
const NONCE_LEN: usize = 12;

/// AES-GCM authentication tag length.
const TAG_LEN: usize = 16;

/// The shortest possible sealed file: magic + nonce + empty ciphertext + tag.
const MIN_SEALED_LEN: usize = MAGIC.len() + NONCE_LEN + TAG_LEN;

/// File names that are NEVER encrypted (matched on the final path
/// component). Three classes:
///
/// - **Identity / pre-wallet boot files** — `.lh_wallet` is the seed the
///   key derives FROM (encrypting it bricks the identity); `.lh_owner`,
///   `.lh_linked_owner`, and `.lh_device_key` are read by the mount path
///   before (or without) a master wallet existing.
/// - **Public local-model artifacts** — `.lh_local_model.safetensors` /
///   `.lh_local_tokenizer.json` are ~550 MB of public Gemma weights from
///   the HF CDN: nothing secret, too large for in-memory AEAD round-trips.
/// - **Notification inbox files** — `.lh_notif_pending.json` is written by
///   BOTH `web/sw.js` (a plain SERVICE WORKER with no seed, so no cipher)
///   and the Rust app via [`crate::app::shared_opfs`]; sealing the Rust
///   writes makes sw.js's `JSON.parse` of the `LHE1…` bytes throw, which
///   silently clobbers the stash down to one entry — closed-tab pushes then
///   never reach the in-app bell (#35 inbox-not-displaying bug). Keep this
///   file family plaintext so the two writers share ONE on-disk format;
///   `.lh_notif_inbox.json` is the merged log of that same plaintext-origin
///   data (kept format-uniform to avoid the inverse hazard).
///
/// Pinned by `exempt_list_is_pinned` — removing `.lh_wallet` from this
/// list is an identity-bricking change.
pub const EXEMPT_FILES: &[&str] = &[
    ".lh_wallet",
    ".lh_owner",
    ".lh_linked_owner",
    ".lh_device_key",
    ".lh_local_model.safetensors",
    ".lh_local_tokenizer.json",
    ".lh_notif_pending.json",
    ".lh_notif_inbox.json",
];

/// At-rest encryption wrapper implementing [`Filesystem`] around an inner
/// filesystem. See the module docs for format, migration, and exemptions.
pub struct EncryptedFilesystem {
    inner: SharedFilesystem,
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for EncryptedFilesystem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never Debug-print key material.
        f.debug_struct("EncryptedFilesystem")
            .field("inner", &self.inner)
            .field("key", &"<redacted>")
            .finish()
    }
}

impl EncryptedFilesystem {
    /// Wrap `inner` with at-rest AES-256-GCM under `key` (32 bytes — in
    /// the browser app, [`crate::wallet::at_rest_key_from_entropy`]).
    pub fn new(inner: SharedFilesystem, key: &[u8; 32]) -> Self {
        Self {
            inner,
            cipher: Aes256Gcm::new(key.into()),
        }
    }

    /// Whether `path`'s file name is on the never-encrypt list.
    pub fn is_exempt(path: &str) -> bool {
        // Strip trailing separators first so `.lh_wallet/` still matches.
        let base = file_name(path.trim_end_matches(['/', '\\']));
        EXEMPT_FILES.contains(&base)
    }

    /// Whether `bytes` carry the sealed-file shape (magic + minimum length).
    pub fn looks_sealed(bytes: &[u8]) -> bool {
        bytes.len() >= MIN_SEALED_LEN && bytes[..MAGIC.len()] == MAGIC
    }

    /// `MAGIC || nonce || ct+tag` with a fresh random nonce per call.
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| Error::other("at-rest encrypt failed"))?;
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + ct.len());
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    /// Decrypt bytes that passed [`Self::looks_sealed`]. A GCM auth
    /// failure (wrong key OR tampered ciphertext) is a clear error —
    /// never silently-returned garbage.
    fn open(&self, path: &str, sealed: &[u8]) -> Result<Vec<u8>> {
        let nonce_start = MAGIC.len();
        let ct_start = nonce_start + NONCE_LEN;
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&sealed[nonce_start..ct_start]);
        let nonce = Nonce::from(nonce);
        self.cipher.decrypt(&nonce, &sealed[ct_start..]).map_err(|_| {
            Error::other(format!(
                "at-rest decrypt failed for '{path}': wrong key or tampered ciphertext (GCM auth)"
            ))
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Filesystem for EncryptedFilesystem {
    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        if Self::is_exempt(path) {
            // Mirror the `write_atomic` exemption (the EXEMPT invariant holds in
            // BOTH directions): exempt files are stored verbatim, so read them
            // verbatim too — never route a possibly sealed-LOOKING exempt blob
            // (e.g. a `.lh_device_key` ciphertext that happens to begin with the
            // magic) through the seed-key GCM decrypt.
            return self.inner.read(path).await;
        }
        let bytes = self.inner.read(path).await?;
        if Self::looks_sealed(&bytes) {
            self.open(path, &bytes)
        } else {
            // Legacy plaintext — pass through as-is.
            Ok(bytes)
        }
    }

    async fn write_atomic(&self, path: &str, bytes: &[u8]) -> Result<()> {
        if Self::is_exempt(path) {
            return self.inner.write_atomic(path, bytes).await;
        }
        let sealed = self.seal(bytes)?;
        self.inner.write_atomic(path, &sealed).await
    }

    async fn metadata(&self, path: &str) -> Result<Option<Metadata>> {
        // Sizes reflect the on-disk (sealed) byte count — documented
        // divergence; the fs tools only branch on kind/existence.
        self.inner.metadata(path).await
    }

    async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
        self.inner.read_dir(path).await
    }

    async fn walk(&self, path: &str, max_depth: Option<usize>) -> Result<Vec<WalkEntry>> {
        self.inner.walk(path, max_depth).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        self.inner.delete(path).await
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        let from_exempt = Self::is_exempt(from);
        let to_exempt = Self::is_exempt(to);
        if from_exempt == to_exempt {
            // Same at-rest representation (both sealed, or both verbatim) —
            // ciphertext is not path-bound, so a raw move is correct and needs
            // no decrypt/re-encrypt round-trip.
            return self.inner.rename(from, to).await;
        }
        // Crossing the exempt boundary changes the required at-rest form. Read
        // THROUGH this wrapper (decrypts a sealed non-exempt source; passthrough
        // for a plaintext exempt source) then write THROUGH it (seals a non-exempt
        // dest; passthrough for an exempt dest), so the destination is stored in
        // the form its future reads expect. Then drop the source.
        let plaintext = self.read(from).await?;
        self.write_atomic(to, &plaintext).await?;
        self.inner.delete(from).await
    }
}

#[cfg(all(test, feature = "native"))]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::filesystem::NativeFilesystem;

    const KEY: [u8; 32] = [7u8; 32];

    fn setup() -> (tempfile::TempDir, EncryptedFilesystem, Arc<NativeFilesystem>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let raw = Arc::new(NativeFilesystem::new());
        let enc = EncryptedFilesystem::new(raw.clone(), &KEY);
        (dir, enc, raw)
    }

    fn p(dir: &tempfile::TempDir, name: &str) -> String {
        dir.path().join(name).to_string_lossy().into_owned()
    }

    /// Round trip: the wrapper writes ciphertext (magic present, plaintext
    /// absent on the raw filesystem) and reads the plaintext back.
    #[tokio::test]
    async fn round_trip_seals_at_rest_and_reads_back() {
        let (dir, enc, raw) = setup();
        let path = p(&dir, ".lh_history.json");
        let plain = b"the conversation history nobody should read at rest";

        enc.write_atomic(&path, plain).await.unwrap();

        let on_disk = raw.read(&path).await.unwrap();
        assert!(EncryptedFilesystem::looks_sealed(&on_disk), "missing LHE1 framing");
        assert!(
            !on_disk
                .windows(plain.len())
                .any(|w| w == plain.as_slice()),
            "plaintext leaked into the at-rest bytes"
        );
        assert_eq!(on_disk.len(), MAGIC.len() + NONCE_LEN + plain.len() + TAG_LEN);

        assert_eq!(enc.read(&path).await.unwrap(), plain);
    }

    /// Transparent migration: a pre-existing plaintext file (no magic)
    /// reads through unchanged — old profiles stay readable forever.
    #[tokio::test]
    async fn legacy_plaintext_reads_through_unchanged() {
        let (dir, enc, raw) = setup();
        let path = p(&dir, ".lh_system_prompt.txt");
        let legacy = b"You are a helpful agent.";
        raw.write_atomic(&path, legacy).await.unwrap();

        assert_eq!(enc.read(&path).await.unwrap(), legacy);
    }

    /// A magic-prefixed file that is too short to be ours passes through
    /// as plaintext instead of erroring.
    #[tokio::test]
    async fn short_magic_prefixed_plaintext_passes_through() {
        let (dir, enc, raw) = setup();
        let path = p(&dir, "notes.txt");
        let almost = b"LHE1 but actually just a short note"; // < MIN? no — long enough...
        // Use a genuinely-too-short payload for the length branch:
        let tiny = b"LHE1tiny";
        assert!(tiny.len() < MIN_SEALED_LEN);
        raw.write_atomic(&path, tiny).await.unwrap();
        assert_eq!(enc.read(&path).await.unwrap(), tiny);

        // And document the known edge: a ≥32-byte plaintext starting with
        // LHE1 IS treated as ciphertext (GCM rejects it with a clear error).
        raw.write_atomic(&path, almost).await.unwrap();
        assert!(enc.read(&path).await.is_err());
    }

    /// Tamper rejection: flipping one ciphertext byte fails GCM auth with
    /// a CLEAR error naming the path — never silent garbage bytes.
    #[tokio::test]
    async fn tampered_ciphertext_is_rejected_with_clear_error() {
        let (dir, enc, raw) = setup();
        let path = p(&dir, "secret.txt");
        enc.write_atomic(&path, b"integrity matters").await.unwrap();

        let mut sealed = raw.read(&path).await.unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        raw.write_atomic(&path, &sealed).await.unwrap();

        let err = enc.read(&path).await.expect_err("tamper must not decrypt");
        let msg = err.to_string();
        assert!(
            msg.contains("at-rest decrypt failed") && msg.contains("secret.txt"),
            "unclear tamper error: {msg}"
        );
    }

    /// Wrong key (e.g. a different seed) fails cleanly, not with garbage.
    #[tokio::test]
    async fn wrong_key_is_rejected_not_garbage() {
        let (dir, enc, raw) = setup();
        let path = p(&dir, "secret.txt");
        enc.write_atomic(&path, b"sealed under key A").await.unwrap();

        let other = EncryptedFilesystem::new(raw.clone(), &[8u8; 32]);
        assert!(other.read(&path).await.is_err());
    }

    /// The identity/boot files are written PLAINTEXT through the wrapper —
    /// `.lh_wallet` is the decryption root (sealing it bricks identity).
    #[tokio::test]
    async fn exempt_identity_files_stay_plaintext_on_disk() {
        let (dir, enc, raw) = setup();
        for name in EXEMPT_FILES {
            let path = p(&dir, name);
            let body = format!("contents of {name}");
            enc.write_atomic(&path, body.as_bytes()).await.unwrap();
            assert_eq!(
                raw.read(&path).await.unwrap(),
                body.as_bytes(),
                "{name} must NEVER be encrypted at rest"
            );
            // Reading back through the wrapper also returns the plaintext.
            assert_eq!(enc.read(&path).await.unwrap(), body.as_bytes());
        }
    }

    /// I1: an exempt file whose RAW bytes happen to look sealed (magic +
    /// ≥ MIN_SEALED_LEN, e.g. an unrelated ciphertext like a `.lh_device_key`
    /// blob) reads back VERBATIM — `read` honors the exemption symmetrically
    /// with `write_atomic` instead of routing it through the seed-key GCM
    /// decrypt (which would fail auth and lose the key).
    #[tokio::test]
    async fn exempt_file_with_sealed_looking_bytes_reads_verbatim() {
        let (dir, enc, raw) = setup();
        let path = p(&dir, ".lh_device_key");
        let mut blob = MAGIC.to_vec();
        blob.extend_from_slice(&[0xABu8; 40]);
        assert!(EncryptedFilesystem::looks_sealed(&blob), "test blob must look sealed");
        // Stored verbatim by the exempt write path...
        enc.write_atomic(&path, &blob).await.unwrap();
        assert_eq!(raw.read(&path).await.unwrap(), blob);
        // ...and read back verbatim, NOT decrypt-attempted.
        assert_eq!(enc.read(&path).await.unwrap(), blob);
    }

    /// PINNED exemption list. Removing `.lh_wallet` (the seed — the key
    /// derives FROM it) would brick every identity; the others are
    /// pre-wallet boot reads or public model artifacts. Adding entries is
    /// fine; update this pin deliberately.
    #[test]
    fn exempt_list_is_pinned() {
        assert_eq!(
            EXEMPT_FILES,
            &[
                ".lh_wallet",
                ".lh_owner",
                ".lh_linked_owner",
                ".lh_device_key",
                ".lh_local_model.safetensors",
                ".lh_local_tokenizer.json",
                ".lh_notif_pending.json",
                ".lh_notif_inbox.json",
            ],
            "exemption list changed — verify the boot path + seed safety before re-pinning"
        );
        assert!(
            EncryptedFilesystem::is_exempt("some/dir/.lh_wallet"),
            "exemption must match on the file name regardless of directory"
        );
        assert!(!EncryptedFilesystem::is_exempt(".lh_history.json"));
    }

    /// Trailing path separators must not smuggle an exempt file past the
    /// name match (else `.lh_wallet/` would get sealed → identity brick).
    #[test]
    fn is_exempt_strips_trailing_separators() {
        assert!(EncryptedFilesystem::is_exempt(".lh_wallet"));
        assert!(EncryptedFilesystem::is_exempt(".lh_wallet/"));
        assert!(EncryptedFilesystem::is_exempt("a/b/.lh_owner\\"));
        assert!(!EncryptedFilesystem::is_exempt("notes.txt"));
    }

    /// Rename moves the ciphertext verbatim and it stays decryptable at
    /// the new path (ciphertext is not path-bound).
    #[tokio::test]
    async fn rename_preserves_decryptability() {
        let (dir, enc, raw) = setup();
        let from = p(&dir, "draft.txt");
        let to = p(&dir, "final.txt");
        enc.write_atomic(&from, b"movable secret").await.unwrap();

        enc.rename(&from, &to).await.unwrap();

        assert!(EncryptedFilesystem::looks_sealed(&raw.read(&to).await.unwrap()));
        assert_eq!(enc.read(&to).await.unwrap(), b"movable secret");
    }

    /// Crossing the EXEMPT boundary re-forms the at-rest bytes: a sealed
    /// non-exempt file renamed to an exempt name is stored PLAINTEXT, and a
    /// plaintext exempt file renamed to a non-exempt name is stored SEALED —
    /// so each destination reads back correctly.
    #[tokio::test]
    async fn rename_across_exempt_boundary_reforms_at_rest_bytes() {
        let (dir, enc, raw) = setup();
        // sealed non-exempt -> exempt name: dest must be stored as PLAINTEXT.
        let sealed = p(&dir, ".lh_history.json");
        let exempt = p(&dir, ".lh_device_key");
        enc.write_atomic(&sealed, b"secret").await.unwrap();
        assert!(EncryptedFilesystem::looks_sealed(&raw.read(&sealed).await.unwrap()));
        enc.rename(&sealed, &exempt).await.unwrap();
        assert_eq!(raw.read(&exempt).await.unwrap(), b"secret", "exempt dest stored verbatim");
        assert_eq!(enc.read(&exempt).await.unwrap(), b"secret");
        // plaintext exempt -> non-exempt name: dest must be SEALED at rest.
        let back = p(&dir, ".lh_lessons.txt");
        enc.rename(&exempt, &back).await.unwrap();
        assert!(EncryptedFilesystem::looks_sealed(&raw.read(&back).await.unwrap()), "non-exempt dest sealed");
        assert_eq!(enc.read(&back).await.unwrap(), b"secret");
    }

    /// Two writes of the same plaintext produce different ciphertexts
    /// (fresh random nonce per seal) — no deterministic-encryption leak.
    #[tokio::test]
    async fn fresh_nonce_per_write() {
        let (dir, enc, raw) = setup();
        let a = p(&dir, "a.txt");
        let b = p(&dir, "b.txt");
        enc.write_atomic(&a, b"same plaintext").await.unwrap();
        enc.write_atomic(&b, b"same plaintext").await.unwrap();
        assert_ne!(raw.read(&a).await.unwrap(), raw.read(&b).await.unwrap());
    }

    /// Debug never prints key material.
    #[test]
    fn debug_redacts_key() {
        let raw = Arc::new(NativeFilesystem::new());
        let enc = EncryptedFilesystem::new(raw, &KEY);
        let dbg = format!("{enc:?}");
        assert!(dbg.contains("<redacted>"));
        assert!(!dbg.contains("7, 7, 7"));
    }
}
