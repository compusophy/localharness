//! The in-tab agent's base system prompt — thin wrapper over the pure,
//! natively-tested `crate::session_prompt` (hoisted per the `turn_flow`
//! pattern so fact-pins + the size budget run under `cargo test`). This
//! wrapper's ONLY job is supplying the runtime facts a pure module can't
//! read: the live chain name.

/// Build the base system instruction for the in-tab agent — see
/// [`crate::session_prompt::base_system_prompt`] for the content contract
/// and the editing rules (telemetry-earned; read them before touching text).
pub(crate) fn base_system_prompt(
    agent_name: &str,
    on_anthropic: bool,
    set_persona_allowed: bool,
) -> String {
    // The active network is runtime-selected (testnet vs mainnet via
    // `LH_CHAIN`/feature) — never hardcode it, or the prompt drifts from the
    // live deployment (on-chain feedback: said "Moderato" on mainnet).
    crate::session_prompt::base_system_prompt(
        agent_name,
        crate::registry::chain::active().name,
        on_anthropic,
        set_persona_allowed,
    )
}
