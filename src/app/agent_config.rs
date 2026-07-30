//! `agent.json` — the agent config manifest, single source of truth for
//! a subdomain's per-agent settings.
//!
//! Both front-ends write the same file: the admin UI (manual edits) and
//! the `configure_agent` builtin tool (the chat agent editing its own
//! config). Today it holds the custom system prompt and the tool
//! allowlist.
//!
//! `system_prompt.rs` and `tool_allowlist.rs` are thin wrappers over this
//! module so their callers (chat::start_session, the admin handlers) don't
//! need to know about the manifest.

use serde::{Deserialize, Serialize};

use crate::types::BuiltinTool;

const MANIFEST_PATH: &str = "agent.json";

/// The on-disk manifest. Absent fields mean "use the default" (built-in
/// system prompt; all tools enabled).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AgentManifest {
    /// Custom system prompt appended after the baseline tooling docs.
    /// `None` = the bundle default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    /// Allowlisted tool wire-names. `None` = all tools enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<String>>,
}

/// Read the manifest. Absent or unparsable `agent.json` = the defaults.
pub(crate) async fn load() -> AgentManifest {
    let fs = super::shared_opfs();
    if let Ok(bytes) = fs.read(MANIFEST_PATH).await {
        if let Ok(manifest) = serde_json::from_slice::<AgentManifest>(&bytes) {
            return manifest;
        }
    }
    AgentManifest::default()
}

/// Persist the manifest as pretty JSON.
pub(crate) async fn save(manifest: &AgentManifest) -> Result<(), String> {
    let fs = super::shared_opfs();
    let json = serde_json::to_vec_pretty(manifest).map_err(|e| format!("serialize: {e}"))?;
    fs.write_atomic(MANIFEST_PATH, &json)
        .await
        .map_err(|e| format!("write: {e}"))
}

/// Set (or clear, with `None`) the custom system prompt. Empty/whitespace
/// is treated as clear.
pub(crate) async fn set_system_prompt(prompt: Option<&str>) -> Result<(), String> {
    let mut manifest = load().await;
    manifest.system_prompt = prompt
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    save(&manifest).await
}

/// Set (or clear, with `None`) the tool allowlist.
pub(crate) async fn set_tools(tools: Option<&[BuiltinTool]>) -> Result<(), String> {
    let mut manifest = load().await;
    manifest.tools = tools
        .filter(|t| !t.is_empty())
        .map(|t| t.iter().map(|tool| tool.wire_name().to_string()).collect());
    save(&manifest).await
}

/// Persist an allowlist of raw tool NAMES — builtins and closure tools alike.
/// `closure_tool_allowed` matches on the name, so a list written from only
/// `BuiltinTool` values silently revokes every closure tool (telemetry #76).
pub(crate) async fn set_tool_names(names: Option<&[String]>) -> Result<(), String> {
    let mut manifest = load().await;
    manifest.tools = names.filter(|n| !n.is_empty()).map(|n| n.to_vec());
    save(&manifest).await
}

/// The saved allowlist as raw names, or `None` when unrestricted.
pub(crate) async fn tool_names() -> Option<Vec<String>> {
    load().await.tools
}

/// The resolved custom system prompt, or `None` for the default.
pub(crate) async fn system_prompt() -> Option<String> {
    load()
        .await
        .system_prompt
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Whether a NON-builtin closure tool name (e.g. `"set_persona"`) is permitted
/// by this agent's config. The allowlist is normally read as `BuiltinTool`s
/// (unknown wire-names are dropped), so a closure-tool name would otherwise be
/// silently invisible to the gate. Semantics mirror the builtin gate:
/// - no allowlist (unrestricted) → permitted (default agents are high-autonomy);
/// - an allowlist that LISTS the name → permitted;
/// - a restrictive allowlist that omits it → denied (low-autonomy agents can't
///   self-edit). This is how `set_persona` is allowlist-gated.
pub(crate) async fn closure_tool_allowed(name: &str) -> bool {
    match load().await.tools {
        None => true,                          // unrestricted = high autonomy
        Some(tools) => tools.iter().any(|t| t == name),
    }
}

/// The resolved tool allowlist as `BuiltinTool`s, or `None` for
/// unrestricted. Unknown wire-names are dropped.
pub(crate) async fn tool_allowlist() -> Option<Vec<BuiltinTool>> {
    let names = load().await.tools?;
    let tools: Vec<BuiltinTool> = names
        .iter()
        .filter_map(|n| BuiltinTool::ALL.iter().find(|t| t.wire_name() == n).copied())
        .collect();
    if tools.is_empty() {
        None
    } else {
        Some(tools)
    }
}
