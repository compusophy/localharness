//! Chat-turn orchestration. Driven by the `send` action in
//! [`super::events`]; entirely HTMX-style — every UI mutation is a
//! `swap_inner` / `append_html` on a targeted `id=`. We never walk the
//! DOM looking for nodes; element identity is established up-front via
//! ids we allocate and templates we render.

use std::collections::VecDeque;
use std::rc::Rc;

use futures_util::StreamExt;
use maud::html;
use wasm_bindgen::JsValue;

use crate::policy;
use crate::tools::ClosureTool;
use crate::{Agent, CapabilitiesConfig, GeminiAgentConfig, StreamChunk};

use super::dom;
use super::templates;
use super::APP;

/// Driven by the `send` data-action. Reads the prompt + key, lazily
/// (re)starts the session, then streams a turn through the Agent.
pub(crate) async fn run_send() {
    let Some(prompt_area) = dom::textarea_by_id("prompt") else {
        dom::set_status("internal: #prompt textarea missing", true);
        return;
    };

    // The api key input lives in the admin dropdown — only present in
    // the DOM when admin is open. Fall back to sessionStorage (sync)
    // and then OPFS (async) so the user can send without keeping
    // admin open just to host the input field.
    let key = match read_api_key().await {
        Some(k) => k,
        None => {
            dom::set_status(
                "no api key — open admin (top right) and paste your gemini key",
                true,
            );
            return;
        }
    };

    let prompt = prompt_area.value().trim().to_string();
    if prompt.is_empty() {
        dom::set_status("enter a prompt first.", true);
        return;
    }

    // Payment gate. If the agent's owner has set a per-turn price AND
    // we know this visitor is *not* the owner, collect payment via
    // the cross-origin iframe signer before the LLM call runs.
    // Owner-of-the-agent always sends free; verification-pending /
    // unregistered / failed states fall through without charging.
    match collect_payment_if_required().await {
        Ok(None) => {} // free or no gate
        Ok(Some(tx_hash)) => {
            dom::set_status(
                &format!("payment received ({}); sending…", short_hash(&tx_hash)),
                false,
            );
        }
        Err(err) => {
            dom::set_status(&format!("payment failed: {err}"), true);
            return;
        }
    }

    // Cache the key in sessionStorage so a refresh doesn't lose it.
    if let Ok(Some(storage)) = dom::session_storage() {
        let _ = storage.set_item("gemini_api_key", &key);
    }

    // Lazily start the session if we have none, or the key changed.
    let session_needs_start = APP.with(|cell| {
        let app = cell.borrow();
        app.agent.is_none() || app.session_key.as_deref() != Some(key.as_str())
    });
    if session_needs_start {
        if let Err(err) = start_session(&key).await {
            dom::set_status(&format!("session start failed: {err:?}"), true);
            return;
        }
    }

    let Some(agent) = APP.with(|cell| cell.borrow().agent.clone()) else {
        dom::set_status("internal: agent not set after start_session", true);
        return;
    };

    // Allocate ids for the user turn, assistant turn, and first text
    // segment up front. Element identity is fixed before we touch the DOM.
    let (user_turn_id, assistant_turn_id, mut seg_id) = APP.with(|cell| {
        let mut app = cell.borrow_mut();
        (app.alloc_id(), app.alloc_id(), app.alloc_id())
    });

    dom::append_html(
        "transcript",
        &templates::turn(user_turn_id, "user", html! { (prompt) }, false).into_string(),
    );
    dom::append_html(
        "transcript",
        &templates::turn(
            assistant_turn_id,
            "assistant",
            templates::text_segment(seg_id, ""),
            true,
        )
        .into_string(),
    );

    let assistant_body_id = format!("turn-body-{assistant_turn_id}");

    // Clear the prompt, keep focus.
    prompt_area.set_value("");
    let _ = prompt_area.focus();
    // No "thinking…" — the assistant turn renders with the .streaming
    // class while in flight, which adds its own "· streaming" suffix
    // to the role line. That's enough feedback.

    // FIFO of pending tool-block ids. The Gemini backend emits
    // ToolCall/ToolResult pairs sequentially (one result per call,
    // in order), so popping the front always matches.
    let mut pending_tools: VecDeque<u32> = VecDeque::new();
    // (seg_id, accumulated_raw_text) for every text segment we render
    // this turn — used for markdown rendering at end-of-stream.
    let mut text_segments: Vec<(u32, String)> = vec![(seg_id, String::new())];

    // Timing: ms since epoch is precise enough for ttft/total pills.
    let t0 = js_sys::Date::now();
    let mut t_first_chunk: Option<f64> = None;

    let response = match agent.chat(prompt).await {
        Ok(r) => r,
        Err(err) => {
            dom::set_status(&format!("agent.chat: {err}"), true);
            mark_turn_done(assistant_turn_id);
            return;
        }
    };
    let mut cursor = response.chunks();

    while let Some(item) = cursor.next().await {
        if t_first_chunk.is_none() {
            t_first_chunk = Some(js_sys::Date::now());
        }
        match item {
            Ok(StreamChunk::Text { text, .. }) => {
                if !text.is_empty() {
                    let (cur_id, cur_text) = text_segments
                        .last_mut()
                        .expect("text_segments seeded at start of turn");
                    cur_text.push_str(&text);
                    let inner = html! { (cur_text) }.into_string();
                    dom::swap_inner(&format!("seg-{cur_id}"), &inner);
                }
            }
            Ok(StreamChunk::ToolCall(call)) => {
                let tool_seg_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                dom::append_html(
                    &assistant_body_id,
                    &templates::tool_call_block(tool_seg_id, &call).into_string(),
                );
                pending_tools.push_back(tool_seg_id);

                // Open a fresh text segment for whatever the model
                // says after the tool call (it usually says nothing
                // until the result comes back, but if it does, this
                // is where it lands).
                seg_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                text_segments.push((seg_id, String::new()));
                dom::append_html(
                    &assistant_body_id,
                    &templates::text_segment(seg_id, "").into_string(),
                );
            }
            Ok(StreamChunk::ToolResult(result)) => {
                if let Some(tool_seg_id) = pending_tools.pop_front() {
                    let result_target = format!("tool-{tool_seg_id}-result");
                    dom::swap_inner(
                        &result_target,
                        &templates::tool_call_result(&result).into_string(),
                    );
                    update_tool_status(tool_seg_id, result.error.is_none());
                }
            }
            Ok(StreamChunk::Thought { .. }) => {
                // Thoughts intentionally not surfaced (yet).
            }
            Err(err) => {
                dom::set_status(&format!("chunk: {err}"), true);
                mark_turn_done(assistant_turn_id);
                return;
            }
        }
    }

    // Stream done — re-render each text segment as markdown so the
    // user sees formatted output instead of raw md syntax.
    for (id, raw) in &text_segments {
        if raw.is_empty() {
            continue;
        }
        let html_str = templates::rendered_markdown(raw).into_string();
        dom::swap_inner(&format!("seg-{id}"), &html_str);
    }

    mark_turn_done(assistant_turn_id);
    APP.with(|cell| cell.borrow_mut().turn_count += 1);
    let turn_count = APP.with(|cell| cell.borrow().turn_count);

    // No status write on success — keep the terminal silent. (Per
    // the minimalism pass; ttft/total metrics still computed for
    // anyone who wants to grep the wasm but not surfaced in chrome.)
    let _t_end = js_sys::Date::now();
    let _ = (t0, t_first_chunk, turn_count);

    // Persist the new history snapshot, then refresh the panel so
    // any tool-created files (and the history marker itself) show up.
    super::history::save_from_agent().await;
    super::opfs::refresh().await;
}

async fn start_session(key: &str) -> Result<(), JsValue> {
    // System instruction — the agent needs to know what it's running
    // inside and what its filesystem looks like. Without this, prompts
    // like "what is pricing" produce blind tool calls because the
    // model has no priors about the localharness environment.
    let host = super::tenant::current();
    let agent_name = match &host {
        super::tenant::Host::Tenant(name) => name.clone(),
        _ => "this agent".to_string(),
    };
    let system_instructions = format!(
        "You are {agent_name}, a browser-resident assistant running inside \
         the localharness platform — a Rust SDK that compiles to wasm and runs \
         in the user's browser tab. You are speaking to your owner, who minted \
         this subdomain as an ERC-721 NFT on Tempo Moderato.\n\n\
         \
         === Your tools (you DO have all of these) ===\n\
         Filesystem (per-origin OPFS sandbox):\n\
           • list_directory(path) — list files in a directory.\n\
           • view_file(path, range?) — read a file's contents.\n\
           • find_file(pattern) — glob search by name.\n\
           • search_directory(pattern, path?) — regex search of file contents.\n\
           • create_file(path, content) — write a new file.\n\
           • edit_file(path, old, new) — exact-string replace in a file.\n\
           • delete_file(path) — DELETE a file. You CAN do this; do not say \
             otherwise. Irreversible — confirm intent first unless the user \
             explicitly told you to delete.\n\
           • rename_file(from, to) — move or rename.\n\n\
         \
         Platform:\n\
           • create_subdomain(name) — register a new <name>.localharness.xyz \
             on-chain, owned by your owner's master wallet. Returns the tx \
             hash. Each subdomain is its own agent tab.\n\
           • start_subagent(system_instructions, prompt) — spawn a one-shot \
             text-only subagent with no tool access. Use for self-contained \
             reasoning / writing tasks you want isolated from your context.\n\
           • spawn_recursive_subagent(system_instructions, prompt) — spawn a \
             full subagent with the same tool surface YOU have (filesystem, \
             create_subdomain, start_subagent, etc.). Use for delegation that \
             needs tools. Recursion depth is implicit (each subagent has its \
             own context; cost grows with depth — don't chain more than 3 \
             levels unless the user asked).\n\
           • generate_image(prompt) — produce an image from a text prompt.\n\n\
         \
         === Conventions ===\n\
         • Files at the OPFS root are the user's. Dotfiles starting with `.lh_*` \
           are internal state (api key, conversation history, owner marker, \
           feedback log) — read only if the user asks, NEVER write or delete.\n\
         • Keep responses concise and conversational. The user is on the same \
           page; they don't need you restating what you just did.\n\
         • Don't speculate about filesystem contents — call list_directory first \
           when you actually need to know.\n\
         • Don't blindly call tools when the user is just chatting. \"hi\" / \
           \"what can you do?\" don't need a tool call.\n\
         • When you do call a tool, the call AND its result are visible to the \
           user in the transcript — no need to re-narrate either."
    );

    // Unrestricted capabilities turn on the write tools; the Agent
    // constructor refuses to start without a policy gate. OPFS is
    // sandboxed per-origin (no path-escape risk) and this is the
    // user's own tab, so allow_all is the right policy for the demo —
    // anyone running the SDK as a library in less trusted contexts
    // should pick a tighter one (e.g. workspace_only / per-tool allow).
    let captured_key = key.to_string();
    let mut cfg = GeminiAgentConfig::new(key.to_string())
        .with_capabilities(CapabilitiesConfig::unrestricted())
        .with_policies(vec![policy::allow_all()])
        .with_filesystem(super::shared_opfs())
        .with_system_instructions(system_instructions)
        .with_tool(create_subdomain_tool())
        .with_tool(spawn_recursive_subagent_tool(captured_key));
    // If a previous session left history on OPFS, restore it into the
    // new connection. Consumed once — subsequent key changes start
    // fresh from the in-memory agent's history.
    if let Some(bytes) = super::history::take_pending() {
        cfg = cfg.with_history_bytes(bytes);
    }
    let agent = Agent::start_gemini(cfg)
        .await
        .map_err(|e| JsValue::from_str(&format!("start_gemini: {e}")))?;
    APP.with(|cell| {
        let mut app = cell.borrow_mut();
        app.agent = Some(Rc::new(agent));
        app.session_key = Some(key.to_string());
        app.turn_count = 0;
    });
    Ok(())
}

fn mark_turn_done(turn_id: u32) {
    let id = format!("turn-{turn_id}");
    if let Some(el) = dom::by_id(&id) {
        let cls = el.class_name();
        let new_cls: Vec<&str> =
            cls.split_whitespace().filter(|c| *c != "streaming").collect();
        el.set_class_name(&new_cls.join(" "));
    }
}

/// Replace the running pill inside a tool block with an ok / err
/// pill. The block template stamps the running pill with
/// `id="tool-{seg_id}-status"`; we swap-outer it so the new span
/// keeps the same id for any future result swap.
fn update_tool_status(tool_seg_id: u32, ok: bool) {
    let target = format!("tool-{tool_seg_id}-status");
    let pill_class = if ok { "tc-status ok" } else { "tc-status err" };
    let new_html = html! {
        span id=(target) class=(pill_class) {}
    }
    .into_string();
    dom::swap_outer(&target, &new_html);
}

/// Returns `Ok(Some(tx_hash))` if a payment was collected, `Ok(None)`
/// if no payment was required (free agent, owner sending, unverified
/// origin), or `Err(_)` if the visitor refused or the on-chain leg
/// failed. Caller short-circuits the send on `Err`.
async fn collect_payment_if_required() -> Result<Option<String>, String> {
    use super::VerifyState;

    let (price_wei, verify_state, tba) = APP.with(|cell| {
        let app = cell.borrow();
        (
            app.pricing_wei.unwrap_or(0),
            app.verify_state.clone(),
            app.tba_address.clone(),
        )
    });
    if price_wei == 0 {
        return Ok(None);
    }
    let Some(tba) = tba else {
        // Priced but no TBA known — can't route the funds. Fail closed
        // rather than silently letting the visitor through for free.
        return Err("agent is priced but its TBA isn't known yet (verification still running?)".into());
    };
    let visitor_address = match verify_state {
        VerifyState::Verified { .. } => return Ok(None), // owner sends free
        VerifyState::Visitor { visitor_address, .. } => visitor_address,
        VerifyState::Pending | VerifyState::Unregistered | VerifyState::Failed { .. } => {
            // Without a recovered visitor address we can't build a tx
            // from-them. Fail closed.
            return Err(
                "agent is priced but owner verification didn't complete — refresh and retry"
                    .into(),
            );
        }
    };

    let purpose = format!(
        "pay {} $localharness per turn to this agent",
        super::format_wei_as_test_eth(price_wei),
    );

    dom::set_status("payment: reading nonce + gas…", false);
    let nonce = crate::registry::next_nonce(&visitor_address)
        .await
        .map_err(|e| format!("nonce: {e}"))?;
    let gas_price = crate::registry::current_gas_price()
        .await
        .map_err(|e| format!("gas price: {e}"))?;

    // Build ERC-20 transfer(tba, price_wei) calldata. We do this
    // here in the subdomain bundle (it's all pure Rust + the
    // registry helpers) so the iframe signer just signs whatever
    // calldata we hand it.
    let tba_bytes = parse_address(&tba)?;
    let mut tba_padded = [0u8; 32];
    tba_padded[12..].copy_from_slice(&tba_bytes);
    let amount_bytes = u256_be(price_wei);
    let selector = transfer_selector();
    let mut calldata = Vec::with_capacity(4 + 32 + 32);
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&tba_padded);
    calldata.extend_from_slice(&amount_bytes);
    let data_hex = bytes_to_hex(&calldata);

    // Estimate gas for the ERC-20 transfer. Tempo gas accounting
    // can be twitchy; a buffer here saves a "out of gas" surprise.
    dom::set_status("payment: estimating gas…", false);
    let gas_limit = match estimate_call_gas(
        &visitor_address,
        crate::registry::LOCALHARNESS_TOKEN_ADDRESS,
        &data_hex,
    ).await {
        Ok(g) => g,
        Err(_) => 120_000, // safe fallback for an ERC-20 transfer
    };

    dom::set_status("payment: signing via apex…", false);
    let raw_tx = super::verify::sign_tx_via_iframe(super::verify::SignTxRequest {
        to_hex: crate::registry::LOCALHARNESS_TOKEN_ADDRESS,
        value_wei: 0,
        nonce,
        gas_limit,
        gas_price,
        chain_id: crate::registry::CHAIN_ID,
        purpose: &purpose,
        data_hex: &data_hex,
    })
    .await?;

    dom::set_status("payment: submitting + waiting for receipt…", false);
    let tx_hash = crate::registry::submit_and_wait_receipt(&raw_tx)
        .await
        .map_err(|e| format!("submit: {e}"))?;

    Ok(Some(tx_hash))
}

/// Read the api key with graceful fallback. Tries the live `#key`
/// input first (if admin is open), then sessionStorage, then OPFS.
/// Returns `None` only if every layer is empty.
async fn read_api_key() -> Option<String> {
    if let Some(input) = dom::input_by_id("key") {
        let v = input.value().trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    if let Ok(Some(storage)) = dom::session_storage() {
        if let Ok(Some(cached)) = storage.get_item("gemini_api_key") {
            let trimmed = cached.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    if let Some(persisted) = super::key_store::load().await {
        let trimmed = persisted.trim().to_string();
        if !trimmed.is_empty() {
            return Some(trimmed);
        }
    }
    None
}

fn parse_address(hex: &str) -> Result<[u8; 20], String> {
    let trimmed = hex.trim().trim_start_matches("0x").trim_start_matches("0X");
    if trimmed.len() != 40 {
        return Err(format!("address must be 20 bytes hex, got {}", trimmed.len()));
    }
    let mut out = [0u8; 20];
    let bytes = trimmed.as_bytes();
    for i in 0..20 {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("non-hex byte {b}")),
    }
}

fn u256_be(value: u128) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[16..].copy_from_slice(&value.to_be_bytes());
    out
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn transfer_selector() -> [u8; 4] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(b"transfer(address,uint256)");
    let mut out = [0u8; 4];
    out.copy_from_slice(&hasher.finalize()[..4]);
    out
}

async fn estimate_call_gas(
    from_hex: &str,
    to_hex: &str,
    data_hex: &str,
) -> Result<u128, String> {
    // Direct RPC because we don't have eth_estimateGas exposed from
    // registry. Mirror its 25% buffer pattern.
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_estimateGas",
        "params": [{
            "from": from_hex,
            "to": to_hex,
            "data": format!("0x{data_hex}"),
        }],
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(crate::registry::RPC_URL)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("estimateGas: {e}"))?;
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("estimateGas parse: {e}"))?;
    let hex = json
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("estimateGas: {json}"))?;
    let raw = u128::from_str_radix(hex.trim_start_matches("0x"), 16)
        .map_err(|e| format!("estimateGas hex: {e}"))?;
    Ok(raw + raw / 4)
}

fn short_hash(hash: &str) -> String {
    let stripped = hash.trim_start_matches("0x");
    if stripped.len() < 12 {
        return hash.to_string();
    }
    format!("0x{}…{}", &stripped[..6], &stripped[stripped.len() - 4..])
}

// =============================================================================
// Platform-level closure tools (browser-specific; not in the SDK builtins).
// =============================================================================

/// `create_subdomain(name)` — register `<name>.localharness.xyz` on the
/// LocalharnessRegistry diamond, signed by the owner's apex wallet via
/// the iframe signer. Returns the tx hash. Sanitises the input the same
/// way `tenant::sanitize` does for the apex claim form.
fn create_subdomain_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to register, e.g. \"alice\" \
                    becomes alice.localharness.xyz. 3-32 chars; lowercase \
                    letters, digits, and hyphens only."
            }
        },
        "required": ["name"]
    });
    ClosureTool::new(
        "create_subdomain",
        "Register a new <name>.localharness.xyz subdomain on-chain. The owner's master \
         wallet pays gas and ends up holding the resulting ERC-721 NFT. Returns the tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let cleaned = super::tenant::sanitize(name);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return Err(crate::error::Error::other(
                    "name must be 3-32 chars, a-z 0-9 -",
                ));
            }
            match super::verify::claim_name_via_iframe(&cleaned).await {
                Ok((owner, tx_hash)) => Ok(serde_json::json!({
                    "name": cleaned,
                    "url": format!("https://{cleaned}.localharness.xyz/"),
                    "owner": owner,
                    "tx_hash": tx_hash,
                })),
                Err(e) => Err(crate::error::Error::other(format!("claim failed: {e}"))),
            }
        },
    )
}

/// `spawn_recursive_subagent(system_instructions, prompt)` — full subagent
/// with the same tool surface as the parent (filesystem, create_subdomain,
/// itself). Runs the supplied prompt as a single conversation, drives it
/// to completion via streaming chunks, returns the assistant's final text.
///
/// Implementation: builds a fresh `Agent::start_gemini` with the SAME
/// api key + filesystem + closure tools. The subagent has its own
/// conversation context (no shared history with the parent), so recursion
/// is bounded by the user's wallet (Gemini cost grows with depth, that's
/// the natural limiter).
fn spawn_recursive_subagent_tool(api_key: String) -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "system_instructions": {
                "type": "string",
                "description": "System prompt for the subagent — describes its persona, \
                    scope, and any constraints. Often \"you are a focused worker \
                    that does X and returns just the result\"."
            },
            "prompt": {
                "type": "string",
                "description": "The user message to send to the subagent."
            }
        },
        "required": ["system_instructions", "prompt"]
    });
    ClosureTool::new(
        "spawn_recursive_subagent",
        "Spawn a subagent with the SAME tool surface as you (filesystem, \
         create_subdomain, start_subagent, spawn_recursive_subagent itself). \
         The subagent has its own conversation context — it cannot see your \
         history. Drives the subagent through one full conversation turn (which \
         may itself involve internal tool calls) and returns the subagent's final \
         text response.",
        schema,
        move |args: serde_json::Value, _ctx| {
            let api_key = api_key.clone();
            async move {
                let system = args
                    .get("system_instructions")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let prompt = args.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
                if prompt.is_empty() {
                    return Err(crate::error::Error::other(
                        "spawn_recursive_subagent: prompt cannot be empty",
                    ));
                }
                let cfg = GeminiAgentConfig::new(api_key.clone())
                    .with_capabilities(CapabilitiesConfig::unrestricted())
                    .with_policies(vec![policy::allow_all()])
                    .with_filesystem(super::shared_opfs())
                    .with_system_instructions(system.to_string())
                    .with_tool(create_subdomain_tool())
                    .with_tool(spawn_recursive_subagent_tool(api_key.clone()));
                let sub = Agent::start_gemini(cfg)
                    .await
                    .map_err(|e| crate::error::Error::other(format!("start_gemini: {e}")))?;
                let response = sub
                    .chat(prompt.to_string())
                    .await
                    .map_err(|e| crate::error::Error::other(format!("subagent chat: {e}")))?;
                let mut cursor = response.chunks();
                let mut text = String::new();
                while let Some(item) = cursor.next().await {
                    match item {
                        Ok(StreamChunk::Text { text: t, .. }) => text.push_str(&t),
                        Ok(_) => {} // ToolCall / ToolResult / Thought ignored — only the final text matters.
                        Err(e) => {
                            return Err(crate::error::Error::other(format!(
                                "subagent chunk: {e}"
                            )))
                        }
                    }
                }
                Ok(serde_json::json!({ "final_response": text }))
            }
        },
    )
}
