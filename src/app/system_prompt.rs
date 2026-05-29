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

//! Now a thin wrapper over [`super::agent_config`] — the prompt lives in
//! the `agent.json` manifest (with one-time migration from the legacy
//! `.lh_system_prompt.txt`).

/// Read the custom prompt for this origin. Returns `None` for the default.
pub(crate) async fn load() -> Option<String> {
    super::agent_config::system_prompt().await
}

/// Persist `content` as the new custom prompt. Empty / whitespace-only
/// content reverts to the default system prompt on the next session.
pub(crate) async fn save(content: &str) -> Result<(), String> {
    super::agent_config::set_system_prompt(Some(content)).await
}
