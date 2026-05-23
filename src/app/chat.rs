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
use crate::{Agent, CapabilitiesConfig, GeminiAgentConfig, StreamChunk};

use super::dom;
use super::templates;
use super::APP;

/// Driven by the `send` data-action. Reads the prompt + key, lazily
/// (re)starts the session, then streams a turn through the Agent.
pub(crate) async fn run_send() {
    let Some(key_input) = dom::input_by_id("key") else {
        dom::set_status("internal: #key input missing", true);
        return;
    };
    let Some(prompt_area) = dom::textarea_by_id("prompt") else {
        dom::set_status("internal: #prompt textarea missing", true);
        return;
    };

    let key = key_input.value().trim().to_string();
    let prompt = prompt_area.value().trim().to_string();

    if key.is_empty() {
        dom::set_status("enter an API key first.", true);
        return;
    }
    if prompt.is_empty() {
        dom::set_status("enter a prompt first.", true);
        return;
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
        dom::set_status("starting session…", false);
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
    dom::set_status("thinking…", false);

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

    let t_end = js_sys::Date::now();
    let total_ms = (t_end - t0) as i64;
    let ttft_ms = t_first_chunk.map(|t| (t - t0) as i64).unwrap_or(total_ms);
    dom::set_status(
        &format!(
            "done · ttft {ttft_ms} ms · total {total_ms} ms · {turn_count} turn{}",
            if turn_count == 1 { "" } else { "s" }
        ),
        false,
    );

    // Persist the new history snapshot, then refresh the panel so
    // any tool-created files (and the history marker itself) show up.
    super::history::save_from_agent().await;
    super::opfs::refresh().await;
}

async fn start_session(key: &str) -> Result<(), JsValue> {
    // Unrestricted capabilities turn on the write tools; the Agent
    // constructor refuses to start without a policy gate. OPFS is
    // sandboxed per-origin (no path-escape risk) and this is the
    // user's own tab, so allow_all is the right policy for the demo —
    // anyone running the SDK as a library in less trusted contexts
    // should pick a tighter one (e.g. workspace_only / per-tool allow).
    let mut cfg = GeminiAgentConfig::new(key.to_string())
        .with_capabilities(CapabilitiesConfig::unrestricted())
        .with_policies(vec![policy::allow_all()])
        .with_filesystem(super::shared_opfs());
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
