//! Per-tenant custom system prompt — the "studio MVP".
//!
//! Each subdomain can override the bundle's default system
//! instructions by writing to `.lh_system_prompt.txt` in its origin's
//! OPFS root. `chat::start_session` reads it on every session start
//! and appends it under an `=== Owner instructions ===` header so the
//! model sees the baseline tooling docs first, then the owner's
//! customization on top.
//!
//! This is the smallest step toward "the app IS the IDE for building
//! agents." Future expansions: tool allowlist per agent, model
//! selection, layout differentiation, etc. — all reduce to OPFS files
//! the bundle reads on mount.

use crate::filesystem::Filesystem;

const PROMPT_PATH: &str = ".lh_system_prompt.txt";

/// Read the custom prompt for this origin. Returns `None` when the
/// file doesn't exist or is empty.
pub(crate) async fn load() -> Option<String> {
    let fs = super::shared_opfs();
    let bytes = fs.read(PROMPT_PATH).await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

/// Persist `content` as the new custom prompt. Empty / whitespace-only
/// content deletes the file entirely (i.e., reverts to the default
/// system prompt on the next session).
pub(crate) async fn save(content: &str) -> Result<(), String> {
    let fs = super::shared_opfs();
    let trimmed = content.trim();
    if trimmed.is_empty() {
        // Best-effort delete; ignore not-found errors.
        let _ = fs.delete(PROMPT_PATH).await;
        return Ok(());
    }
    fs.write_atomic(PROMPT_PATH, trimmed.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))
}
