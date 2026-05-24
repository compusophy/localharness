//! Per-subdomain ownership marker, OPFS-only.
//!
//! Mirrors self.tools' device-UUID model: when you "claim" a
//! subdomain, we write `.lh_owner` to that subdomain's OPFS root
//! containing a UUID. Only that UUID can edit. Lives entirely in the
//! per-origin OPFS sandbox — no central registry yet.
//!
//! **Limitation by design:** a different device visiting the same
//! subdomain has its own OPFS, so it sees the subdomain as unclaimed.
//! That's the price of zero-backend v1 — cross-device ownership is
//! handled by the on-chain registry path in [`super::registry`],
//! which this module is the legacy fallback to.

use crate::filesystem::Filesystem;

const OWNER_FILE: &str = ".lh_owner";

/// Read the persisted owner UUID for this origin, if any.
pub(crate) async fn current_owner() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(OWNER_FILE).await.ok()?;
    let s = String::from_utf8(bytes).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Write a fresh owner UUID to this origin's OPFS. Idempotent — if
/// already owned, overwrites (caller is expected to check first).
pub(crate) async fn claim() -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let fs = super::shared_opfs();
    fs.write_atomic(OWNER_FILE, id.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    Ok(id)
}

/// Drop the ownership marker. Used by "release" / debug flows.
#[allow(dead_code)]
pub(crate) async fn release() {
    let fs = super::shared_opfs();
    let _ = fs.delete(OWNER_FILE).await;
}
