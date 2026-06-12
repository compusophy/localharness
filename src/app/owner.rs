//! Self-correcting, on-chain-derived ownership HINT, OPFS-only.
//!
//! `.lh_owner` is NOT authority — the on-chain registry is. This file is
//! purely a first-paint flash-avoider: it stores the on-chain owner
//! ADDRESS this device last *proved* it controls (written only after a
//! `verify::VerifyResult::VerifiedOwner`). On the next load the presence
//! of the hint lets `paint_tenant` paint the studio immediately instead
//! of flashing the public face — but every load still re-verifies against
//! the chain, and the hint is deleted ([`forget`]) the moment the chain
//! disagrees. So the hint can never lie for more than the initial frame.
//!
//! It is per-origin (lives in the subdomain's OPFS sandbox). A different
//! device starts with no hint and earns one by proving ownership.


const OWNER_FILE: &str = ".lh_owner";

/// Read the on-chain owner address this device last proved it controls,
/// if any. Returns `None` when the hint is absent/empty.
pub(crate) async fn current_owner() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(OWNER_FILE).await.ok()?;
    let s = String::from_utf8(bytes).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Record `owner_address` as the proven on-chain owner of this origin.
/// Called only after a `VerifiedOwner` result (or a successful first
/// claim) so the next load paints the studio without a public-face flash.
/// Idempotent — overwrites any prior hint.
pub(crate) async fn remember(owner_address: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    fs.write_atomic(OWNER_FILE, owner_address.trim().as_bytes())
        .await
        .map_err(|e| e.to_string())
}

/// Delete the ownership hint. Called when the chain disagrees with the
/// optimistic studio paint (ownership lost / transferred) so the next
/// load starts from the public face — and via "release" / debug flows.
pub(crate) async fn forget() {
    let fs = super::shared_opfs();
    let _ = fs.delete(OWNER_FILE).await;
}
