//! Session bootstrap — `start_session` builds the in-tab Agent: resolves the
//! backend (Gemini / Anthropic / local), assembles the system prompt
//! (`prompt::base_system_prompt` + self-docs digest + owner instructions +
//! self-recorded lessons), registers every closure tool, and seeds prior
//! history.

use std::rc::Rc;

use wasm_bindgen::JsValue;

use crate::app::APP;
use crate::policy;
use crate::types::ThinkingLevel;
use crate::{Agent, CapabilitiesConfig, GeminiAgentConfig};

use super::prompt::base_system_prompt;
use super::tools::bounty::{
    accept_result_tool, claim_bounty_tool, discover_bounties_tool, post_bounty_tool,
    submit_result_tool,
};
use super::tools::governance::{
    cast_vote_tool, execute_proposal_tool, list_proposals_tool, propose_measure_tool,
};
use super::tools::guild::{
    create_guild_tool, fund_guild_tool, invite_to_guild_tool, list_my_guilds_tool,
    spend_treasury_tool,
};
use super::tools::misc::{
    clear_context_tool, compact_context_tool, notify_tool, record_lesson_tool,
    set_persona_tool, spawn_recursive_subagent_tool, submit_feedback_tool,
};
use super::tools::platform::{
    batch_create_subdomains_tool, bulk_release_subdomains_tool, create_and_publish_app_tool,
    create_subdomain_tool, discover_agents_tool, list_subdomains_tool, release_subdomain_tool,
    batch_send_lh_tool, check_balances_tool, send_lh_tool,
};
use super::{ANTHROPIC_MAX_OUTPUT_TOKENS, GEMINI_MAX_OUTPUT_TOKENS};

pub(crate) async fn start_session(
    key: &str,
    base_url: Option<url::Url>,
    identity: &str,
) -> Result<(), JsValue> {
    // System instruction — the agent needs to know what it's running
    // inside and what its filesystem looks like. Without this, prompts
    // like "what is pricing" produce blind tool calls because the
    // model has no priors about the localharness environment.
    let host = crate::app::tenant::current();
    let agent_name = match &host {
        crate::app::tenant::Host::Tenant(name) => name.clone(),
        _ => "this agent".to_string(),
    };

    // Which LLM backend this session uses — needed up front so the prompt
    // advertises ONLY the tools the chosen backend actually registers. The
    // Anthropic backend reuses the Gemini `register_builtins` with both
    // client slots `None`, so the two Gemini-client-coupled builtins
    // (`start_subagent`, `generate_image`) do NOT register on Claude. Gate
    // their prompt lines on the backend so a Claude agent is never told it
    // has tools it can't call.
    let model = crate::app::model::load().await;
    let on_anthropic = crate::app::model::is_anthropic(&model);

    // SELF-EDIT GATE (computed up here so both the prompt line AND the tool
    // registration agree). `set_persona` lets the agent rewrite its own system
    // instruction — a higher-autonomy tool, so it's only granted when the
    // allowlist permits it (unrestricted agents qualify; a restrictive allowlist
    // must list `set_persona`). Low-autonomy agents are never told about it.
    let set_persona_allowed = crate::app::tool_allowlist::closure_tool_allowed("set_persona").await;

    let system_instructions =
        base_system_prompt(&agent_name, on_anthropic, set_persona_allowed);

    // Self-knowledge: append a concise runtime digest so the agent has
    // grounded priors about its OWN platform/SDK every turn (and knows it
    // can read the full live spec via read_self_docs). This is the
    // always-available, offline half of feature 1b.
    let system_instructions = format!(
        "{system_instructions}\n\n{}",
        crate::app::self_docs::system_prompt_digest()
    );

    // Owner customization: append the contents of `.lh_system_prompt.txt`
    // (if any) under a clear header so the model sees the baked-in
    // tooling docs first, then the owner's overrides on top. This is
    // the studio-MVP hook — owners differentiate their agent's
    // personality / role / constraints without forking the bundle.
    let system_instructions = match crate::app::system_prompt::load().await {
        Some(custom) => {
            format!("{system_instructions}\n\n=== Owner instructions ===\n{custom}")
        }
        None => system_instructions,
    };

    // Self-recorded lessons: fold in the bounded lessons blob (OPFS working
    // copy, else the on-chain slot) so a mistake corrected once stays
    // corrected across sessions and devices — the read half of the lessons
    // loop (`record_lesson` is the write half).
    let system_instructions = match crate::app::lessons::load()
        .await
        .as_deref()
        .and_then(crate::lessons::compose_section)
    {
        Some(section) => format!("{system_instructions}\n\n{section}"),
        None => system_instructions,
    };

    let mut capabilities = match crate::app::tool_allowlist::load().await {
        Some(mut tools) => {
            // Always union the golden tools so neither the owner nor the
            // agent can disable recovery (finish / ask_question /
            // configure_agent).
            for golden in crate::app::tool_allowlist::GOLDEN {
                if !tools.contains(golden) {
                    tools.push(*golden);
                }
            }
            let mut caps = CapabilitiesConfig::unrestricted();
            caps.enabled_tools = Some(tools);
            caps
        }
        None => CapabilitiesConfig::unrestricted(),
    };
    // ENABLE AUTO-COMPACTION. `unrestricted()` leaves `compaction_threshold =
    // None`, which DISABLES it (`should_compact` always false) — so the in-tab
    // conversation grew unbounded until it overflowed the model's context window
    // and the turn came back empty ("(empty response)" reliably after a certain
    // length). The backend (gemini/anthropic loop.rs) compares this against
    // `usage.prompt_token_count` (the LIVE context size) and, when crossed,
    // summarizes the old prefix before the next turn (compaction.rs). Conservative
    // ceiling — set well under any plausible window so it ALWAYS trips before an
    // overflow; tunable, and a recency-weighted summarization scheme can retain far
    // more at this same ceiling.
    capabilities.compaction_threshold = Some(128_000);

    // `model` (the owner's per-subdomain `.lh_model` choice) was loaded above
    // so the prompt could be gated to the backend. A `claude-*` id routes to
    // the Anthropic backend; everything else (the default `gemini-*`) to
    // Gemini. Both backends go through the SAME credit-proxy `base_url` in
    // credits mode (the proxy is multi-provider — Gemini on `/v1beta/*`,
    // Anthropic on `/v1/messages`) and carry the SAME `key` (the proxy auth
    // token, or a raw key in BYOK). BYOK only routes Gemini directly; a Claude
    // model on BYOK would need a raw Anthropic key, so the credit proxy is the
    // intended Claude path.
    let captured_key = key.to_string();

    // Credits mode signs a FRESH proxy auth token for EVERY request. The
    // proxy enforces a 5-minute freshness window on the signed token, so a
    // token baked in at session start dies mid-conversation — every later
    // request 401s "stale or future timestamp" (on-chain feedback #46) and
    // long thinking turns break in the middle. BYOK (no proxy base_url)
    // keeps the static key.
    let auth_provider: Option<crate::backends::KeyProvider> = if base_url.is_some() {
        super::access::credit_signer().await.map(|(signer, _addr)| {
            std::sync::Arc::new(move || {
                let now = (js_sys::Date::now() / 1000.0) as u64;
                crate::registry::proxy_auth_token(&signer, now)
            }) as crate::backends::KeyProvider
        })
    } else {
        None
    };
    // History from a previous session (if any), consumed once here so a
    // backend switch doesn't lose the transcript.
    let pending_history = crate::app::history::take_pending();

    let agent = if crate::app::model::is_local(&model) {
        // In-browser local model (Gemma 3 270M via Burn-wgpu). No API key, no
        // proxy: weights are read from this origin's OPFS (downloaded once via
        // the model tab). The local backend speaks plain text — no tools — so
        // we pass only the system instructions + filesystem. History from a
        // prior session seeds only when it decodes as local history. Gated on
        // the heavy `local` feature; without it, the id can't be served here.
        #[cfg(feature = "local")]
        {
            let mut cfg = crate::LocalAgentConfig::new(model.clone())
                .with_capabilities(capabilities)
                .with_filesystem(crate::app::shared_opfs())
                .with_system_instructions(system_instructions);
            if let Some(bytes) = pending_history {
                if crate::backends::local::connection::decode_transcript_bytes(&bytes).is_ok() {
                    cfg = cfg.with_history_bytes(bytes);
                }
            }
            Agent::start_local(cfg)
                .await
                .map_err(|e| JsValue::from_str(&format!("start_local: {e}")))?
        }
        #[cfg(not(feature = "local"))]
        {
            // Keep the moved-in bindings live so the borrow checker is happy on
            // this (never-taken-in-practice) path, then surface a clear error.
            let _ = (&capabilities, &system_instructions, &pending_history);
            return Err(JsValue::from_str(
                "local model selected but this build was compiled without the `local` feature",
            ));
        }
    } else if crate::app::model::is_anthropic(&model) {
        let mut cfg = crate::AnthropicAgentConfig::new(key.to_string())
            .with_model(model.clone())
            .with_capabilities(capabilities)
            .with_policies(vec![policy::allow_all()])
            .with_pre_tool_hook(std::sync::Arc::new(super::dedup::DuplicateActionGuard))
            .with_filesystem(crate::app::shared_opfs())
            .with_system_instructions(system_instructions)
            // Parity with the Gemini path: give a hard task room to answer in
            // one call (the 8192 default is tight for a long reasoning turn).
            .with_max_tokens(ANTHROPIC_MAX_OUTPUT_TOKENS)
            .with_tool(create_subdomain_tool())
            .with_tool(create_and_publish_app_tool())
            .with_tool(batch_create_subdomains_tool())
            .with_tool(release_subdomain_tool())
            .with_tool(bulk_release_subdomains_tool())
            .with_tool(list_subdomains_tool())
            .with_tool(discover_agents_tool())
            .with_tool(send_lh_tool())
            .with_tool(batch_send_lh_tool())
            .with_tool(check_balances_tool())
            .with_tool(post_bounty_tool())
            .with_tool(claim_bounty_tool())
            .with_tool(submit_result_tool())
            .with_tool(accept_result_tool())
            .with_tool(discover_bounties_tool())
            .with_tool(create_guild_tool())
            .with_tool(invite_to_guild_tool())
            .with_tool(fund_guild_tool())
            .with_tool(spend_treasury_tool())
            .with_tool(list_my_guilds_tool())
            .with_tool(propose_measure_tool())
            .with_tool(cast_vote_tool())
            .with_tool(execute_proposal_tool())
            .with_tool(list_proposals_tool())
            .with_tool(submit_feedback_tool())
            .with_tool(notify_tool())
            .with_tool(record_lesson_tool())
            .with_tool(crate::app::self_docs::read_self_docs_tool())
            .with_tool(clear_context_tool())
            .with_tool(compact_context_tool())
            .with_tool(spawn_recursive_subagent_tool(captured_key, base_url.clone()));
        // Self-edit tool — gated on the allowlist (see `set_persona_allowed`).
        if set_persona_allowed {
            cfg = cfg.with_tool(set_persona_tool());
        }
        // Credits mode: route Anthropic through the credit proxy (it serves
        // `/v1/messages`). BYOK has no direct-Anthropic path here, so this is
        // a no-op without a proxy base_url and the call would hit
        // api.anthropic.com with the raw key.
        if let Some(b) = &base_url {
            cfg = cfg.with_base_url(b.clone());
        }
        if let Some(p) = auth_provider.clone() {
            cfg = cfg.with_auth_provider(p);
        }
        // The on-disk history is the LAST backend's wire format. Only seed it
        // into Anthropic when it actually parses as Anthropic history —
        // otherwise (e.g. switching from a Gemini session) start fresh rather
        // than failing the whole session start. The mount-time transcript
        // paint stays regardless, so the user still sees the prior turns.
        if let Some(bytes) = pending_history {
            if crate::backends::anthropic::decode_transcript_bytes(&bytes).is_ok() {
                cfg = cfg.with_history_bytes(bytes);
            }
        }
        Agent::start_anthropic(cfg)
            .await
            .map_err(|e| JsValue::from_str(&format!("start_anthropic: {e}")))?
    } else {
        let mut cfg = GeminiAgentConfig::new(key.to_string())
            .with_model(model.clone())
            .with_capabilities(capabilities)
            .with_policies(vec![policy::allow_all()])
            .with_pre_tool_hook(std::sync::Arc::new(super::dedup::DuplicateActionGuard))
            .with_filesystem(crate::app::shared_opfs())
            .with_system_instructions(system_instructions)
            // Give a hard task room to BOTH reason and answer in one call, and
            // bound reasoning (visible thinking) so it can't eat the whole
            // window — the fix for "(empty response)" on long tasks.
            .with_max_output_tokens(GEMINI_MAX_OUTPUT_TOKENS)
            // Deep-think for the coding-heavy in-tab path. High = a 16384 thinking
            // budget, which Gemini draws FROM the 32768 output cap above — leaving
            // ~16k guaranteed for the final answer / tool calls. So reasoning gets
            // real room to PLAN + reason about rustlite WITHOUT starving the output
            // (the "(empty response)" fix holds because budget ≥ 2× thinking). The
            // visible PLAN-FIRST + compile-in-the-loop discipline below leans on
            // this headroom.
            .with_thinking(ThinkingLevel::High)
            .with_tool(create_subdomain_tool())
            .with_tool(create_and_publish_app_tool())
            .with_tool(batch_create_subdomains_tool())
            .with_tool(release_subdomain_tool())
            .with_tool(bulk_release_subdomains_tool())
            .with_tool(list_subdomains_tool())
            .with_tool(discover_agents_tool())
            .with_tool(send_lh_tool())
            .with_tool(batch_send_lh_tool())
            .with_tool(check_balances_tool())
            .with_tool(post_bounty_tool())
            .with_tool(claim_bounty_tool())
            .with_tool(submit_result_tool())
            .with_tool(accept_result_tool())
            .with_tool(discover_bounties_tool())
            .with_tool(create_guild_tool())
            .with_tool(invite_to_guild_tool())
            .with_tool(fund_guild_tool())
            .with_tool(spend_treasury_tool())
            .with_tool(list_my_guilds_tool())
            .with_tool(propose_measure_tool())
            .with_tool(cast_vote_tool())
            .with_tool(execute_proposal_tool())
            .with_tool(list_proposals_tool())
            .with_tool(submit_feedback_tool())
            .with_tool(notify_tool())
            .with_tool(record_lesson_tool())
            .with_tool(crate::app::self_docs::read_self_docs_tool())
            .with_tool(clear_context_tool())
            .with_tool(compact_context_tool())
            .with_tool(spawn_recursive_subagent_tool(captured_key, base_url.clone()));
        // Self-edit tool — gated on the allowlist (see `set_persona_allowed`).
        if set_persona_allowed {
            cfg = cfg.with_tool(set_persona_tool());
        }
        // Credits mode: route the whole agent through the credit proxy. BYOK
        // leaves base_url None → direct to generativelanguage.googleapis.com.
        if let Some(b) = &base_url {
            cfg = cfg.with_base_url(b.clone());
        }
        if let Some(p) = auth_provider.clone() {
            cfg = cfg.with_auth_provider(p);
        }
        // If a previous session left history on OPFS, restore it into the
        // new connection. Consumed once — subsequent key changes start
        // fresh from the in-memory agent's history. Only seed it when it
        // parses as Gemini history (so switching back from a Claude session
        // doesn't fail the session start on an incompatible wire format).
        if let Some(bytes) = pending_history {
            if crate::backends::gemini::decode_transcript_bytes(&bytes).is_ok() {
                cfg = cfg.with_history_bytes(bytes);
            }
        }
        Agent::start_gemini(cfg)
            .await
            .map_err(|e| JsValue::from_str(&format!("start_gemini: {e}")))?
    };
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        app.agent = Some(Rc::new(agent));
        // Stable identity (address in credits mode, key in BYOK) — NOT the
        // rotating credits token, so the session isn't restarted per turn.
        app.session_key = Some(identity.to_string());
        app.turn_count = 0;
    });
    Ok(())
}
