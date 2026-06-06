//! Cross-subdomain secure folder — apex-side encrypted store (SCAFFOLD).
//!
//! ## What this is
//! A folder the OWNER can read/write from ANY of their subdomains. OPFS is
//! per-origin sandboxed, so there is no natural shared store: a file written
//! on `a.localharness.xyz` is invisible to `b.localharness.xyz`. The only
//! origin every one of the owner's identities resolves to is the APEX
//! (`localharness.xyz`), which already holds the master seed
//! ([`super::wallet_store`]). So the shared folder lives in **apex OPFS**
//! under `.lh_shared/`, ENCRYPTED-AT-REST under a seed-derived AES-256-GCM
//! key ([`super::encryption::sharedfs_key_from_entropy`]). Plaintext never
//! touches disk.
//!
//! ## What ships here (the scaffold)
//! The APEX-SIDE storage helpers + data types only:
//! - [`SharedEntry`] — a listed shared-folder entry (name + byte size).
//! - [`apex_write`] / [`apex_read`] / [`apex_list`] / [`apex_delete`] —
//!   read/write/list/delete the seed-encrypted `.lh_shared/` folder from the
//!   apex origin (where the seed is first-party). These power both an
//!   apex-local shared-folder UI and the round-trip broker below.
//! - [`seal_file_for`] — the round-trip authorization boundary: returns a
//!   shared file ECIES-sealed to a subdomain's ephemeral key, but ONLY if
//!   this device's seed OWNS the requesting name on-chain (mirrors
//!   [`super::seed_pull`]'s `seal_seed_for`).
//!
//! ## What is DEFERRED (NOT shipped — see `design/` / CLAUDE.md)
//! - The mount-routing branches that drive the top-level apex round-trip
//!   (`?sharedfs_read=1` on apex, `?sharedfs_in=1` on the tenant) — the
//!   transport must reuse [`super::seed_pull`]'s **top-level navigation**
//!   pattern, NEVER an iframe (the cross-origin signer iframe is
//!   partition-dead on mobile — CLAUDE.md hard gotcha).
//! - WRITE FROM A SUBDOMAIN (the inverse round-trip + URL-fragment payload
//!   chunking). v1 is apex-write + subdomain-READ.
//! - The OPFS-panel `[pull from shared]` button + wiring the pulled bytes
//!   into the editor/display panel.
//!
//! This module deliberately does NOT advertise a finished feature: it is the
//! durable, encrypted apex store + the authorization seal, which the
//! deferred round-trip wiring builds on without reopening the crypto design.

use crate::filesystem::{EntryKind, Filesystem};

/// Directory in apex OPFS holding the owner's shared folder, sibling of
/// `.lh_wallet`. Every file under here is sealed at rest with the
/// seed-derived [`super::encryption::sharedfs_key_from_entropy`] key.
const SHARED_DIR: &str = ".lh_shared";

/// One entry in the shared folder, as surfaced to a listing UI / a
/// `sharedfs_list` round-trip manifest. `size` is the DECRYPTED logical
/// size when known, else the on-disk ciphertext length.
#[derive(Debug, Clone)]
pub(crate) struct SharedEntry {
    /// File name only (no path components, no `.lh_shared/` prefix).
    pub(crate) name: String,
    /// Size in bytes (ciphertext length on disk; the GCM overhead is
    /// IV(12)+tag(16) over the plaintext).
    pub(crate) size: u64,
}

/// Reject anything that could escape `.lh_shared/`. The path component
/// arrives from a subdomain URL in the round-trip, so this guard is
/// load-bearing: no traversal, no absolute paths, no nesting (flat folder
/// for v1), non-empty, bounded length.
fn path_is_safe(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 128
        && !path.contains("..")
        && !path.contains('/')
        && !path.contains('\\')
        && !path.starts_with('.')
}

/// Full apex-OPFS path for a shared file. Caller MUST have passed the name
/// through [`path_is_safe`] first.
fn opfs_path(name: &str) -> String {
    format!("{SHARED_DIR}/{name}")
}

/// Derive the at-rest seal key from the loaded master wallet's seed. Returns
/// `None` when this origin holds no seed (a non-apex / visitor device) — the
/// caller then has nothing to read or write.
async fn shared_key() -> Option<[u8; 32]> {
    let wallet = super::wallet_store::load().await?;
    let entropy = wallet.mnemonic.to_entropy();
    Some(super::encryption::sharedfs_key_from_entropy(&entropy))
}

/// Apex-side WRITE: seal `plaintext` under the seed key and store it at
/// `.lh_shared/<name>`. Must run on the apex origin (where the seed lives).
/// Returns `Err` on an unsafe name, a missing seed, or an OPFS failure.
pub(crate) async fn apex_write(name: &str, plaintext: &[u8]) -> Result<(), String> {
    if !path_is_safe(name) {
        return Err("invalid shared-file name".into());
    }
    let key = shared_key().await.ok_or("no master seed on this origin")?;
    let sealed = super::encryption::seal_with_raw_key(&key, plaintext)
        .await
        .ok_or("seal failed")?;
    super::shared_opfs()
        .write_atomic(&opfs_path(name), &sealed)
        .await
        .map_err(|e| format!("shared write: {e}"))
}

/// Apex-side READ: load `.lh_shared/<name>` and decrypt it under the seed
/// key. Returns `Ok(None)` when the file does not exist (or isn't our
/// ciphertext); `Err` only on an unsafe name or a missing seed.
pub(crate) async fn apex_read(name: &str) -> Result<Option<Vec<u8>>, String> {
    if !path_is_safe(name) {
        return Err("invalid shared-file name".into());
    }
    let key = shared_key().await.ok_or("no master seed on this origin")?;
    let fs = super::shared_opfs();
    let stored = match fs.read(&opfs_path(name)).await {
        Ok(b) if !b.is_empty() => b,
        _ => return Ok(None),
    };
    Ok(super::encryption::open_with_raw_key(&key, &stored).await)
}

/// Apex-side LIST: enumerate `.lh_shared/` as [`SharedEntry`] rows (files
/// only, sorted by name). An absent folder lists as empty. Reading the
/// folder needs no seed (names + sizes aren't secret); decrypting any file
/// still does.
pub(crate) async fn apex_list() -> Vec<SharedEntry> {
    let fs = super::shared_opfs();
    let mut out: Vec<SharedEntry> = match fs.read_dir(SHARED_DIR).await {
        Ok(entries) => entries
            .into_iter()
            .filter(|e| matches!(e.kind, EntryKind::File))
            .map(|e| SharedEntry {
                name: e.name,
                size: e.size.unwrap_or(0),
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Apex-side DELETE: remove `.lh_shared/<name>`. Idempotent-ish — a missing
/// file surfaces the backend's delete error, which the caller may ignore.
pub(crate) async fn apex_delete(name: &str) -> Result<(), String> {
    if !path_is_safe(name) {
        return Err("invalid shared-file name".into());
    }
    super::shared_opfs()
        .delete(&opfs_path(name))
        .await
        .map_err(|e| format!("shared delete: {e}"))
}

/// Round-trip authorization boundary (apex side). Given a requesting
/// subdomain `to`, a shared-file `path`, and the subdomain's ephemeral
/// compressed-SEC1 public key `epk` (hex), return the file's plaintext
/// **ECIES-sealed to `epk`** — but ONLY if this device's seed OWNS `to`
/// on-chain. A visitor's apex (a different seed) or a non-owned name yields
/// `None`, so the requester learns nothing. Mirrors
/// [`super::seed_pull`]'s `seal_seed_for` exactly.
///
/// The returned ciphertext is safe to ride a URL fragment back to the
/// subdomain (decryptable only by the ephemeral private key the subdomain
/// stashed before navigating). The mount-routing that performs the actual
/// top-level navigation is the DEFERRED half (see the module doc).
pub(crate) async fn seal_file_for(
    to: &str,
    path: &str,
    epk: &[u8],
) -> Option<Vec<u8>> {
    if !path_is_safe(path) || epk.is_empty() {
        return None;
    }
    // Authorization: this seed must own `to` on-chain (same guard as the
    // seed-pull broker). A non-owner returns None → harmless bounce.
    let wallet = super::wallet_store::load().await?;
    let owner = super::registry::owner_of_name(to).await.ok().flatten()?;
    if !owner.eq_ignore_ascii_case(&wallet.address_hex()) {
        return None;
    }
    let plain = apex_read(path).await.ok().flatten()?;
    super::encryption::ecies_seal(epk, &plain).await
}
