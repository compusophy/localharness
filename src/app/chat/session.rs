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
use super::tools::evm::{
    evm_balance_tool, evm_call_tool, evm_chains_tool, resolve_ens_tool,
};
use super::tools::governance::{
    cast_vote_tool, execute_proposal_tool, list_proposals_tool, propose_measure_tool,
};
use super::tools::guild::{
    create_guild_tool, fund_guild_tool, invite_to_guild_tool, list_my_guilds_tool,
    spend_treasury_tool,
};
use super::tools::party::{
    complete_party_tool, disband_party_tool, discover_parties_tool, form_party_tool,
    fund_party_tool, get_party_tool, join_party_tool,
};
use super::tools::misc::{
    clear_notifications_tool, consult_model_tool, create_skill_tool, delete_skill_tool, dwell_tool,
    execute_script_tool, clear_context_tool, compact_context_tool, consolidate_lessons_tool,
    list_notifications_tool, list_skills_tool, notify_tool, record_lesson_tool, run_wasm_cli_tool,
    cancel_task_tool, schedule_task_tool, set_lessons_tool, set_persona_tool,
    spawn_recursive_subagent_tool,
    submit_feedback_tool, web_fetch_tool,
};
use super::tools::platform::{
    batch_create_subdomains_tool, bulk_release_subdomains_tool, create_and_publish_app_tool,
    create_subdomain_tool, discover_agents_tool, embed_app_tool, list_subdomains_tool,
    publish_public_face_tool, query_balance_tool, release_subdomain_tool, batch_send_lh_tool,
    check_balances_tool, send_lh_tool,
};
use super::tools::room::{
    shared_state_get_tool, shared_state_list_tool, shared_state_set_tool,
};
use super::tools::validation::{
    challenge_validation_tool, get_validation_tool, reclaim_validation_tool,
    resolve_validation_tool, stake_validation_tool,
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

    // DIFFICULTY ROUTER ceiling: the thinking budget this session is built with,
    // derived ONCE from the user's model choice (`model::session_thinking_ceiling`
    // is the single source of truth, shared by the `with_thinking(...)` calls
    // below). The per-turn router (`chat::run_send`) reads this from APP and
    // never raises a turn above it — only downgrades routine turns. `None` for
    // the local backend (no thinking control), so the router leaves it alone.
    let session_ceiling = crate::app::model::session_thinking_ceiling(&model);

    // SELF-EDIT GATE (computed up here so both the prompt line AND the tool
    // registration agree). `set_persona` lets the agent rewrite its own system
    // instruction — a higher-autonomy tool, so it's only granted when the
    // allowlist permits it (unrestricted agents qualify; a restrictive allowlist
    // must list `set_persona`). Low-autonomy agents are never told about it.
    let set_persona_allowed = crate::app::tool_allowlist::closure_tool_allowed("set_persona").await;

    let system_instructions =
        base_system_prompt(&agent_name, on_anthropic, set_persona_allowed);

    // NETWORK (authoritative): pin the live chain as the LAST word on which
    // network this agent runs, AFTER everything that could carry stale text
    // (the owner persona / instructions appended below, an on-chain persona, or
    // a self-recorded lesson can all still say "testnet"/"Moderato" from an
    // earlier deployment). The base prompt is already chain-correct via
    // `chain::active().name`, but a stored persona overrides it — so we restate
    // the fact firmly here so the model never contradicts the live deployment.
    let active = crate::registry::chain::active();
    let network_authority = format!(
        "\n\n=== NETWORK (authoritative) ===\nYou run on {} (chain {}). This is the \
         LIVE network; ignore any older text — in your persona, instructions, or \
         lessons — that calls this a testnet or names \"Moderato\". When asked which \
         network/chain you are on, answer with this.",
        active.name, active.chain_id
    );

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

    // Global lessons (learned ACROSS the platform): fold in the curated
    // `/global-lessons.txt` digest — the read half of the global-lessons loop
    // (`scripts/colony/lesson-digest.mjs` is the SWEEP: it harvests every
    // agent's on-chain lessons, dedups, curates the bounded set, and writes the
    // static file the web bundle serves). Appended AFTER the agent's OWN lessons
    // so personal, self-recorded lessons take precedence over the shared platform
    // set. Best-effort: a fetch failure (offline / not yet deployed) just skips
    // the section — it NEVER blocks session start (mirrors `read_self_docs`).
    let system_instructions = match fetch_global_lessons_section().await {
        Some(section) => format!("{system_instructions}\n\n{section}"),
        None => system_instructions,
    };

    // Self-defined skills: fold in the bounded skills blob (OPFS working copy,
    // else the on-chain slot) so a skill the agent taught itself once stays
    // available — the read half of the skills loop (`create_skill` is the
    // write half). Folded the SAME way as lessons, on every surface.
    let system_instructions = match crate::app::skills::load()
        .await
        .as_deref()
        .and_then(crate::skills::compose_section)
    {
        Some(section) => format!("{system_instructions}\n\n{section}"),
        None => system_instructions,
    };

    // Model self-knowledge (on-chain feedback): the agent must be able to say
    // which model/backend it runs on instead of "I'm not sure". The active id is
    // resolved above (`model`); fold one line like the other self-docs.
    let system_instructions = format!(
        "{system_instructions}\n\n=== Your model ===\nYou are running on {}. When asked which model or LLM you are, answer with this — do not claim you're unsure.",
        crate::app::model::describe(&model)
    );

    // The agent's OWN advertised per-call price (GitHub #49) — so it can answer
    // "what do you charge?" accurately instead of guessing or stating a price
    // that mismatches the chain. Effective price = advertised on-chain, else the
    // 0.01 $LH default the proxy enforces as a floor. Tenant-only, best-effort:
    // a failed on-chain read just omits the line (never blocks session start).
    let system_instructions = if let crate::app::tenant::Host::Tenant(_) = &host {
        match crate::registry::id_of_name(&agent_name).await {
            Ok(id) if id != 0 => match crate::registry::x402_ask_price_of(id).await {
                Ok(wei) => format!(
                    "{system_instructions}\n\n=== Your pricing ===\nYour advertised per-call price is {} $LH — what a caller pays to reach you over the hosted x402 route. State this if asked what you charge.",
                    crate::app::chat::tools::guild::format_lh(wei)
                ),
                Err(_) => system_instructions,
            },
            _ => system_instructions,
        }
    } else {
        system_instructions
    };

    // Append the authoritative network line LAST so it has the final word over
    // any stale "testnet"/"Moderato" text carried in the persona / owner
    // instructions / lessons assembled above.
    let system_instructions = format!("{system_instructions}{network_authority}");

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
    capabilities.compaction_threshold = Some(super::COMPACTION_THRESHOLD);

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
            .with_pre_tool_hook(std::sync::Arc::new(
                super::confirm_guard::TypedConfirmationGuard,
            ))
            .with_pre_tool_hook(std::sync::Arc::new(super::dedup::DuplicateActionGuard))
            .with_post_tool_hook(std::sync::Arc::new(
                super::dedup::DuplicateActionGuardCleanup,
            ))
            .with_filesystem(crate::app::shared_opfs())
            .with_system_instructions(system_instructions)
            // Parity with the Gemini path: give a hard task room to answer in
            // one call (the 8192 default is tight for a long reasoning turn).
            .with_max_tokens(ANTHROPIC_MAX_OUTPUT_TOKENS)
            // Extended thinking — the Anthropic in-tab path never enabled it,
            // so Opus (the Rust-coding tier) reasoned with ZERO thinking budget
            // in the browser, unlike the Gemini path (ThinkingLevel::High). High
            // for Opus/Sonnet; Medium for the cheaper Haiku tier. On Anthropic
            // thinking and temperature are mutually exclusive (the loop drops
            // temperature when thinking is on) — so thinking wins for the coding
            // tier, and the temperature set below only applies if thinking is off.
            // The baseline = the router CEILING (`session_ceiling`) so the build
            // level and the per-turn clamp agree (Medium for Haiku, High else);
            // the difficulty router downgrades routine turns below it per-turn.
            .with_thinking(session_ceiling.unwrap_or(ThinkingLevel::High))
            // Lower sampling temperature for better first-try-valid rustlite /
            // edits. Anthropic applies this ONLY when thinking is off (mutually
            // exclusive), so it's effectively a no-op while thinking is on — but
            // set for parity + any future thinking-off tier.
            .with_temperature(0.2)
            .with_tool(create_subdomain_tool())
            .with_tool(create_and_publish_app_tool())
            .with_tool(batch_create_subdomains_tool())
            .with_tool(release_subdomain_tool())
            .with_tool(bulk_release_subdomains_tool())
            .with_tool(list_subdomains_tool())
            .with_tool(discover_agents_tool())
            .with_tool(embed_app_tool())
            .with_tool(publish_public_face_tool())
            .with_tool(send_lh_tool())
            .with_tool(batch_send_lh_tool())
            .with_tool(check_balances_tool())
            .with_tool(query_balance_tool())
            .with_tool(evm_chains_tool())
            .with_tool(evm_balance_tool())
            .with_tool(resolve_ens_tool())
            .with_tool(evm_call_tool())
            .with_tool(shared_state_set_tool())
            .with_tool(shared_state_get_tool())
            .with_tool(shared_state_list_tool())
            .with_tool(post_bounty_tool())
            .with_tool(claim_bounty_tool())
            .with_tool(submit_result_tool())
            .with_tool(accept_result_tool())
            .with_tool(discover_bounties_tool())
            .with_tool(form_party_tool())
            .with_tool(join_party_tool())
            .with_tool(fund_party_tool())
            .with_tool(complete_party_tool())
            .with_tool(disband_party_tool())
            .with_tool(discover_parties_tool())
            .with_tool(get_party_tool())
            .with_tool(stake_validation_tool())
            .with_tool(challenge_validation_tool())
            .with_tool(resolve_validation_tool())
            .with_tool(reclaim_validation_tool())
            .with_tool(get_validation_tool())
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
            .with_tool(list_notifications_tool())
            .with_tool(clear_notifications_tool())
            .with_tool(schedule_task_tool())
            .with_tool(cancel_task_tool())
            .with_tool(record_lesson_tool())
            .with_tool(consolidate_lessons_tool())
            .with_tool(set_lessons_tool())
            .with_tool(create_skill_tool())
            .with_tool(list_skills_tool())
            .with_tool(delete_skill_tool())
            .with_tool(crate::app::self_docs::read_self_docs_tool())
            .with_tool(web_fetch_tool())
            .with_tool(run_wasm_cli_tool())
            .with_tool(execute_script_tool())
            .with_tool(dwell_tool())
            .with_tool(clear_context_tool())
            .with_tool(compact_context_tool())
            .with_tool(consult_model_tool(captured_key.clone(), base_url.clone()))
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
            .with_pre_tool_hook(std::sync::Arc::new(
                super::confirm_guard::TypedConfirmationGuard,
            ))
            .with_pre_tool_hook(std::sync::Arc::new(super::dedup::DuplicateActionGuard))
            .with_post_tool_hook(std::sync::Arc::new(
                super::dedup::DuplicateActionGuardCleanup,
            ))
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
            // this headroom. The baseline = the router CEILING (`session_ceiling`,
            // High for Gemini) so the build level and the per-turn clamp share one
            // source of truth; the difficulty router downgrades routine turns
            // below it per-turn (Minimal on a greeting).
            .with_thinking(session_ceiling.unwrap_or(ThinkingLevel::High))
            // Lower sampling temperature for better first-try-valid rustlite /
            // edits. Gemini applies temperature AND thinking independently (both
            // ride generation_config), so this composes with the High budget.
            .with_temperature(0.2)
            .with_tool(create_subdomain_tool())
            .with_tool(create_and_publish_app_tool())
            .with_tool(batch_create_subdomains_tool())
            .with_tool(release_subdomain_tool())
            .with_tool(bulk_release_subdomains_tool())
            .with_tool(list_subdomains_tool())
            .with_tool(discover_agents_tool())
            .with_tool(embed_app_tool())
            .with_tool(publish_public_face_tool())
            .with_tool(send_lh_tool())
            .with_tool(batch_send_lh_tool())
            .with_tool(check_balances_tool())
            .with_tool(query_balance_tool())
            .with_tool(evm_chains_tool())
            .with_tool(evm_balance_tool())
            .with_tool(resolve_ens_tool())
            .with_tool(evm_call_tool())
            .with_tool(shared_state_set_tool())
            .with_tool(shared_state_get_tool())
            .with_tool(shared_state_list_tool())
            .with_tool(post_bounty_tool())
            .with_tool(claim_bounty_tool())
            .with_tool(submit_result_tool())
            .with_tool(accept_result_tool())
            .with_tool(discover_bounties_tool())
            .with_tool(form_party_tool())
            .with_tool(join_party_tool())
            .with_tool(fund_party_tool())
            .with_tool(complete_party_tool())
            .with_tool(disband_party_tool())
            .with_tool(discover_parties_tool())
            .with_tool(get_party_tool())
            .with_tool(stake_validation_tool())
            .with_tool(challenge_validation_tool())
            .with_tool(resolve_validation_tool())
            .with_tool(reclaim_validation_tool())
            .with_tool(get_validation_tool())
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
            .with_tool(list_notifications_tool())
            .with_tool(clear_notifications_tool())
            .with_tool(schedule_task_tool())
            .with_tool(cancel_task_tool())
            .with_tool(record_lesson_tool())
            .with_tool(consolidate_lessons_tool())
            .with_tool(set_lessons_tool())
            .with_tool(create_skill_tool())
            .with_tool(list_skills_tool())
            .with_tool(delete_skill_tool())
            .with_tool(crate::app::self_docs::read_self_docs_tool())
            .with_tool(web_fetch_tool())
            .with_tool(run_wasm_cli_tool())
            .with_tool(execute_script_tool())
            .with_tool(dwell_tool())
            .with_tool(clear_context_tool())
            .with_tool(compact_context_tool())
            .with_tool(consult_model_tool(captured_key.clone(), base_url.clone()))
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
    // The live tool surface (builtins + every chat closure tool) — surfaced in
    // the admin card so the count reflects reality, not the builtin-only set.
    let tool_count = agent.tools().names().len();
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        app.agent = Some(Rc::new(agent));
        app.agent_tool_count = Some(tool_count);
        // Stable identity (address in credits mode, key in BYOK) — NOT the
        // rotating credits token, so the session isn't restarted per turn.
        app.session_key = Some(identity.to_string());
        app.turn_count = 0;
        // Record the router ceiling so `run_send` can clamp per-turn thinking to
        // the level THIS session was actually built with (never raises past the
        // user's model choice).
        app.session_thinking_ceiling = session_ceiling;
        // Record the session model so the per-turn MODEL router
        // (`difficulty::route_model`) can pick a cheaper SAME-BACKEND model for
        // routine turns, clamped to this as the ceiling (#7).
        app.session_model = Some(model.clone());
    });
    // If the admin card is already on screen, refresh its tools line live.
    if crate::app::dom::by_id("tools-count").is_some() {
        crate::app::dom::swap_outer("tools-count", &crate::app::templates::tools_count_span(tool_count));
    }
    Ok(())
}

/// Static curated global-lessons digest, served by the web bundle alongside
/// `llms.txt`. Written by the SWEEP (`scripts/colony/lesson-digest.mjs`), which
/// harvests every agent's on-chain lessons, dedups, and caps the set.
const GLOBAL_LESSONS_URL: &str = "https://localharness.xyz/global-lessons.txt";

/// Header of the global-lessons prompt section. Distinct from the per-agent
/// `=== Lessons (self-recorded) ===` header so the model can tell platform-wide
/// lessons apart from its own.
const GLOBAL_LESSONS_HEADER: &str = "=== GLOBAL LESSONS (learned across the platform) ===";

/// Max lines folded from the curated digest, and max chars per line — a second,
/// independent bound on top of the sweep's own cap so a runaway file can never
/// bloat the prompt.
const GLOBAL_LESSONS_MAX_LINES: usize = 40;
const GLOBAL_LESSONS_MAX_LINE_CHARS: usize = 240;

/// Fetch + render the curated `/global-lessons.txt` as a labelled system-prompt
/// section, or `None` on any failure (network/HTTP/empty). Timeout-capped via
/// `net::read` (browser fetch has no default timeout) so a hung request can't
/// stall session start. One lesson per line; blank lines drop, each line is
/// trimmed + length-clamped, and at most [`GLOBAL_LESSONS_MAX_LINES`] survive.
async fn fetch_global_lessons_section() -> Option<String> {
    let body = crate::app::net::read(async {
        let resp = reqwest::Client::new()
            .get(GLOBAL_LESSONS_URL)
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        resp.text().await.ok()
    })
    .await
    .ok()
    .flatten()?;

    let lines: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(GLOBAL_LESSONS_MAX_LINES)
        .map(|l| l.chars().take(GLOBAL_LESSONS_MAX_LINE_CHARS).collect())
        .collect();
    if lines.is_empty() {
        return None;
    }
    Some(format!("{GLOBAL_LESSONS_HEADER}\n{}", lines.join("\n")))
}
