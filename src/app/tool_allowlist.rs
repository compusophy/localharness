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

//! Now a thin wrapper over [`super::agent_config`] — the allowlist lives
//! in the `agent.json` manifest (with one-time migration from the legacy
//! `.lh_tool_allowlist.txt`). Golden tools that can never be disabled are
//! defined in [`GOLDEN`] and enforced at session start.

use crate::types::BuiltinTool;

/// Tools that are always available regardless of the allowlist, so the
/// owner (or the agent) can never lock themselves out of recovery:
/// `finish` (end a turn), `ask_question` (talk to the user), and
/// `configure_agent` (change/reset the config). Enforced in
/// `chat::start_session` by unioning these into the effective set.
pub(crate) const GOLDEN: &[BuiltinTool] = &[
    BuiltinTool::Finish,
    BuiltinTool::AskQuestion,
    BuiltinTool::ConfigureAgent,
];

/// Load the tool allowlist for this origin. Returns `None` when
/// unrestricted (all tools enabled).
pub(crate) async fn load() -> Option<Vec<BuiltinTool>> {
    super::agent_config::tool_allowlist().await
}

/// Persist `tools` as the new allowlist. An empty slice reverts to
/// unrestricted.
pub(crate) async fn save(tools: &[BuiltinTool]) -> Result<(), String> {
    let arg = if tools.is_empty() { None } else { Some(tools) };
    super::agent_config::set_tools(arg).await
}

/// Return a human-readable summary for the admin UI.
pub(crate) fn summary(tools: &[BuiltinTool]) -> String {
    if tools.is_empty() {
        return "all tools enabled".to_string();
    }
    let names: Vec<&str> = tools.iter().map(|t| t.wire_name()).collect();
    format!("{} tools: {}", names.len(), names.join(", "))
}
