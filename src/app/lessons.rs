//! OPFS-local working copy of the agent's self-recorded lessons.
//!
//! `.lh_lessons.txt` mirrors the on-chain blob stored under
//! `keccak256("localharness.lessons")` (written by the `record_lesson` tool;
//! merge semantics in `crate::lessons`). Session bootstrap reads the local
//! copy without an RPC round-trip; a device that has never recorded here
//! falls back to the on-chain slot for this tenant's tokenId — so lessons
//! survive sessions AND devices.

use crate::filesystem::Filesystem;

const LESSONS_FILE: &str = ".lh_lessons.txt";

/// Read the local lessons working copy. `None` when absent/empty.
pub(crate) async fn load_local() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(LESSONS_FILE).await.ok()?;
    let s = String::from_utf8(bytes).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Persist `content` as the local lessons working copy (atomic swap).
pub(crate) async fn save(content: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    fs.write_atomic(LESSONS_FILE, content.trim().as_bytes())
        .await
        .map_err(|e| e.to_string())
}

/// The lessons blob for THIS tenant: the OPFS working copy when present,
/// else the published on-chain `lessons_of` slot (second device / fresh
/// profile). `None` when no lessons exist anywhere (or not on a registered
/// tenant) — best-effort, an RPC failure degrades to no lessons.
pub(crate) async fn load() -> Option<String> {
    if let Some(local) = load_local().await {
        return Some(local);
    }
    let name = super::tenant::current_name()?;
    let id = super::registry::id_of_name(&name)
        .await
        .ok()
        .filter(|&id| id != 0)?;
    super::registry::lessons_of(id).await.ok().flatten()
}
