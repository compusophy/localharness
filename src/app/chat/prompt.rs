//! The in-tab agent's base system prompt — thin wrapper over the pure,
//! natively-tested `crate::session_prompt` (hoisted per the `turn_flow`
//! pattern so fact-pins + the size budget run under `cargo test`). This
//! wrapper's ONLY job is supplying the runtime facts a pure module can't
//! read: the live chain name.

/// sessionStorage key selecting the prompt variant for THIS session. The
/// ablation A/B switch (the `lh_router` opt-out pattern): `"lean"` selects
/// [`crate::session_prompt::lean_system_prompt`]; anything else (or unset,
/// the default) selects the full base prompt. EXPERIMENT-ONLY, no UI —
/// the measurement harness (and a curious owner) sets it from the console:
/// `sessionStorage.setItem('lh_prompt', 'lean')`, then start a new session.
const PROMPT_VARIANT_KEY: &str = "lh_prompt";

/// Build the base system instruction for the in-tab agent — see
/// [`crate::session_prompt`] for the content contract and the editing rules
/// (telemetry-earned; read them before touching text).
pub(crate) fn base_system_prompt(
    agent_name: &str,
    on_anthropic: bool,
    set_persona_allowed: bool,
) -> String {
    // The active network is runtime-selected (testnet vs mainnet via
    // `LH_CHAIN`/feature) — never hardcode it, or the prompt drifts from the
    // live deployment (on-chain feedback: said "Moderato" on mainnet).
    let network = crate::registry::chain::active().name;
    let lean = crate::app::dom::session_storage()
        .ok()
        .flatten()
        .and_then(|s| s.get_item(PROMPT_VARIANT_KEY).ok().flatten())
        .is_some_and(|v| v == "lean");
    if lean {
        crate::session_prompt::lean_system_prompt(
            agent_name,
            network,
            on_anthropic,
            set_persona_allowed,
        )
    } else {
        crate::session_prompt::base_system_prompt(
            agent_name,
            network,
            on_anthropic,
            set_persona_allowed,
        )
    }
}
