//! Chat-turn orchestration. Driven by the `send` action in
//! [`super::events`]; entirely HTMX-style — every UI mutation is a
//! `swap_inner` / `append_html` on a targeted `id=`. We never walk the
//! DOM looking for nodes; element identity is established up-front via
//! ids we allocate and templates we render.

use std::collections::VecDeque;

use futures_util::StreamExt;
use maud::html;
use wasm_bindgen::JsValue;

use crate::turn_flow::{
    classify_empty, classify_turn, empty_message, EmptyKind, TurnOutcome,
    MAX_AUTO_CONTINUATIONS,
};
use crate::{Agent, StreamChunk};

use super::dom;
use super::templates;
use super::APP;

mod access;
mod dedup;
mod prompt;
mod session;
mod tools;

// The chat:: surface the rest of the app calls (events.rs, agent_rpc.rs,
// app::mod, teams_sync.rs) — re-exported so the split keeps every external
// call site at the same `crate::app::chat::` paths.
pub(crate) use access::{
    credit_address_existing, credit_signer, ensure_credit_meter, escrow_bridge_wei,
};
// Not currently called from outside `chat`, but part of the documented chat::
// surface — keep it reachable at the same `crate::app::chat::` path.
#[allow(unused_imports)]
pub(crate) use access::model_access_is_credits;
pub(crate) use session::start_session;

use access::{collect_payment_if_required, resolve_credit_access, short_hash};

thread_local! {
    /// True while a turn is streaming — guards against starting a second
    /// turn (e.g. pressing Enter again mid-turn).
    static TURN_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set by the stop button; the stream loop checks it each chunk and
    /// breaks, cooperatively ending the turn.
    static TURN_CANCEL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set by the `clear_context` tool; drained AFTER the turn ends in
    /// `run_send` (never inline — wiping history mid-turn corrupts the
    /// in-flight turn the tool runs inside).
    static PENDING_CLEAR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set by the `compact_context` tool; drained post-turn like
    /// `PENDING_CLEAR`.
    static PENDING_COMPACT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The user prompt that started the CURRENT run — stashed by [`run_send`]
    /// so the [⇪ background] promote handler can re-issue it as an on-chain
    /// goal-job task. Read via [`active_run_prompt`]; only meaningful while
    /// `TURN_ACTIVE` (overwritten by the next run, never a durable record).
    static RUN_PROMPT: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// Set by [`request_stop_for_promote`]: this cancellation is a promote-
    /// to-background, so the "Stopped. What should I do instead?" redirect
    /// note is skipped (the promote confirmation lands in the status line).
    /// Cleared by `TurnGuard` on every run exit.
    static PROMOTE_REQUESTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Schedule a full context clear for when the in-flight turn ends — set by
/// the `clear_context` tool (`tools::misc`), drained in [`run_send`].
pub(crate) fn set_pending_clear() {
    PENDING_CLEAR.with(|c| c.set(true));
}

/// Schedule a context compaction for when the in-flight turn ends — set by
/// the `compact_context` tool (`tools::misc`), drained in [`run_send`].
pub(crate) fn set_pending_compact() {
    PENDING_COMPACT.with(|c| c.set(true));
}

/// Request cooperative cancellation of the running turn (the stop
/// button). Two layers: `TURN_CANCEL` breaks the UI's chunk loop, and
/// `agent.cancel_turn()` stops the *producer* — the detached task driving
/// the agent loop — so it stops calling the model and running tools
/// instead of finishing the turn in the background while the UI moves on.
pub(crate) fn request_stop_turn() {
    TURN_CANCEL.with(|c| c.set(true));
    if let Some(agent) = APP.with(|cell| cell.borrow().agent.clone()) {
        agent.cancel_turn();
    }
}

/// The prompt that started the in-flight run, if one is active. The
/// promote-to-background handler reads this to build the goal-job task;
/// `None` once the run ends (lifecycle keeps the button absent then, this
/// is the belt-and-braces guard).
pub(crate) fn active_run_prompt() -> Option<String> {
    if !TURN_ACTIVE.with(|c| c.get()) {
        return None;
    }
    let p = RUN_PROMPT.with(|c| c.borrow().clone());
    (!p.is_empty()).then_some(p)
}

/// Stop the running turn FOR A PROMOTE-TO-BACKGROUND: same cooperative
/// cancel as the stop button, but flags the run so the cancel branch skips
/// the "Stopped. What should I do instead?" redirect note. Returns false
/// (no-op) when no run is active or a promote was already requested —
/// a double-press can't schedule two jobs.
pub(crate) fn request_stop_for_promote() -> bool {
    if !TURN_ACTIVE.with(|c| c.get()) {
        return false;
    }
    if PROMOTE_REQUESTED.with(|c| c.replace(true)) {
        return false;
    }
    request_stop_turn();
    true
}

/// RAII cleanup for a turn: clears the active/cancel flags and restores
/// the send button (if the stop button is currently shown) on every
/// exit path, including early returns and future cancellation.
struct TurnGuard;
impl Drop for TurnGuard {
    fn drop(&mut self) {
        TURN_ACTIVE.with(|c| c.set(false));
        TURN_CANCEL.with(|c| c.set(false));
        PROMOTE_REQUESTED.with(|c| c.set(false));
        if dom::by_id("terminal-stop").is_some() {
            dom::swap_outer("terminal-stop", &templates::send_button().into_string());
        }
    }
}

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
    // Resolve how this turn reaches the model: platform credits (proxy
    // auth token + proxy base URL) or BYOK (the stored Gemini key).
    let access = match resolve_credit_access().await {
        Some(a) => a,
        None => {
            super::show_api_key_modal();
            return;
        }
    };
    // Credits mode (base_url set): top up the per-request meter so the proxy
    // bills real `$LH` per call (NOT a free session). Silent best-effort.
    if access.base_url.is_some() {
        ensure_credit_meter().await;
    }
    let key = access.cfg_auth;

    let prompt = prompt_area.value().trim().to_string();
    if prompt.is_empty() {
        // Silent no-op — no explanatory validation text. (on-chain feedback)
        return;
    }

    // One turn at a time. A second send (e.g. Enter pressed again mid-
    // turn) is ignored rather than racing a concurrent stream.
    if TURN_ACTIVE.with(|c| c.get()) {
        return;
    }
    TURN_ACTIVE.with(|c| c.set(true));
    TURN_CANCEL.with(|c| c.set(false));
    // Stash the prompt for the run's lifetime so [⇪ background] can promote
    // this exact request to an on-chain goal job (see `active_run_prompt`).
    RUN_PROMPT.with(|c| *c.borrow_mut() = prompt.clone());
    // From here, every return path resets the flags + restores the
    // send button via Drop.
    let _turn_guard = TurnGuard;

    // A fresh send wipes any stale status-line error (feedback #64: errors
    // never cleared without a reload). Real errors land in the transcript;
    // the status line is a transient mirror only.
    dom::set_status("", false);

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
            transcript_system_error(&format!("payment failed: {err}"));
            dom::set_status("payment failed — see the message above", true);
            return;
        }
    }

    // Cache the BYOK key so a refresh doesn't lose it. Credits tokens
    // rotate per resolve and carry nothing worth caching.
    if access.base_url.is_none() {
        if let Ok(Some(storage)) = dom::session_storage() {
            let _ = storage.set_item("gemini_api_key", &key);
        }
    }

    // Lazily start the session if we have none, or the identity changed.
    let session_needs_start = APP.with(|cell| {
        let app = cell.borrow();
        app.agent.is_none() || app.session_key.as_deref() != Some(access.identity.as_str())
    });
    if session_needs_start {
        if let Err(err) = start_session(&key, access.base_url.clone(), &access.identity).await {
            transcript_system_error(&format!("session start failed: {err:?}"));
            dom::set_status("session start failed — see the message above", true);
            return;
        }
    }

    let Some(agent) = APP.with(|cell| cell.borrow().agent.clone()) else {
        dom::set_status("internal: agent not set after start_session", true);
        return;
    };

    // Clear the prompt, keep focus — the value is already captured above.
    prompt_area.set_value("");
    let _ = prompt_area.focus();

    // Swap the send arrow for the stop slot for the whole (possibly
    // multi-turn) run; the guard / loop-end restores it. The [⇪ background]
    // promote rides along only on a tenant — scheduling a goal job needs an
    // on-chain name to target, which apex / Host::Other don't have.
    let can_promote = matches!(
        super::tenant::current(),
        super::tenant::Host::Tenant(_)
    );
    dom::swap_outer(
        "terminal-send",
        &templates::stop_button(can_promote).into_string(),
    );

    // === Continuous execution ===
    // The first turn carries the user's prompt and renders a user bubble.
    // After it ends, if the model made tool actions but did NOT signal
    // completion (no `finish`, no terminal question) we auto-continue with
    // a brief internal nudge — no user bubble, no Enter press — so the
    // agent drives a multi-step goal to the end instead of stopping after
    // the first step. Bounded by `MAX_AUTO_CONTINUATIONS`, and every
    // iteration cooperatively honours the stop button (TURN_CANCEL).
    let mut next_input = TurnInput::User(prompt);
    let mut auto_continuations: u32 = 0;
    // Fresh request — clear the duplicate-action ledger.
    dedup::reset_run();
    loop {
        if TURN_CANCEL.with(|c| c.get()) {
            break;
        }
        let outcome = stream_turn(&agent, next_input).await;

        // Persist + refresh after every turn so tool-created files and the
        // history marker show up incrementally (not just at the very end).
        super::history::save_from_agent().await;
        super::opfs::refresh().await;
        update_context_bar(&agent);

        // Drain any context-management a tool requested THIS turn. Deferred
        // to here (not run inside the tool) because clearing/summarising the
        // history mid-turn corrupts the in-flight turn the tool ran inside —
        // the backend re-locks history after the tool to append its result.
        if PENDING_CLEAR.with(|c| c.replace(false)) {
            // Clear supersedes compact — the context is being wiped, so any
            // compact requested the same turn is moot. Drain its flag too, or
            // it would leak and spuriously compact the fresh/empty history at
            // the start of the NEXT user message.
            PENDING_COMPACT.with(|c| c.set(false));
            agent.clear_history(); // wipe the model's working context
            super::history::clear_persisted().await; // wipe the durable OPFS copy
            dom::swap_inner("transcript", ""); // instant visible wipe, no refresh
            break; // the context is gone — nothing left to continue toward
        }
        if PENDING_COMPACT.with(|c| c.replace(false)) && agent.compact().await {
            // Compaction rewrote the backend history (older turns → one
            // summary). Mirror that on screen: wipe and repaint from the
            // now-compacted transcript, then persist so a reload matches.
            let entries = agent.transcript();
            dom::swap_inner("transcript", "");
            super::history::paint_entries(&entries);
            super::history::save_from_agent().await;
        }

        match outcome {
            // Hard stop conditions — never auto-continue.
            TurnOutcome::Finished
            | TurnOutcome::FinalAnswer
            | TurnOutcome::Empty
            | TurnOutcome::Error
            | TurnOutcome::Cancelled => break,
            // Either the turn ended right after tool activity without an
            // explicit completion signal (Incomplete — keep going toward the
            // goal), or it was TRUNCATED mid-answer (EmptyTruncated — the model
            // ran out of output budget while reasoning and produced no text;
            // retry so the answer actually lands). Both auto-continue under the
            // SAME safety cap so neither can loop forever, and both honour the
            // stop button (checked at the top of the loop).
            TurnOutcome::Incomplete | TurnOutcome::EmptyTruncated => {
                if auto_continuations >= MAX_AUTO_CONTINUATIONS {
                    // Safety cap reached — stop and hand control back rather
                    // than looping forever. Surface it so it's never silent.
                    let note_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                    dom::append_html(
                        "transcript",
                        &templates::turn(
                            note_id,
                            "assistant",
                            templates::text_segment(
                                note_id,
                                "(paused — reached the auto-continue limit for this \
                                 message. Send another message to keep going.)",
                            ),
                            false,
                        )
                        .into_string(),
                    );
                    dom::scroll_to_bottom("transcript");
                    break;
                }
                auto_continuations += 1;
                // A truncated turn gets a "finish concisely" nudge; an
                // incomplete (tool-active) turn gets the standard goal nudge.
                next_input = if matches!(outcome, TurnOutcome::EmptyTruncated) {
                    TurnInput::ResumeTruncated
                } else {
                    TurnInput::AutoContinue
                };
            }
        }
    }

    // Restore the send button if the stop button is still showing.
    if dom::by_id("terminal-stop").is_some() {
        dom::swap_outer("terminal-stop", &templates::send_button().into_string());
    }
}

/// Output-token cap per model call for the in-tab agent. Gemini 3.x does
/// DYNAMIC thinking by default; with no `maxOutputTokens` set, a hard task's
/// reasoning can exhaust the model's default window before any final-answer
/// text is emitted — the turn ends `MAX_TOKENS` with empty text, surfacing as
/// "(empty response)" on mobile. A generous cap (well within gemini-3.5-flash's
/// 65536-token output limit) leaves room for the model to BOTH reason AND
/// answer in one call. Paired with a bounded thinking level so reasoning can't
/// monopolise it. (Phones aren't the cause — the same too-small default budget
/// hits everywhere; mobile just surfaces hard tasks more often.)
///
/// DEEP-THINK NOTE: the in-tab session now runs `ThinkingLevel::High` (a 16384
/// thinking budget — see `gemini::loop::thinking_level_to_config`). Gemini's
/// `thinkingBudget` is a CEILING ON REASONING TOKENS THAT IS DRAWN FROM this
/// `maxOutputTokens` window, so 16384 (think) + this 32768 (total) leaves a
/// guaranteed ~16k for the actual answer/tool-calls — thinking can deepen WITHOUT
/// starving the final text (the empty-response fix is preserved: budget ≥ 2×
/// thinking). Don't lower this below `2 × High thinking budget` without also
/// dropping the thinking level, or hard coding turns can regress to empty.
const GEMINI_MAX_OUTPUT_TOKENS: u32 = 32_768;

/// Output-token cap per call for the in-tab Anthropic path. The backend default
/// is 8192 (`anthropic::wire::DEFAULT_MAX_TOKENS`) — tight for a long reasoning
/// turn. A higher cap (well within Claude Haiku's output limit) gives a hard
/// task room to answer in one call. Mirrors the Gemini budget bump.
const ANTHROPIC_MAX_OUTPUT_TOKENS: u32 = 16_384;

/// Internal nudge fed to the model after a TRUNCATED (max-tokens) empty turn:
/// the model was mid-reasoning and ran out of output budget without emitting a
/// final answer. Ask it to resume CONCISELY so the continuation fits. Distinct
/// from [`AUTO_CONTINUE_NUDGE`] (which assumes prior tool activity to build on).
/// `pub(crate)` so [`is_internal_nudge`] callers see one surface for both.
pub(crate) const TRUNCATED_RETRY_NUDGE: &str = "Your previous response was cut off before \
you finished (it hit the output limit). Continue and finish your answer now, \
concisely. If the task is large, break it into smaller steps and take just the \
next one.";

/// Internal nudge fed to the model on an auto-continuation. Kept terse so
/// it doesn't derail the goal; instructs the model to either keep working
/// or call `finish` / ask a question when it's actually done or blocked.
pub(crate) const AUTO_CONTINUE_NUDGE: &str = "Continue toward the user's goal. First review \
what you already did above — NEVER repeat an action that already succeeded (no duplicate \
notifications, transfers, posts, or feedback). If the task is fully complete, call the \
`finish` tool. If you're blocked or need a decision, ask the user a question. Otherwise \
take the next step now without waiting.";

/// True when `text` is one of the INTERNAL nudges ([`AUTO_CONTINUE_NUDGE`] /
/// [`TRUNCATED_RETRY_NUDGE`]) injected between turns. Nudges never paint a
/// user bubble live, so transcript replay (`history::paint_entries`) must
/// skip them too — register any future nudge constant HERE so replay can't
/// leak it as a ghost user turn.
pub(crate) fn is_internal_nudge(text: &str) -> bool {
    text == AUTO_CONTINUE_NUDGE || text == TRUNCATED_RETRY_NUDGE
}

/// What a single streamed turn carries in.
enum TurnInput {
    /// A real user message — renders a user bubble.
    User(String),
    /// An internal auto-continuation nudge after tool activity — no user bubble.
    AutoContinue,
    /// An internal nudge after a TRUNCATED (max-tokens) empty turn — asks the
    /// model to finish its answer concisely. No user bubble.
    ResumeTruncated,
}

/// Stream ONE agent turn into the transcript and report how it ended.
/// Renders a user bubble only for [`TurnInput::User`]; auto-continuations
/// render just the assistant bubble so the internal nudge never shows.
async fn stream_turn(agent: &Agent, input: TurnInput) -> TurnOutcome {
    let (prompt, render_user) = match input {
        TurnInput::User(p) => (p, true),
        TurnInput::AutoContinue => (AUTO_CONTINUE_NUDGE.to_string(), false),
        TurnInput::ResumeTruncated => (TRUNCATED_RETRY_NUDGE.to_string(), false),
    };

    // Allocate ids for the (optional) user turn, assistant turn, and first
    // text segment up front. Element identity is fixed before we touch the DOM.
    let (user_turn_id, assistant_turn_id, mut seg_id) = APP.with(|cell| {
        let mut app = cell.borrow_mut();
        (app.alloc_id(), app.alloc_id(), app.alloc_id())
    });

    if render_user {
        dom::append_html(
            "transcript",
            &templates::turn(user_turn_id, "user", html! { (prompt) }, false).into_string(),
        );
    }
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
    dom::scroll_to_bottom("transcript");

    let assistant_body_id = format!("turn-body-{assistant_turn_id}");

    // FIFO of pending tool blocks: (block id, the call itself). The Gemini
    // backend emits ToolCall/ToolResult pairs sequentially (one result per
    // call, in order), so popping the front always matches. The call is
    // retained because the inline result card needs its args at result time
    // (create/edit results don't echo the written content).
    let mut pending_tools: VecDeque<(u32, crate::types::ToolCall)> = VecDeque::new();
    // (seg_id, accumulated_raw_text) for every text segment we render
    // this turn — used for markdown rendering at end-of-stream.
    let mut text_segments: Vec<(u32, String)> = vec![(seg_id, String::new())];
    // Did this turn put ANYTHING visible on screen (text or a tool call)?
    let mut any_visible = false;
    // Completion signals tracked across the stream:
    let mut saw_tool_call = false; // any goal-step tool action this turn?
    let mut saw_finish = false; // the model called `finish`?
    let mut saw_question = false; // the model called `ask_question` (blocking)?
    let mut saw_thinking = false; // any reasoning deltas streamed this turn?

    let response = match agent.chat(prompt).await {
        Ok(r) => r,
        Err(err) => {
            report_turn_error("agent.chat", &format!("{err}"), assistant_turn_id);
            return TurnOutcome::Error;
        }
    };
    let mut cursor = response.chunks();

    while let Some(item) = cursor.next().await {
        // Honor a stop request (checked per chunk — cooperative).
        if TURN_CANCEL.with(|c| c.get()) {
            break;
        }
        match item {
            Ok(StreamChunk::Text { text, .. }) => {
                if !text.is_empty() {
                    any_visible = true;
                    let (cur_id, cur_text) = text_segments
                        .last_mut()
                        .expect("text_segments seeded at start of turn");
                    cur_text.push_str(&text);
                    let inner = html! { (cur_text) }.into_string();
                    dom::swap_inner(&format!("seg-{cur_id}"), &inner);
                    dom::scroll_to_bottom("transcript");
                }
            }
            Ok(StreamChunk::ToolCall(call)) => {
                any_visible = true;
                // `finish` and `ask_question` are completion / blocking
                // signals, NOT goal steps — they end the autonomous loop
                // (finish = done, ask_question = waiting on the user). Only a
                // real goal-step tool marks the turn as mid-goal / continuable.
                if call.name == "finish" {
                    saw_finish = true;
                } else if call.name == "ask_question" {
                    saw_question = true;
                } else {
                    saw_tool_call = true;
                }
                let tool_seg_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                dom::append_html(
                    &assistant_body_id,
                    &templates::tool_call_block(tool_seg_id, &call).into_string(),
                );
                pending_tools.push_back((tool_seg_id, call));

                // Open a fresh text segment for whatever the model
                // says after the tool call.
                seg_id = APP.with(|cell| cell.borrow_mut().alloc_id());
                text_segments.push((seg_id, String::new()));
                dom::append_html(
                    &assistant_body_id,
                    &templates::text_segment(seg_id, "").into_string(),
                );
                dom::scroll_to_bottom("transcript");
            }
            Ok(StreamChunk::ToolResult(result)) => {
                if let Some((tool_seg_id, call)) = pending_tools.pop_front() {
                    let result_target = format!("tool-{tool_seg_id}-result");
                    dom::swap_inner(
                        &result_target,
                        &templates::tool_call_result(&result).into_string(),
                    );
                    // Inline result card under the pill (file / directory /
                    // display outputs) so the transcript stays chronological
                    // without tab-hopping. For render_html the framebuffer is
                    // already painted (synchronous render before the result
                    // chunk), so a thumbnail is a cheap canvas snapshot;
                    // run_cartridge frames arrive async from the worker, so
                    // it gets the marker card only.
                    let thumb = if call.name == "render_html" && result.error.is_none() {
                        super::display::snapshot_data_url()
                    } else {
                        None
                    };
                    if let Some(card) = templates::inline_result_card(
                        &call.name,
                        &call.args,
                        &result,
                        thumb.as_deref(),
                    ) {
                        dom::swap_inner(
                            &format!("tool-{tool_seg_id}-card"),
                            &card.into_string(),
                        );
                    }
                    dom::scroll_to_bottom("transcript");
                } else {
                    // No pending tool block to attach this result to — the
                    // backend emitted a ToolResult without a preceding
                    // ToolCall (out-of-order / duplicate). Surface it instead
                    // of dropping it silently.
                    web_sys::console::warn_1(&JsValue::from_str(
                        "orphaned ToolResult (no pending tool call) — dropping",
                    ));
                }
            }
            Ok(StreamChunk::Thought { .. }) => {
                // Thoughts intentionally not surfaced (yet), but record that the
                // model DID reason this turn. A turn that streamed only thinking
                // and no final text was almost certainly TRUNCATED mid-answer
                // (output budget exhausted) — that case is retried, not
                // dead-ended as "(empty response)".
                saw_thinking = true;
            }
            Err(err) => {
                report_turn_error("stream", &format!("{err}"), assistant_turn_id);
                return TurnOutcome::Error;
            }
        }
    }

    // Stream done — re-render each text segment as markdown.
    for (id, raw) in &text_segments {
        if raw.is_empty() {
            continue;
        }
        let html_str = templates::rendered_markdown(raw).into_string();
        dom::swap_inner(&format!("seg-{id}"), &html_str);
    }

    mark_turn_done(assistant_turn_id);

    // The stream completed without error but produced no visible output.
    // Classify WHY (from the model's finish-reason note + whether it reasoned)
    // so the message names the likely cause + remedy, and so a TRUNCATED turn
    // (model ran out of output budget mid-answer) can be retried rather than
    // dead-ended as a flat "(empty response)".
    let empty_kind = if !any_visible && !TURN_CANCEL.with(|c| c.get()) {
        let kind = classify_empty(response.finish_note().as_deref(), saw_thinking);
        let body_id = format!("turn-body-{assistant_turn_id}");
        // For the RETRYABLE (truncated) case, don't print a scary error — the
        // loop will auto-continue and the real answer follows. Only print a
        // message for the terminal cases (nothing more is coming).
        if !matches!(kind, EmptyKind::Truncated) {
            dom::append_html(
                &body_id,
                &format!(
                    "<div class=\"turn-error\">{}</div>",
                    dom::msg_span(dom::Msg::Muted, empty_message(kind))
                ),
            );
            dom::scroll_to_bottom("transcript");
        }
        Some(kind)
    } else {
        None
    };

    // If the user hit stop, append a short redirect prompt — unless this
    // cancel is a promote-to-background, where the work CONTINUES headless
    // and the confirmation lands in the status line instead.
    if TURN_CANCEL.with(|c| c.get()) {
        if !PROMOTE_REQUESTED.with(|c| c.get()) {
            let note_id = APP.with(|cell| cell.borrow_mut().alloc_id());
            dom::append_html(
                "transcript",
                &templates::turn(
                    note_id,
                    "assistant",
                    templates::text_segment(note_id, "Stopped. What should I do instead?"),
                    false,
                )
                .into_string(),
            );
            dom::scroll_to_bottom("transcript");
        }
        return TurnOutcome::Cancelled;
    }

    APP.with(|cell| cell.borrow_mut().turn_count += 1);

    // Classify how the turn ended for the continuous-execution loop. The
    // cancel case is handled by the early return above, so it's not passed in.
    // `empty_kind` is Some only for a no-visible-output turn; a Truncated one
    // becomes the RETRYABLE `EmptyTruncated` so the loop continues toward an
    // answer instead of dead-ending.
    let retryable_empty = matches!(empty_kind, Some(EmptyKind::Truncated));
    classify_turn(
        saw_finish,
        saw_question,
        saw_tool_call,
        any_visible,
        retryable_empty,
    )
}

/// Surface a turn failure. Renders the error INTO the assistant bubble
/// (so a failed turn never looks like a silent blank reply) AND mirrors a
/// short form to the status line. If it looks like a credits/quota problem
/// or an auth / API-key problem (the most common first-run failures), the
/// in-bubble message explains the likely cause and the next step.
fn report_turn_error(context: &str, err: &str, assistant_turn_id: u32) {
    mark_turn_done(assistant_turn_id);
    let lower = err.to_lowercase();
    // The proxy's token-freshness rejection is NOT an API-key problem —
    // with per-request token minting the only remaining cause is a device
    // clock off by more than the proxy's 5-minute window. Don't pop the
    // Gemini key modal at a platform-credits user for it.
    let stale_token = lower.contains("stale or future timestamp");
    let looks_like_auth = !stale_token
        && (lower.contains("api key")
        || lower.contains("api_key")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("permission_denied")
        || lower.contains("unauthenticated"));
    // The credit proxy 402s when there's no active session / no $LH for the
    // signing address. On a subdomain that address is this origin's local
    // credit key — distinct from the apex wallet — so "I redeemed credits"
    // and "this origin has credits" are not the same thing.
    let looks_like_credits = lower.contains("402")
        || lower.contains("payment required")
        || lower.contains("insufficient")
        || lower.contains("no active session")
        || lower.contains("quota")
        || lower.contains("429");

    // Visible, escaped message in the transcript bubble. This is the
    // primary surface — the status line is a secondary mirror.
    let bubble = if stale_token {
        format!(
            "request auth went stale — your device clock looks off by more \
             than 5 minutes; sync it and retry. Raw error: {err}"
        )
    } else if looks_like_credits {
        "request rejected (no credits / session for this origin). Open the \
         account tab → platform credits to redeem or open a session, or \
         switch to your own Gemini key. Raw error: "
            .to_string()
            + err
    } else if looks_like_auth {
        format!("model rejected the API key — check your Gemini key. Raw error: {err}")
    } else {
        format!("{context} failed: {err}")
    };
    let body_id = format!("turn-body-{assistant_turn_id}");
    dom::append_html(
        &body_id,
        &format!(
            "<div class=\"turn-error\">{}</div>",
            dom::msg_span(dom::Msg::Error, &bubble)
        ),
    );
    dom::scroll_to_bottom("transcript");

    if stale_token {
        dom::set_status("auth token went stale — check your device clock, then retry", true);
    } else if looks_like_auth {
        dom::set_status("API key rejected — check your Gemini key.", true);
        super::show_api_key_modal();
    } else if looks_like_credits {
        dom::set_status("no credits / session for this origin — see the account tab.", true);
    } else {
        // The bubble above already carries the full raw error — repeating it
        // here painted the same wall of JSON twice (once in the transcript,
        // once in the input container). Keep the status line to a short
        // marker so the aria-live region still announces the failure.
        dom::set_status("turn failed — see the message above", true);
    }
}

/// Surface a pre-turn failure IN THE STREAM (feedback #64): errors belong in
/// the chronological transcript, not only the footer status line. Renders the
/// same `.turn-error` shape `report_turn_error` uses, inside a bare assistant
/// turn block, and scrolls it into view.
fn transcript_system_error(text: &str) {
    dom::append_html(
        "transcript",
        &format!(
            "<div class=\"turn assistant\"><div class=\"body\"><div class=\"turn-error\">{}</div></div></div>",
            dom::msg_span(dom::Msg::Error, text)
        ),
    );
    dom::scroll_to_bottom("transcript");
}

/// The in-tab auto-compaction ceiling — shared by the session config and the
/// context-fullness bar so the bar's "full" always means "compaction next".
pub(crate) const COMPACTION_THRESHOLD: u32 = 128_000;

/// Repaint the context-fullness bar (feedback #59): fill = the last turn's
/// live prompt tokens vs [`COMPACTION_THRESHOLD`]. A full bar means the next
/// turn will summarize the old history prefix.
fn update_context_bar(agent: &crate::Agent) {
    let tokens = agent
        .conversation()
        .last_turn_usage()
        .and_then(|u| u.prompt_token_count)
        .unwrap_or(0)
        .max(0) as u64;
    let pct = ((tokens as f64 / COMPACTION_THRESHOLD as f64) * 100.0).min(100.0);
    if let Some(el) = dom::by_id("ctx-fill") {
        let _ = el.set_attribute("style", &format!("width:{pct:.1}%"));
    }
    if let Some(el) = dom::by_id("ctx-bar") {
        let _ = el.set_attribute(
            "title",
            &format!("context: {tokens} / {COMPACTION_THRESHOLD} tokens"),
        );
    }
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
