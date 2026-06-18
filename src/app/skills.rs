//! OPFS-local working copy of the agent's self-defined skills.
//!
//! `.lh_skills.json` mirrors the on-chain blob stored under
//! `keccak256("localharness.skills")` (written by the `create_skill` /
//! `delete_skill` tools; merge semantics in `crate::skills`). Session bootstrap
//! reads the local copy without an RPC round-trip; a device that has never
//! defined a skill here falls back to the on-chain slot for this tenant's
//! tokenId — so skills survive sessions AND devices.

const SKILLS_FILE: &str = ".lh_skills.json";

/// Read the local skills working copy (a JSON array). `None` when absent/empty.
pub(crate) async fn load_local() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(SKILLS_FILE).await.ok()?;
    let s = String::from_utf8(bytes).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Persist `content` (a JSON array) as the local skills working copy (atomic swap).
pub(crate) async fn save(content: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    fs.write_atomic(SKILLS_FILE, content.trim().as_bytes())
        .await
        .map_err(|e| e.to_string())
}

/// The skills blob for THIS tenant: the OPFS working copy when present, else
/// the published on-chain `skills_of` slot (second device / fresh profile).
/// `None` when no skills exist anywhere (or not on a registered tenant) —
/// best-effort, an RPC failure degrades to no skills.
pub(crate) async fn load() -> Option<String> {
    if let Some(local) = load_local().await {
        return Some(local);
    }
    let name = super::tenant::current_name()?;
    let id = super::registry::id_of_name(&name)
        .await
        .ok()
        .filter(|&id| id != 0)?;
    super::registry::skills_of(id).await.ok().flatten()
}
