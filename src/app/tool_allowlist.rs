//! Per-tenant tool allowlist — studio v2.
//!
//! Each subdomain can restrict which built-in tools the agent exposes
//! by writing tool names (one per line) to `.lh_tool_allowlist.txt` in
//! its origin's OPFS root. When the file exists and is non-empty,
//! `chat::start_session` builds a `CapabilitiesConfig` with only those
//! tools enabled. When the file is absent or empty, all tools are
//! available (unrestricted mode — the default).
//!
//! Follows the same OPFS file pattern as `system_prompt.rs`.

use crate::filesystem::Filesystem;
use crate::types::BuiltinTool;

const ALLOWLIST_PATH: &str = ".lh_tool_allowlist.txt";

/// Load the tool allowlist for this origin. Returns `None` when the
/// file doesn't exist or is empty (meaning: unrestricted).
pub(crate) async fn load() -> Option<Vec<BuiltinTool>> {
    let fs = super::shared_opfs();
    let bytes = fs.read(ALLOWLIST_PATH).await.ok()?;
    if bytes.is_empty() {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    let tools: Vec<BuiltinTool> = text
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            BuiltinTool::ALL.iter().find(|t| t.wire_name() == trimmed).copied()
        })
        .collect();
    if tools.is_empty() {
        return None;
    }
    Some(tools)
}

/// Persist `tools` as the new allowlist. An empty slice deletes the
/// file (reverts to unrestricted). `finish` is always implicitly
/// included even if the caller omits it.
pub(crate) async fn save(tools: &[BuiltinTool]) -> Result<(), String> {
    let fs = super::shared_opfs();
    if tools.is_empty() {
        let _ = fs.delete(ALLOWLIST_PATH).await;
        return Ok(());
    }
    let mut lines: Vec<&str> = tools.iter().map(|t| t.wire_name()).collect();
    if !lines.contains(&"finish") {
        lines.push("finish");
    }
    lines.sort();
    lines.dedup();
    let content = lines.join("\n");
    fs.write_atomic(ALLOWLIST_PATH, content.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))
}

/// Return a human-readable summary for the admin UI.
pub(crate) fn summary(tools: &[BuiltinTool]) -> String {
    if tools.is_empty() {
        return "all tools enabled".to_string();
    }
    let names: Vec<&str> = tools.iter().map(|t| t.wire_name()).collect();
    format!("{} tools: {}", names.len(), names.join(", "))
}
