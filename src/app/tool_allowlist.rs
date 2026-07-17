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

/// Every tool an agent can be granted, grouped for the admin grid: the
/// backend-neutral builtins PLUS the closure tools (`crate::agent_tools::
/// AGENT_TOOLS` — the canonical published surface).
///
/// The grid used to render `BuiltinTool::ALL` alone (19 of ~90). That wasn't
/// just under-reporting (telemetry #76) — closure tools had no checkbox, yet a
/// SAVE writes a non-empty allowlist, and `closure_tool_allowed` grants a
/// closure tool only when the list NAMES it. So saving the all-checked grid
/// silently revoked ~71 tools the owner never chose to remove.
pub(crate) fn all_tool_groups() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut groups: Vec<(&str, Vec<&str>)> = crate::agent_tools::AGENT_TOOLS
        .iter()
        .map(|(group, tools)| (*group, tools.to_vec()))
        .collect();
    // Builtins the published doc list doesn't carry (`configure_agent` is
    // golden; `run_command` is native-only) — the grid must still offer every
    // name a save could drop.
    let listed: Vec<&str> = groups.iter().flat_map(|(_, t)| t.clone()).collect();
    let extra: Vec<&str> = BuiltinTool::ALL
        .iter()
        .map(|t| t.wire_name())
        .filter(|n| !listed.contains(n))
        .collect();
    if !extra.is_empty() {
        groups.push(("Runtime", extra));
    }
    groups
}

/// Load the tool allowlist for this origin. Returns `None` when
/// unrestricted (all tools enabled).
pub(crate) async fn load() -> Option<Vec<BuiltinTool>> {
    super::agent_config::tool_allowlist().await
}

/// The saved allowlist as raw NAMES (builtins + closure tools), or `None` when
/// unrestricted. `load()` narrows to `BuiltinTool` for the capabilities config
/// and drops closure names; the grid needs them back to paint its checkboxes.
pub(crate) async fn load_names() -> Option<Vec<String>> {
    super::agent_config::tool_names().await
}

/// Persist `names` as the new allowlist — builtins AND closure tools, since
/// `closure_tool_allowed` matches on the raw name. An empty slice reverts to
/// unrestricted.
pub(crate) async fn save_names(names: &[String]) -> Result<(), String> {
    let arg = if names.is_empty() { None } else { Some(names) };
    super::agent_config::set_tool_names(arg).await
}

/// Whether a NON-builtin closure tool (e.g. the `set_persona` self-edit tool)
/// is permitted by this agent's config. Unrestricted agents get it; a
/// restrictive allowlist must LIST the name to grant it — so a low-autonomy
/// agent never receives `set_persona`. Thin wrapper over
/// [`super::agent_config::closure_tool_allowed`].
pub(crate) async fn closure_tool_allowed(name: &str) -> bool {
    super::agent_config::closure_tool_allowed(name).await
}

/// Return a human-readable summary for the admin UI.
pub(crate) fn summary(names: &[String]) -> String {
    if names.is_empty() {
        return "all tools enabled".to_string();
    }
    format!("{} of {} tools enabled", names.len(), total_tools())
}

/// How many distinct tools the grid offers — the honest denominator.
pub(crate) fn total_tools() -> usize {
    all_tool_groups().iter().map(|(_, t)| t.len()).sum()
}
