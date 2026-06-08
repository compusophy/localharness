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

/// RAII cleanup for a turn: clears the active/cancel flags and restores
/// the send button (if the stop button is currently shown) on every
/// exit path, including early returns and future cancellation.
struct TurnGuard;
impl Drop for TurnGuard {
    fn drop(&mut self) {
        TURN_ACTIVE.with(|c| c.set(false));
        TURN_CANCEL.with(|c| c.set(false));
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
    // Credits mode (base_url set): ensure a session is open so the proxy
    // accepts the call. Free in beta; silent best-effort.
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
    // From here, every return path resets the flags + restores the
    // send button via Drop.
    let _turn_guard = TurnGuard;

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
            dom::set_status(&format!("session start failed: {err:?}"), true);
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

    // Swap the send arrow for a stop button for the whole (possibly
    // multi-turn) run; the guard / loop-end restores it.
    dom::swap_outer("terminal-send", &templates::stop_button().into_string());

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
    loop {
        if TURN_CANCEL.with(|c| c.get()) {
            break;
        }
        let outcome = stream_turn(&agent, next_input).await;

        // Persist + refresh after every turn so tool-created files and the
        // history marker show up incrementally (not just at the very end).
        super::history::save_from_agent().await;
        super::opfs::refresh().await;

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
            // The turn ended right after tool activity without an explicit
            // completion signal — keep going toward the goal.
            TurnOutcome::Incomplete => {
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
                next_input = TurnInput::AutoContinue;
            }
        }
    }

    // Restore the send button if the stop button is still showing.
    if dom::by_id("terminal-stop").is_some() {
        dom::swap_outer("terminal-stop", &templates::send_button().into_string());
    }
}

/// Upper bound on automatic "continue toward the goal" turns per single
/// user message. A safety cap so a confused model can't loop forever
/// (and to bound credit spend). The user can always send again to extend.
const MAX_AUTO_CONTINUATIONS: u32 = 10;

/// Internal nudge fed to the model on an auto-continuation. Kept terse so
/// it doesn't derail the goal; instructs the model to either keep working
/// or call `finish` / ask a question when it's actually done or blocked.
const AUTO_CONTINUE_NUDGE: &str = "Continue toward the user's goal. If the task is \
fully complete, call the `finish` tool. If you're blocked or need a decision, ask \
the user a question. Otherwise take the next step now without waiting.";

/// What a single streamed turn carries in.
enum TurnInput {
    /// A real user message — renders a user bubble.
    User(String),
    /// An internal auto-continuation nudge — no user bubble.
    AutoContinue,
}

/// How a single streamed turn ended — drives the continuous-execution loop.
#[derive(Debug, PartialEq, Eq)]
enum TurnOutcome {
    /// The model called `finish` — task explicitly complete. Stop.
    Finished,
    /// The turn ended on a final text answer with no tool activity this
    /// turn (plain conversation / a closing reply / a question). Stop —
    /// don't spam empty auto-continues on a chat reply.
    FinalAnswer,
    /// The turn performed tool actions and ended WITHOUT a completion
    /// signal — the model likely stopped mid-goal. Auto-continue.
    Incomplete,
    /// Nothing visible was produced (empty response). Stop.
    Empty,
    /// The turn errored (already surfaced in the transcript). Stop.
    Error,
    /// The user hit stop mid-turn. Stop.
    Cancelled,
}

/// Stream ONE agent turn into the transcript and report how it ended.
/// Renders a user bubble only for [`TurnInput::User`]; auto-continuations
/// render just the assistant bubble so the internal nudge never shows.
async fn stream_turn(agent: &Agent, input: TurnInput) -> TurnOutcome {
    let (prompt, render_user) = match input {
        TurnInput::User(p) => (p, true),
        TurnInput::AutoContinue => (AUTO_CONTINUE_NUDGE.to_string(), false),
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

    // FIFO of pending tool-block ids. The Gemini backend emits
    // ToolCall/ToolResult pairs sequentially (one result per call,
    // in order), so popping the front always matches.
    let mut pending_tools: VecDeque<u32> = VecDeque::new();
    // (seg_id, accumulated_raw_text) for every text segment we render
    // this turn — used for markdown rendering at end-of-stream.
    let mut text_segments: Vec<(u32, String)> = vec![(seg_id, String::new())];
    // Did this turn put ANYTHING visible on screen (text or a tool call)?
    let mut any_visible = false;
    // Completion signals tracked across the stream:
    let mut saw_tool_call = false; // any goal-step tool action this turn?
    let mut saw_finish = false; // the model called `finish`?
    let mut saw_question = false; // the model called `ask_question` (blocking)?

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
                pending_tools.push_back(tool_seg_id);

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
                if let Some(tool_seg_id) = pending_tools.pop_front() {
                    let result_target = format!("tool-{tool_seg_id}-result");
                    dom::swap_inner(
                        &result_target,
                        &templates::tool_call_result(&result).into_string(),
                    );
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
                // Thoughts intentionally not surfaced (yet).
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
    if !any_visible && !TURN_CANCEL.with(|c| c.get()) {
        let body_id = format!("turn-body-{assistant_turn_id}");
        dom::append_html(
            &body_id,
            &format!(
                "<div class=\"turn-error\">{}</div>",
                dom::msg_span(
                    dom::Msg::Muted,
                    "(empty response — the model returned no text. If you're on \
                     platform credits, check your session/balance in the account tab.)"
                )
            ),
        );
        dom::scroll_to_bottom("transcript");
    }

    // Stash cumulative token usage for the admin Usage tab.
    if let Some(total) = agent.cumulative_usage().total_token_count {
        APP.with(|cell| cell.borrow_mut().total_tokens = total.max(0) as u64);
    }

    // If the user hit stop, append a short redirect prompt.
    if TURN_CANCEL.with(|c| c.get()) {
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
        return TurnOutcome::Cancelled;
    }

    APP.with(|cell| cell.borrow_mut().turn_count += 1);

    // Classify how the turn ended for the continuous-execution loop. The
    // cancel case is handled by the early return above, so it's not passed in.
    classify_turn(saw_finish, saw_question, saw_tool_call, any_visible)
}

/// Decide how a completed (non-cancelled) turn ended, for the
/// continuous-execution loop. Pure over the four signals tracked while
/// streaming so it can be unit-tested without a browser:
/// - `saw_finish`: the model called `finish` → task complete, stop.
/// - `saw_question`: the model called `ask_question` → it's blocking on the
///   user, stop and wait (do NOT auto-continue — that would spam the model
///   and never let the user answer).
/// - `saw_tool_call`: a goal-step tool ran (NOT finish / ask_question).
/// - `any_visible`: anything (text or a tool block) was rendered.
///
/// Precedence: `finish` wins over everything (the model can call other tools
/// then `finish` in the same turn — that's still "done"). A blocking question
/// stops next. Then truly-empty turns. Then a goal-step-only turn auto-continues
/// (`Incomplete`). A pure text reply with no tool activity is a `FinalAnswer`.
fn classify_turn(
    saw_finish: bool,
    saw_question: bool,
    saw_tool_call: bool,
    any_visible: bool,
) -> TurnOutcome {
    if saw_finish {
        TurnOutcome::Finished
    } else if saw_question {
        // Blocking on the user — a conversational stop, like FinalAnswer.
        // Never auto-continue (the default ask_question returns "skipped",
        // so a continue would loop the model 10x without a real answer).
        TurnOutcome::FinalAnswer
    } else if !any_visible {
        TurnOutcome::Empty
    } else if saw_tool_call {
        // Ended right after tool activity with no explicit completion —
        // the model probably has more to do. Auto-continue.
        TurnOutcome::Incomplete
    } else {
        // Pure text reply, no tool calls — a conversational answer or a
        // question. Don't auto-continue (would spam empty turns).
        TurnOutcome::FinalAnswer
    }
}

/// Surface a turn failure. Renders the error INTO the assistant bubble
/// (so a failed turn never looks like a silent blank reply) AND mirrors a
/// short form to the status line. If it looks like a credits/quota problem
/// or an auth / API-key problem (the most common first-run failures), the
/// in-bubble message explains the likely cause and the next step.
fn report_turn_error(context: &str, err: &str, assistant_turn_id: u32) {
    mark_turn_done(assistant_turn_id);
    let lower = err.to_lowercase();
    let looks_like_auth = lower.contains("api key")
        || lower.contains("api_key")
        || lower.contains("401")
        || lower.contains("403")
        || lower.contains("permission_denied")
        || lower.contains("unauthenticated");
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
    let bubble = if looks_like_credits {
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

    if looks_like_auth {
        dom::set_status("API key rejected — check your Gemini key.", true);
        super::show_api_key_modal();
    } else if looks_like_credits {
        dom::set_status("no credits / session for this origin — see the account tab.", true);
    } else {
        dom::set_status(&format!("{context}: {err}"), true);
    }
}

pub(crate) async fn start_session(
    key: &str,
    base_url: Option<url::Url>,
    identity: &str,
) -> Result<(), JsValue> {
    // System instruction — the agent needs to know what it's running
    // inside and what its filesystem looks like. Without this, prompts
    // like "what is pricing" produce blind tool calls because the
    // model has no priors about the localharness environment.
    let host = super::tenant::current();
    let agent_name = match &host {
        super::tenant::Host::Tenant(name) => name.clone(),
        _ => "this agent".to_string(),
    };

    // Which LLM backend this session uses — needed up front so the prompt
    // advertises ONLY the tools the chosen backend actually registers. The
    // Anthropic backend reuses the Gemini `register_builtins` with both
    // client slots `None`, so the two Gemini-client-coupled builtins
    // (`start_subagent`, `generate_image`) do NOT register on Claude. Gate
    // their prompt lines on the backend so a Claude agent is never told it
    // has tools it can't call.
    let model = super::model::load().await;
    let on_anthropic = super::model::is_anthropic(&model);
    // Prompt fragments for the two Gemini-only builtins — empty on Anthropic.
    let start_subagent_line = if on_anthropic {
        ""
    } else {
        "  • start_subagent(system_instructions, prompt) — spawn a one-shot \
           text-only subagent with no tool access. Use for self-contained \
           reasoning / writing tasks you want isolated from your context.\n"
    };
    let generate_image_line = if on_anthropic {
        ""
    } else {
        "  • generate_image(prompt) — produce an image from a text prompt.\n"
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
           • create_subdomain(name, persona?, prefund_lh?) — register a NEW \
             name-only <name>.localharness.xyz subdomain on-chain, owned by your \
             owner's master wallet (the ACTOR MODEL). Use this to make a new \
             subdomain/agent WITHOUT an app: when the user says \
             \"create/make/spin up a subdomain\" or \"make me a new <name>\", \
             call THIS — never run_cartridge, which does NOT create a subdomain. \
             OPTIONAL actor extras: `persona` publishes the new agent's on-chain \
             system instruction; `prefund_lh` moves that much $LH from YOUR \
             wallet into the new agent's token-bound account (its own spendable \
             wallet — to pay other agents). Both omitted = a bare subdomain. \
             Returns {{ name, url, owner, tx_hash, persona_set?, prefunded_lh?, \
             tba? }}; after it succeeds, give the user the returned `url` as a \
             clickable link. Each subdomain is its own agent tab with its own \
             per-origin sandbox.\n\
           • create_and_publish_app(name, source, persona?, prefund_lh?) — \
             ONE-SHOT: register a new <name>.localharness.xyz AND publish a \
             compiled rustlite cartridge as its fullscreen public face (compile \
             + register + publish in a single call). Use this whenever the user \
             wants a subdomain that IS an app — \"make me a clock/<app> \
             subdomain\". This is how you create a subdomain with an app from \
             here (a per-origin sandbox means you can't write another \
             subdomain's files directly). Same OPTIONAL actor extras as \
             create_subdomain (`persona`, `prefund_lh`), folded into the same \
             sponsored tx. Returns {{ name, url, tx_hash, persona_set?, \
             prefunded_lh?, tba? }}.\n\
           • batch_create_subdomains(names) — register MANY subdomains in ONE \
             on-chain transaction. Use THIS instead of calling create_subdomain \
             repeatedly when the user asks for more than one name at once \
             (\"register a, b and c\", \"make me 5 subdomains\", \"spin up \
             a-b-c-d\"). Taken/invalid names are skipped and reported in \
             `skipped`. Max 20 per call. Returns {{ registered, skipped, count, \
             tx_hash, urls }}.\n\
           • release_subdomain(name, confirmation) — DESTRUCTIVE + \
             IRREVERSIBLE: burns the subdomain NFT and frees the name. \
             Requires `confirmation` to EXACTLY equal `name` — and you must \
             only pass that after the OWNER has TYPED the exact name in \
             chat. Never invent or auto-fill the confirmation. Refuses your \
             MAIN.\n\
           • bulk_release_subdomains(confirmation, names?) — DESTRUCTIVE + \
             IRREVERSIBLE batch: burns MANY subdomains at once and frees their \
             names. Omit `names` to release ALL non-MAIN holdings; pass `names` \
             for a subset. ONE master confirmation, not per-name. ALWAYS call it \
             FIRST with confirmation empty to get the exact list it will \
             release, show the user that list, then ask them to TYPE the phrase \
             \"release all non-main\" and only then retry with that \
             confirmation. Never auto-fill it. Always refuses your MAIN.\n\
           • list_subdomains() — list every subdomain your owner holds \
             (their identity's holdings). Read-only; use when asked what \
             subdomains/agents they have.\n\
           • send_lh(recipient, amount) — TRANSFER real $LH credits from your \
             owner's wallet. `recipient` is a raw 0x… address OR a subdomain \
             name (the funds go to that name's on-chain OWNER). `amount` is a \
             decimal $LH figure (\"5\", \"1.5\"), must be > 0. This MOVES VALUE \
             — always confirm the recipient and amount with the owner before \
             calling. Returns {{ amount, recipient, resolved_recipient, tx_hash \
             }}.\n\
         {start_subagent_line}\
           • spawn_recursive_subagent(system_instructions, prompt) — spawn a \
             full subagent with the same tool surface YOU have (filesystem, \
             create_subdomain, start_subagent, etc.). Use for delegation that \
             needs tools. Recursion depth is implicit (each subagent has its \
             own context; cost grows with depth — don't chain more than 3 \
             levels unless the user asked).\n\
           • call_agent(name, message) — send a message to another agent by \
             subdomain name and receive its text response. The target agent \
             must have an API key configured. Use this for inter-agent \
             collaboration, delegation, or multi-agent workflows.\n\
           • discover_agents(query) — find agents by capability/persona, then \
             call_agent them. Read-only registry scan: returns the names + \
             persona snippets of agents whose name OR on-chain persona matches \
             `query` (ranked, name hits first). Use it to FIND a peer to \
             delegate to before calling call_agent.\n\
           • compile_rustlite(source, function?, args?) — compile Rust-subset \
             source code to wasm and execute a function. Supports structs, \
             enums, fns, match, if/else, while/loop, let mut. No traits, \
             no generics, no references. Returns the i32 result.\n\
           • run_cartridge(source) — compile a rustlite cartridge and run it \
             on the VISUAL DISPLAY the user sees (a 256x144 pixel framebuffer). \
             The cartridge exports `fn frame(t: i32)` (animated, t = elapsed ms) \
             or `fn render()`, and draws via `use host::display;`. Drawing: \
             clear(rgb), fill_rect(x,y,w,h,rgb), set_pixel(x,y,rgb), \
             draw_char(x,y,code,rgb,scale) (ASCII code, e.g. 65='A'), \
             draw_number(x,y,value,rgb,scale) (decimal int), present() (call \
             last). Input polled each frame: pointer_x(), pointer_y(), \
             pointer_down() (1 while pressed). State across frames (no globals \
             in rustlite): state_get(slot)/state_set(slot,value), 64 int slots. \
             Colors 0xRRGGBB (white = 16777215). Font covers 0-9, A-Z, a-z, \
             space, and common punctuation (! ? , : ; ' \" . - + / = etc.). \
             You CAN build real interactive apps now — a \
             clickable button is a fill_rect + label, hit-tested against \
             pointer_down() + pointer position, with state in the slots. \
             NETWORKING (multiplayer / multi-device sync) via `use host::net;`: \
             open(url_ptr) -> handle (WebSocket to a length-prefixed string at \
             url_ptr in memory; -1 on error), send(handle, ptr) -> 1/0 (send the \
             length-prefixed string at ptr), poll(handle, out_ptr, max) -> len \
             (copy the next inbound message into memory at out_ptr, <= max bytes; \
             0 if the inbox is empty), status(handle) (0 connecting / 1 open / \
             2 closing / 3 closed), close(handle). Drain poll() each frame to \
             receive. Use a public WebSocket relay for collaborative apps. \
             Use this to render visual/animated content on THIS subdomain's \
             display when the user asks to build/draw/show an app or graphic \
             HERE. It runs on the CURRENT tab and does NOT create a subdomain \
             and is NEVER how you produce a link — for those, use \
             create_subdomain. \
             Each run is auto-saved to `cartridge.rl` (visible in files, \
             survives reload). This is what 'build/run/show me an app' (on \
             this tab) means — run_cartridge launches it live on the DISPLAY, \
             non-fullscreen, no reload. ONLY when the user EXPLICITLY asks to make this \
             subdomain PERMANENTLY BECOME the app (fullscreen on every load, \
             no IDE chrome) should you ALSO save the same source to `app.rl` \
             via create_file. Never write `app.rl` for an ordinary app \
             request — it forces a fullscreen takeover the user didn't ask \
             for and doesn't even run until the next reload.\n\
           • render_html(source) — render an HTML document onto the VISUAL \
             DISPLAY. The display CAN show HTML: this lays out block-level \
             text (h1-h6, p, ul/li, blockquote, br) word-wrapped in the \
             bitmap font, monochrome. It is a snapshot — no JavaScript, no \
             CSS, no images (headings just render bigger). For interactive or \
             animated apps use run_cartridge. Pair with create_file to also \
             save the HTML as `index.html`. (Opening an .html file from the \
             files panel renders it here too.)\n\
           • submit_feedback(text) — submit feedback on-chain via the \
             FeedbackFacet. Emits a FeedbackSubmitted event on the registry \
             diamond. Use when the user asks to leave feedback or to report \
             issues about another agent. Keep it SHORT — a few sentences, \
             under ~2000 bytes. Summarize; do NOT paste long multi-paragraph \
             reports. Text over 2048 bytes is rejected before it reaches the \
             chain.\n\
         {generate_image_line}\
           • configure_agent(system_prompt?, tools?, reset?) — read or change \
             YOUR OWN config (custom system prompt + tool allowlist), stored in \
             `agent.json`. Use this when the user asks you to change your \
             personality/role/instructions or restrict your tools. Changes \
             apply on your NEXT session. finish/ask_question/configure_agent \
             can never be disabled.\n\
           • read_self_docs() — read YOUR OWN runtime documentation (the live \
             https://localharness.xyz/llms.txt plus an embedded summary). \
             Read-only. Use it to self-diagnose, accurately explain your own \
             platform/SDK, or give grounded feedback about it instead of guessing.\n\
           • finish(result?) — signal that the task is COMPLETE. Call this when, \
             and only when, you've fully satisfied the user's request — it ends \
             the autonomous loop. If you still have steps left, just keep going \
             (don't wait to be nudged); if you're blocked or need input, ask the \
             user a question instead of calling finish.\n\n\
         \
         === Conventions ===\n\
         • Pick the right tool — do NOT default to run_cartridge: \
           \"create / make / spin up a new subdomain\" → create_subdomain; \
           \"build / show / draw an app or anything visual\" on THIS tab → \
           run_cartridge; \"give me a link / hyperlink / URL to <name>\" → \
           just write the Markdown link [<name>](https://<name>.localharness.xyz/) \
           as text, with NO tool call (call list_subdomains first only if you \
           must confirm the name exists). A request for a link is NEVER a \
           reason to run a cartridge.\n\
         • Registering MULTIPLE names at once → batch_create_subdomains(names), \
           ONE tx, NOT a create_subdomain loop. A loop spends one sponsored \
           transaction per name and eats your auto-continue budget; the batch \
           registers them all in a single transaction and reports which were \
           skipped (taken/invalid).\n\
         • On-chain actions (create_subdomain, submit_feedback, publishing \
           a public face, etc.) are SPONSORED and signed automatically by the \
           owner's master wallet behind the scenes — there is NO wallet popup, \
           prompt, or modal for the user to approve. Transactions just happen, \
           zero-click. NEVER tell the user to approve/confirm a transaction, \
           check for a wallet prompt, or sign anything; just report the result.\n\
         • DESTRUCTIVE / IRREVERSIBLE actions are the EXCEPTION to zero-click \
           and the ONE thing you must never do casually: releasing/burning a \
           subdomain (release_subdomain), deleting files, or anything that \
           destroys an asset, NFT, wallet, or identity. NEVER perform one \
           unless, in THIS conversation, the owner has TYPED an explicit \
           confirmation — for release_subdomain, the exact subdomain name; \
           for bulk_release_subdomains, the literal phrase \"release all \
           non-main\" after you've shown the user the list of names that will \
           be burned. A \
           vague \"yes\", \"do it\", or merely mentioning the thing is NOT \
           consent; require the typed phrase, and if it's absent, ask for it \
           and STOP. NEVER invent or auto-fill a confirmation argument. When \
           unsure whether something is destructive, treat it as destructive.\n\
         • Files at the OPFS root are the user's. These internal files are \
           managed by the platform — read only if asked, NEVER write or delete: \
           `.lh_history.json` (conversation history — this is what 'clear \
           history' targets), `.lh_api_key`, `.lh_owner`, `.lh_feedback.txt`, \
           and `agent.json` (your config — change it via configure_agent, not \
           by editing the file).\n\
         • Keep responses concise and conversational. The user is on the same \
           page; they don't need you restating what you just did.\n\
         • Don't speculate about filesystem contents — call list_directory first \
           when you actually need to know.\n\
         • Don't blindly call tools when the user is just chatting. \"hi\" / \
           \"what can you do?\" don't need a tool call.\n\
         • When you do call a tool, lead with a short one-line note on what \
           you're about to do (e.g. \"checking your files…\") so the turn is \
           never silent — but don't re-narrate the call's args or dump its \
           result afterward; both are already visible in the transcript."
    );

    // Self-knowledge: append a concise runtime digest so the agent has
    // grounded priors about its OWN platform/SDK every turn (and knows it
    // can read the full live spec via read_self_docs). This is the
    // always-available, offline half of feature 1b.
    let system_instructions = format!(
        "{system_instructions}\n\n{}",
        super::self_docs::system_prompt_digest()
    );

    // Owner customization: append the contents of `.lh_system_prompt.txt`
    // (if any) under a clear header so the model sees the baked-in
    // tooling docs first, then the owner's overrides on top. This is
    // the studio-MVP hook — owners differentiate their agent's
    // personality / role / constraints without forking the bundle.
    let system_instructions = match super::system_prompt::load().await {
        Some(custom) => {
            format!("{system_instructions}\n\n=== Owner instructions ===\n{custom}")
        }
        None => system_instructions,
    };

    let capabilities = match super::tool_allowlist::load().await {
        Some(mut tools) => {
            // Always union the golden tools so neither the owner nor the
            // agent can disable recovery (finish / ask_question /
            // configure_agent).
            for golden in super::tool_allowlist::GOLDEN {
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
    // History from a previous session (if any), consumed once here so a
    // backend switch doesn't lose the transcript.
    let pending_history = super::history::take_pending();

    let agent = if super::model::is_local(&model) {
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
                .with_filesystem(super::shared_opfs())
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
    } else if super::model::is_anthropic(&model) {
        let mut cfg = crate::AnthropicAgentConfig::new(key.to_string())
            .with_model(model.clone())
            .with_capabilities(capabilities)
            .with_policies(vec![policy::allow_all()])
            .with_filesystem(super::shared_opfs())
            .with_system_instructions(system_instructions)
            .with_tool(create_subdomain_tool())
            .with_tool(create_and_publish_app_tool())
            .with_tool(batch_create_subdomains_tool())
            .with_tool(release_subdomain_tool())
            .with_tool(bulk_release_subdomains_tool())
            .with_tool(list_subdomains_tool())
            .with_tool(discover_agents_tool())
            .with_tool(send_lh_tool())
            .with_tool(submit_feedback_tool())
            .with_tool(super::self_docs::read_self_docs_tool())
            .with_tool(clear_context_tool())
            .with_tool(compact_context_tool())
            .with_tool(spawn_recursive_subagent_tool(captured_key, base_url.clone()));
        // Credits mode: route Anthropic through the credit proxy (it serves
        // `/v1/messages`). BYOK has no direct-Anthropic path here, so this is
        // a no-op without a proxy base_url and the call would hit
        // api.anthropic.com with the raw key.
        if let Some(b) = &base_url {
            cfg = cfg.with_base_url(b.clone());
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
            .with_filesystem(super::shared_opfs())
            .with_system_instructions(system_instructions)
            .with_tool(create_subdomain_tool())
            .with_tool(create_and_publish_app_tool())
            .with_tool(batch_create_subdomains_tool())
            .with_tool(release_subdomain_tool())
            .with_tool(bulk_release_subdomains_tool())
            .with_tool(list_subdomains_tool())
            .with_tool(discover_agents_tool())
            .with_tool(send_lh_tool())
            .with_tool(submit_feedback_tool())
            .with_tool(super::self_docs::read_self_docs_tool())
            .with_tool(clear_context_tool())
            .with_tool(compact_context_tool())
            .with_tool(spawn_recursive_subagent_tool(captured_key, base_url.clone()));
        // Credits mode: route the whole agent through the credit proxy. BYOK
        // leaves base_url None → direct to generativelanguage.googleapis.com.
        if let Some(b) = &base_url {
            cfg = cfg.with_base_url(b.clone());
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

fn mark_turn_done(turn_id: u32) {
    let id = format!("turn-{turn_id}");
    if let Some(el) = dom::by_id(&id) {
        let cls = el.class_name();
        let new_cls: Vec<&str> =
            cls.split_whitespace().filter(|c| *c != "streaming").collect();
        el.set_class_name(&new_cls.join(" "));
    }
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
        "pay {} LH per turn to this agent",
        price_wei / 1_000_000_000_000_000_000u128,
    );

    // Build ERC-20 transfer(tba, price_wei) calldata against the
    // credits token. Sponsored Tempo tx: visitor's wallet (at apex)
    // signs the sender_hash, the bundle sponsor pays gas in AlphaUSD.
    // Visitor holds zero of anything except the LH they're spending.
    let tba_bytes = parse_address(&tba)?;
    let mut tba_padded = [0u8; 32];
    tba_padded[12..].copy_from_slice(&tba_bytes);
    let amount_bytes = u256_be(price_wei);
    let selector = transfer_selector();
    let mut calldata = Vec::with_capacity(4 + 32 + 32);
    calldata.extend_from_slice(&selector);
    calldata.extend_from_slice(&tba_padded);
    calldata.extend_from_slice(&amount_bytes);

    let token_addr = parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS)?;
    let call = crate::tempo_tx::TempoCall {
        to: token_addr,
        value_wei: 0,
        input: calldata,
    };

    dom::set_status("payment: signing via apex…", false);
    let tx_hash = super::events::run_sponsored_tempo_call(
        &visitor_address,
        vec![call],
        500_000,
        &purpose,
    )
    .await
    .map_err(|e| format!("payment: {e}"))?;

    Ok(Some(tx_hash))
}

/// The localharness credit proxy origin — a drop-in Gemini base URL
/// (its `vercel.json` rewrites `/v1beta/*` onto the edge fn). Single source
/// of truth lives in `registry` so the native CLI's headless `call` and the
/// browser share one origin.
const CREDIT_PROXY_URL: &str = crate::registry::CREDIT_PROXY_URL;

/// True when the user is on platform `$LH` credits (via the proxy).
/// Persisted in localStorage; **defaults to credits** — a new account
/// uses platform credits with no setup, and BYOK is opt-in via admin →
/// account. Only an explicit "byok" choice flips it off.
pub(crate) fn model_access_is_credits() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("lh_model_access").ok().flatten())
        .map(|v| v != "byok")
        .unwrap_or(true)
}

/// Resolved model access for a chat session.
struct ModelAccess {
    /// Goes in the GeminiClient api-key slot: a Gemini key (BYOK) or the
    /// credit-proxy auth token (credits).
    cfg_auth: String,
    /// Proxy base URL in credits mode; `None` for BYOK (direct to Google).
    base_url: Option<url::Url>,
    /// STABLE restart-detection identity — never the rotating credits
    /// token (which changes every resolve).
    identity: String,
}

/// Lowercase 0x-hex of bytes.
fn hex_of(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The LOCAL signing key for the credit path — master wallet on the
/// apex / seed-bearing origin, else a local per-origin key (loaded or
/// generated + persisted on first use). NEVER the cross-origin iframe
/// signer: the whole credit path is iframe-free.
pub(crate) async fn credit_signer() -> Option<(k256::ecdsa::SigningKey, [u8; 20])> {
    if let Some(pair) =
        APP.with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)))
    {
        return Some(pair);
    }
    if let Some(sk) = super::wallet_store::load_device_key().await {
        let addr = crate::wallet::address(&sk);
        return Some((sk, addr));
    }
    let w = crate::wallet::generate();
    super::wallet_store::persist_device_key(&w.private_key_hex)
        .await
        .ok()?;
    // `w` is Drop (zeroizes its hex) — clone the signer, copy the address.
    Some((w.signer.clone(), w.address))
}

/// The credit identity's 0x address if one already exists locally —
/// does NOT generate (so status refreshes don't mint a key). master
/// wallet, else a persisted device key, else None.
pub(crate) async fn credit_address_existing() -> Option<String> {
    if let Some(a) = APP.with(|c| c.borrow().wallet.as_ref().map(|w| w.address_hex())) {
        return Some(a);
    }
    let sk = super::wallet_store::load_device_key().await?;
    Some(hex_of(&crate::wallet::address(&sk)))
}

/// Resolve how this turn reaches the model. Credits mode mints a fresh
/// proxy auth token `address:timestamp:signature` (personal-signed by
/// the local key); BYOK falls back to the stored Gemini key. `None`
/// only when BYOK has no key (caller then shows the key modal).
async fn resolve_credit_access() -> Option<ModelAccess> {
    if model_access_is_credits() {
        let (signer, addr) = credit_signer().await?;
        let addr_hex = hex_of(&addr); // lowercase 0x — matches the proxy
        let ts = (js_sys::Date::now() / 1000.0) as u64;
        let msg = format!("localharness-proxy:{addr_hex}:{ts}");
        let sig = crate::wallet::personal_sign(&signer, msg.as_bytes());
        return Some(ModelAccess {
            cfg_auth: format!("{addr_hex}:{ts}:{}", hex_of(&sig)),
            base_url: url::Url::parse(CREDIT_PROXY_URL).ok(),
            identity: format!("credits:{addr_hex}"),
        });
    }
    let key = read_api_key().await?;
    Some(ModelAccess {
        cfg_auth: key.clone(),
        base_url: None,
        identity: key,
    })
}

/// Credits mode: fund the PER-REQUEST METER so the proxy debits real `$LH` per
/// call (per-call billing — NOT a free session). Moves any `$LH` sitting in the
/// wallet into the `CreditMeterFacet` (approve + deposit, one sponsored tx); the
/// proxy then debits `creditOf` per request and the balance actually decrements.
/// The `wallet == 0` check makes this idempotent — once moved, there's nothing
/// to re-deposit. Best-effort + silent: a failure just falls through to the
/// proxy's gating (a still-active free session keeps the agent usable).
///
/// NOTE: deposited `$LH` lives in the meter and has no withdraw path — that's
/// fine, it's there to be spent on calls. (Old free sessions still bypass
/// metering until they expire ≤1h; the proxy now PREFERS the funded meter, so
/// once funded, billing is immediate regardless of a lingering session.)
pub(crate) async fn ensure_credit_meter() {
    let Some((signer, addr)) = credit_signer().await else {
        return;
    };
    let addr_hex = hex_of(&addr);
    let wallet = super::registry::token_balance_of(&addr_hex)
        .await
        .unwrap_or(0);
    if wallet == 0 {
        return; // nothing to fund the meter with (already moved, or empty)
    }
    let Ok(fee_payer) = super::sponsor::signer() else {
        return;
    };
    let _ = super::registry::deposit_credits_sponsored(
        &signer,
        &fee_payer,
        wallet,
        super::registry::ALPHA_USD_ADDRESS,
    )
    .await;
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

fn transfer_selector() -> [u8; 4] {
    selector4(b"transfer(address,uint256)")
}

/// First 4 bytes of keccak256 of an ABI function signature.
fn selector4(sig: &[u8]) -> [u8; 4] {
    use sha3::{Digest, Keccak256};
    let mut hasher = Keccak256::new();
    hasher.update(sig);
    let mut out = [0u8; 4];
    out.copy_from_slice(&hasher.finalize()[..4]);
    out
}

/// ERC-20 `transfer(to, amount_wei)` calldata against the `$LH` token — the
/// exact shape `send_lh` builds. `to` is a 20-byte address; `amount_wei` is
/// 18-decimal token wei.
fn lh_transfer_calldata(to: &[u8; 20], amount_wei: u128) -> Vec<u8> {
    let mut to_padded = [0u8; 32];
    to_padded[12..].copy_from_slice(to);
    let mut calldata = Vec::with_capacity(4 + 32 + 32);
    calldata.extend_from_slice(&transfer_selector());
    calldata.extend_from_slice(&to_padded);
    calldata.extend_from_slice(&u256_be(amount_wei));
    calldata
}

/// `createTokenBoundAccount(tokenId)` calldata against the registry diamond.
/// Idempotent: deploys the ERC-6551 account so a counterfactual TBA can hold
/// funds (registry's own helper is private, so we mirror it here — chat.rs
/// already hand-builds calldata for `send_lh` the same way).
fn create_tba_calldata(token_id: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(4 + 32);
    data.extend_from_slice(&selector4(b"createTokenBoundAccount(uint256)"));
    data.extend_from_slice(&u256_be(token_id as u128));
    data
}

/// Result of preparing the optional actor-model extras (persona + prefund) for
/// a freshly-registered subdomain. `calls` are appended to the same sponsored
/// Tempo tx that publishes / sets up the new token; `extra_gas` is added to the
/// base gas budget.
struct ActorSetup {
    calls: Vec<crate::tempo_tx::TempoCall>,
    extra_gas: u128,
    prefunded_lh: Option<String>,
    tba: Option<String>,
    persona_set: bool,
}

/// Build the optional persona + prefund calls for `create_subdomain` /
/// `create_and_publish_app` (the ACTOR MODEL).
///
/// **Billing-semantics finding → prefund recipient = the new subdomain's TBA.**
/// The credit proxy keys `$LH` usage by the *signing EOA address*
/// (`sessionExpiryOf(address)` / `creditOf(address)` in `proxy/api/gemini.ts`),
/// and the creator already OWNS the new name, so funds sent to the creator's
/// own wallet would be a no-op for "the new actor". The meaningful, spendable
/// wallet an actor controls is its **token-bound account (TBA)** — that's also
/// the x402 payee when one agent pays another (`proxy/api/mcp.ts` resolves
/// `tokenBoundAccountByName` → "payee (the agent's TBA)"). So prefund flows
/// CREATOR-wallet → new-name's TBA, giving the spawned actor operating funds it
/// controls. We batch `createTokenBoundAccount(tokenId)` first (idempotent) so
/// the counterfactual TBA exists to receive the transfer.
///
/// `creator` is the owner address paying / signing; `token_id` is the new
/// name's freshly-minted id; `name` is the (sanitised) subdomain.
async fn build_actor_setup(
    creator: &str,
    token_id: u64,
    name: &str,
    persona: Option<&str>,
    prefund_lh: Option<&str>,
) -> Result<ActorSetup, crate::error::Error> {
    let registry_addr =
        parse_address(super::registry::REGISTRY_ADDRESS).map_err(crate::error::Error::other)?;
    let mut calls: Vec<crate::tempo_tx::TempoCall> = Vec::new();
    let mut extra_gas: u128 = 0;
    let mut persona_set = false;
    let mut prefunded_lh = None;
    let mut tba_out = None;

    // PERSONA — publish the new subdomain's on-chain system prompt under the
    // persona metadata key (keccak256("localharness.persona")), the same slot
    // the CLI `persona` cmd + headless `call` read. setMetadata is gas-hungry
    // (~8.5k/byte; see CLAUDE.md), so scale the budget by length.
    if let Some(p) = persona {
        let p = p.trim();
        if !p.is_empty() {
            calls.push(crate::tempo_tx::TempoCall {
                to: registry_addr,
                value_wei: 0,
                input: super::registry::encode_set_persona(token_id, p),
            });
            extra_gas += 1_200_000 + (p.len() as u128) * 8_500;
            persona_set = true;
        }
    }

    // PREFUND — move `$LH` from the CREATOR to the new name's TBA. Validate the
    // creator actually holds the amount first (clear error, before any write).
    if let Some(amt_str) = prefund_lh {
        let amt_str = amt_str.trim();
        if !amt_str.is_empty() {
            let amount_wei = crate::encoding::parse_token_amount(amt_str).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse prefund_lh \"{amt_str}\" — pass a decimal $LH figure \
                     like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei > 0 {
                // Balance gate: refuse if the creator can't cover it.
                let bal = super::registry::token_balance_of(creator)
                    .await
                    .map_err(crate::error::Error::other)?;
                if bal < amount_wei {
                    return Err(crate::error::Error::other(format!(
                        "insufficient $LH to prefund: need {amt_str}, creator holds \
                         {} wei — redeem a code or lower prefund_lh",
                        bal
                    )));
                }
                // Resolve the new name's TBA (counterfactual address). We batch
                // createTokenBoundAccount(tokenId) FIRST so it's deployed to
                // receive funds (idempotent if already deployed).
                let tba = super::registry::tba_of_name(name)
                    .await
                    .map_err(crate::error::Error::other)?
                    .ok_or_else(|| {
                        crate::error::Error::other(
                            "could not resolve the new subdomain's token-bound account \
                             (TBA) to prefund — retry shortly",
                        )
                    })?;
                let tba_bytes = parse_address(&tba).map_err(crate::error::Error::other)?;
                let token_addr =
                    parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS)
                        .map_err(crate::error::Error::other)?;
                // 1) deploy the TBA (on the registry diamond)
                calls.push(crate::tempo_tx::TempoCall {
                    to: registry_addr,
                    value_wei: 0,
                    input: create_tba_calldata(token_id),
                });
                // 2) ERC-20 transfer creator → TBA (on the $LH token)
                calls.push(crate::tempo_tx::TempoCall {
                    to: token_addr,
                    value_wei: 0,
                    input: lh_transfer_calldata(&tba_bytes, amount_wei),
                });
                // TBA deploy (~mint-class cold SSTOREs) + ERC-20 transfer.
                extra_gas += 1_500_000 + 500_000;
                prefunded_lh = Some(amt_str.to_string());
                tba_out = Some(tba);
            }
        }
    }

    let _ = creator; // (used above only when prefunding)
    Ok(ActorSetup {
        calls,
        extra_gas,
        prefunded_lh,
        tba: tba_out,
        persona_set,
    })
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
            },
            "persona": {
                "type": "string",
                "description": "OPTIONAL system instruction / persona for the new \
                    agent — published on-chain as its system prompt (the persona \
                    that headless `call`s and the public face read). Omit to leave \
                    the default."
            },
            "prefund_lh": {
                "type": "string",
                "description": "OPTIONAL amount of $LH to prefund the new agent with, \
                    as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                    wallet to the new subdomain's token-bound account (its own \
                    spendable wallet — used to pay other agents via x402). Omit, or \
                    pass \"0\", to skip. Must not exceed your $LH balance."
            }
        },
        "required": ["name"]
    });
    ClosureTool::new(
        "create_subdomain",
        "Register a new <name>.localharness.xyz subdomain on-chain (the ACTOR MODEL). \
         The owner's master wallet pays gas and ends up holding the resulting ERC-721 \
         NFT. OPTIONALLY spawn the actor WITH behavior + funds in one call: `persona` \
         publishes its on-chain system instruction; `prefund_lh` moves that much $LH \
         from your wallet into the new agent's token-bound account (its own wallet). \
         Returns { name, url, owner, tx_hash, persona_set?, prefunded_lh?, tba? }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let persona = args.get("persona").and_then(|v| v.as_str());
            let prefund_lh = args.get("prefund_lh").and_then(|v| v.as_str());
            let cleaned = super::tenant::sanitize(name);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return Err(crate::error::Error::other("invalid name"));
            }
            // Register the name first (master wallet ends up holding the new id).
            let (owner, claim_tx) = super::verify::claim_name_via_iframe(&cleaned)
                .await
                .map_err(|e| crate::error::Error::other(format!("claim failed: {e}")))?;
            // Proactively push this device's Gemini key to the MAIN slot so the
            // new subdomain inherits it (no re-save).
            {
                let n = cleaned.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    super::events::sync_local_key_to_main(&n).await;
                });
            }

            // Optional ACTOR-MODEL extras: persona + prefund. Only if asked.
            let want_persona = persona.map(|p| !p.trim().is_empty()).unwrap_or(false);
            let want_prefund = prefund_lh
                .map(|p| {
                    let t = p.trim();
                    !t.is_empty() && t != "0"
                })
                .unwrap_or(false);
            let mut result = serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "owner": owner,
                "tx_hash": claim_tx,
            });
            if want_persona || want_prefund {
                // Resolve the freshly-minted tokenId for the metadata/TBA ops.
                let token_id = match super::registry::id_of_name(&cleaned).await {
                    Ok(id) if id != 0 => id,
                    Ok(_) => {
                        return Err(crate::error::Error::other(
                            "registered but tokenId not yet visible on-chain — retry \
                             persona/prefund shortly",
                        ))
                    }
                    Err(e) => return Err(crate::error::Error::other(format!("id_of_name: {e}"))),
                };
                let setup = build_actor_setup(
                    &owner,
                    token_id,
                    &cleaned,
                    persona,
                    prefund_lh,
                )
                .await?;
                if !setup.calls.is_empty() {
                    let tx_hash = super::events::run_sponsored_tempo_call(
                        &owner,
                        setup.calls,
                        setup.extra_gas,
                        "spawn actor (persona + prefund)",
                    )
                    .await
                    .map_err(|e| {
                        crate::error::Error::other(format!("actor setup failed: {e}"))
                    })?;
                    result["setup_tx_hash"] = serde_json::json!(tx_hash);
                    result["persona_set"] = serde_json::json!(setup.persona_set);
                    if let Some(amt) = setup.prefunded_lh {
                        result["prefunded_lh"] = serde_json::json!(amt);
                    }
                    if let Some(tba) = setup.tba {
                        result["tba"] = serde_json::json!(tba);
                    }
                }
            }
            Ok(result)
        },
    )
}

/// `create_and_publish_app(name, source)` — one-shot: register
/// `<name>.localharness.xyz` AND publish a compiled rustlite cartridge as
/// its public face, so "make me a clock subdomain" works in a single tool
/// call. Compiles `source` first (so a bad cartridge fails before the
/// on-chain register), claims the name via the iframe (master wallet ends
/// up holding the new tokenId), resolves the tokenId, then publishes via a
/// SPONSORED setMetadata batch (app.wasm bytes + public_face="app") in ONE
/// Tempo tx — exactly like the admin publish-app flow. Returns
/// `{ name, url, tx_hash }`.
fn create_and_publish_app_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to register, e.g. \"clock\" \
                    becomes clock.localharness.xyz. 3-32 chars; lowercase \
                    letters, digits, and hyphens only."
            },
            "source": {
                "type": "string",
                "description": "rustlite cartridge source — the SAME dialect as \
                    run_cartridge. Exports `fn frame(t: i32)` (animated) or \
                    `fn render()` and draws via `use host::display;`. This becomes \
                    the subdomain's fullscreen public face."
            },
            "persona": {
                "type": "string",
                "description": "OPTIONAL system instruction / persona for the new \
                    agent — published on-chain as its system prompt (read by \
                    headless `call`s). Omit to leave the default."
            },
            "prefund_lh": {
                "type": "string",
                "description": "OPTIONAL amount of $LH to prefund the new agent with, \
                    as a decimal string (\"5\", \"1.5\"). Transferred from YOUR \
                    wallet to the new subdomain's token-bound account (its own \
                    spendable wallet). Omit, or pass \"0\", to skip. Must not exceed \
                    your $LH balance."
            }
        },
        "required": ["name", "source"]
    });
    ClosureTool::new(
        "create_and_publish_app",
        "One-shot: register a new <name>.localharness.xyz AND publish a compiled \
         rustlite cartridge as its fullscreen public face, in a single call (compile \
         + on-chain register + sponsored setMetadata publish). Use this for \"make me \
         a clock/<app> subdomain\". The ACTOR MODEL: optionally also set the new \
         agent's `persona` (on-chain system instruction) and `prefund_lh` it with $LH \
         (into its token-bound account), all in the SAME sponsored tx. create_subdomain \
         remains for registering a name-only subdomain. Returns { name, url, tx_hash, \
         persona_set?, prefunded_lh?, tba? }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim();
            let source = args.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let persona = args.get("persona").and_then(|v| v.as_str());
            let prefund_lh = args.get("prefund_lh").and_then(|v| v.as_str());
            let cleaned = super::tenant::sanitize(name);
            if cleaned.len() < 3 || cleaned.len() > 32 {
                return Err(crate::error::Error::other("invalid name"));
            }
            if source.trim().is_empty() {
                return Err(crate::error::Error::other("source cannot be empty"));
            }
            // Compile FIRST so a bad cartridge fails before we register the
            // name on-chain. Surface a clear error so the agent reports it.
            let wasm = crate::rustlite::compile(source)
                .map_err(|e| crate::error::Error::other(format!("compile failed: {e}")))?;
            if wasm.len() > 16_384 {
                return Err(crate::error::Error::other(format!(
                    "app wasm too large to publish: {} bytes (max 16384)",
                    wasm.len()
                )));
            }
            // Register the name. The owner's master wallet ends up holding
            // the new tokenId, so it's authorized to setMetadata below.
            let (owner, _claim_tx) = super::verify::claim_name_via_iframe(&cleaned)
                .await
                .map_err(|e| crate::error::Error::other(format!("claim failed: {e}")))?;
            // Inherit this device's Gemini key onto the new subdomain.
            {
                let n = cleaned.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    super::events::sync_local_key_to_main(&n).await;
                });
            }
            // Resolve the freshly-minted tokenId.
            let token_id = match super::registry::id_of_name(&cleaned).await {
                Ok(id) if id != 0 => id,
                Ok(_) => {
                    return Err(crate::error::Error::other(
                        "registered but tokenId not yet visible on-chain — retry publish shortly",
                    ))
                }
                Err(e) => return Err(crate::error::Error::other(format!("id_of_name: {e}"))),
            };
            // Publish: app wasm bytes + public_face="app" in ONE sponsored
            // Tempo tx (two setMetadata calls), exactly like the admin
            // publish-app flow. Owner signs the sender_hash via the apex
            // iframe; the sponsor pays gas.
            let registry_addr = parse_address(super::registry::REGISTRY_ADDRESS)
                .map_err(crate::error::Error::other)?;
            let mk = |input: Vec<u8>| crate::tempo_tx::TempoCall {
                to: registry_addr,
                value_wei: 0,
                input,
            };
            let mut calls = vec![
                mk(super::registry::encode_set_app_wasm(token_id, &wasm)),
                mk(super::registry::encode_set_public_face(token_id, "app")),
            ];
            let words = (wasm.len() / 32 + 1) as u128;
            let mut gas = 1_300_000 + words * 40_000;
            // ACTOR MODEL: fold optional persona + prefund into the SAME tx.
            let setup =
                build_actor_setup(&owner, token_id, &cleaned, persona, prefund_lh).await?;
            calls.extend(setup.calls);
            gas += setup.extra_gas;
            let tx_hash = super::events::run_sponsored_tempo_call(
                &owner,
                calls,
                gas,
                "create + publish app",
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("publish failed: {e}")))?;
            let mut result = serde_json::json!({
                "name": cleaned,
                "url": format!("https://{cleaned}.localharness.xyz/"),
                "tx_hash": tx_hash,
            });
            if setup.persona_set {
                result["persona_set"] = serde_json::json!(true);
            }
            if let Some(amt) = setup.prefunded_lh {
                result["prefunded_lh"] = serde_json::json!(amt);
            }
            if let Some(tba) = setup.tba {
                result["tba"] = serde_json::json!(tba);
            }
            Ok(result)
        },
    )
}

/// `release_subdomain(name, confirmation)` — DESTRUCTIVE: burn the NFT +
/// free the name. Gated: `confirmation` must EXACTLY equal `name`, which
/// forces a typed confirmation in chat (the owner types the name). The
/// system prompt also forbids auto-filling it.
fn release_subdomain_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "Subdomain to release/recycle — burns the NFT, frees the name."
            },
            "confirmation": {
                "type": "string",
                "description": "Must EXACTLY equal `name`. Pass ONLY after the owner has \
                    TYPED the exact name in this chat. Never auto-fill or invent it."
            }
        },
        "required": ["name", "confirmation"]
    });
    ClosureTool::new(
        "release_subdomain",
        "DESTRUCTIVE + IRREVERSIBLE: burn a subdomain NFT and free its name. Requires \
         `confirmation` to exactly equal `name` (the owner must type the name). Refuses \
         your MAIN. Returns the tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            let confirmation = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Err(crate::error::Error::other("name is required"));
            }
            if confirmation != name {
                return Err(crate::error::Error::other(format!(
                    "release_subdomain NOT executed — confirmation must exactly equal \"{name}\". \
                     Ask the owner to TYPE \"{name}\" to confirm, then retry. Do not auto-fill it."
                )));
            }
            match super::events::run_release_subdomain(&name).await {
                Ok(tx) => Ok(serde_json::json!({ "released": name, "tx_hash": tx })),
                Err(e) => Err(crate::error::Error::other(format!("release failed: {e}"))),
            }
        },
    )
}

/// `bulk_release_subdomains(confirmation, names?)` — DESTRUCTIVE batch burn.
/// With no `names`, targets EVERY non-MAIN subdomain the owner holds; with
/// `names`, only that subset. Single master confirmation (NOT per-name): the
/// owner must type the literal phrase `release all non-main`. An empty/absent
/// confirmation returns the list it WOULD release (so the agent can show the
/// user first) and performs NO write. Refuses the MAIN. Withheld from
/// subagents (only registered on the main agent).
fn bulk_release_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "names": {
                "type": "array",
                "items": { "type": "string" },
                "description": "OPTIONAL subset of subdomain names to release in one \
                    batch. Omit to target EVERY non-MAIN subdomain the owner holds."
            },
            "confirmation": {
                "type": "string",
                "description": "Must EXACTLY equal `release all non-main`. Pass ONLY after \
                    the owner has TYPED that exact phrase in this chat. First call this \
                    tool with confirmation empty to GET the list of names that will be \
                    released, show the user, and ask them to type the phrase. Never \
                    auto-fill or invent it."
            }
        },
        "required": []
    });
    ClosureTool::new(
        "bulk_release_subdomains",
        "DESTRUCTIVE + IRREVERSIBLE: burn MANY subdomain NFTs and free their names in \
         ONE batch. With no `names`, releases EVERY non-MAIN subdomain the owner holds; \
         with `names`, only that subset. Requires a SINGLE master `confirmation` equal to \
         \"release all non-main\" (the owner types it once — NOT one confirmation per \
         name). ALWAYS call first with confirmation empty to receive the list of names it \
         will release, show the user, then ask them to type the phrase and retry. Always \
         refuses your MAIN. Returns the released names + tx hash.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            const CONFIRM_PHRASE: &str = "release all non-main";
            let confirmation = args
                .get("confirmation")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();

            // Resolve the kill-list: explicit subset, else all non-MAIN holdings.
            let tenant = match super::tenant::current() {
                super::tenant::Host::Tenant(n) => n,
                _ => return Err(crate::error::Error::other("not running on a subdomain")),
            };
            let owner = super::registry::owner_of_name(&tenant)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| crate::error::Error::other("no on-chain owner"))?;
            let main_id = super::registry::main_of(&owner)
                .await
                .map_err(crate::error::Error::other)?;

            let explicit: Vec<String> = args
                .get("names")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            let targets: Vec<String> = if explicit.is_empty() {
                let tokens = super::registry::list_owned_tokens(&owner)
                    .await
                    .map_err(crate::error::Error::other)?;
                tokens
                    .into_iter()
                    .filter(|t| main_id == 0 || t.token_id != main_id)
                    .map(|t| t.name)
                    .collect()
            } else {
                explicit
            };

            if targets.is_empty() {
                return Ok(serde_json::json!({
                    "status": "nothing_to_release",
                    "note": "no non-MAIN subdomains to release"
                }));
            }

            // REPORT-BEFORE-CONFIRM: no valid confirmation -> list + STOP.
            if confirmation != CONFIRM_PHRASE {
                return Ok(serde_json::json!({
                    "status": "confirmation_required",
                    "count": targets.len(),
                    "will_release": targets,
                    "instruction": format!(
                        "These {} subdomain(s) will be PERMANENTLY released (burned). \
                         Show this list to the owner. To proceed, the owner must TYPE the \
                         exact phrase \"{}\" — then call bulk_release_subdomains again with \
                         that confirmation. Do NOT auto-fill it.",
                        targets.len(), CONFIRM_PHRASE
                    )
                }));
            }

            match super::events::run_bulk_release(&targets).await {
                Ok((released, tx)) => Ok(serde_json::json!({
                    "released": released,
                    "count": released.len(),
                    "tx_hash": tx,
                })),
                Err(e) => Err(crate::error::Error::other(format!("bulk release failed: {e}"))),
            }
        },
    )
}

/// `batch_create_subdomains(names)` — register MANY subdomains in ONE
/// sponsored multi-call tx (the mirror of `bulk_release_subdomains`, but
/// ADDITIVE: NO destructive confirmation). The sanctioned mass-registration
/// path — one tx instead of an N-deep `create_subdomain` loop. Names are
/// sanitised + availability-checked; taken/invalid names are skipped and
/// reported. Capped at MAX_BATCH_CREATE to bound a confused model. Not
/// granted to subagents (same restraint as bulk_release).
fn batch_create_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "names": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Subdomain names to register in ONE tx, e.g. \
                    [\"alice\",\"bob\"] -> alice.localharness.xyz, \
                    bob.localharness.xyz. Each: 3-32 chars, lowercase letters, \
                    digits, hyphens. Already-taken or invalid names are skipped \
                    and reported back. Max 20 per call."
            }
        },
        "required": ["names"]
    });
    ClosureTool::new(
        "batch_create_subdomains",
        "Register MANY <name>.localharness.xyz subdomains on-chain in a SINGLE \
         sponsored transaction. PREFER THIS over calling create_subdomain in a \
         loop when registering more than one name — it is one tx, not N. The \
         owner's master wallet ends up holding every resulting ERC-721 NFT. \
         Taken or invalid names are skipped (not an error) and listed in \
         `skipped`. Max 20 names per call. Returns { registered, skipped, \
         count, tx_hash, urls }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            const MAX_BATCH_CREATE: usize = 20;
            let requested: Vec<String> = args
                .get("names")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            if requested.is_empty() {
                return Err(crate::error::Error::other("names cannot be empty"));
            }
            if requested.len() > MAX_BATCH_CREATE {
                return Err(crate::error::Error::other(format!(
                    "too many names: {} (max {MAX_BATCH_CREATE} per batch) — \
                     split into multiple calls",
                    requested.len()
                )));
            }
            match super::events::run_batch_create_subdomains(&requested).await {
                Ok((registered, tx)) => {
                    let skipped: Vec<&String> = requested
                        .iter()
                        .filter(|r| {
                            let c = super::tenant::sanitize(r);
                            !registered.iter().any(|reg| reg == &c)
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "registered": registered,
                        "skipped": skipped,
                        "count": registered.len(),
                        "tx_hash": tx,
                        "urls": registered.iter()
                            .map(|n| format!("https://{n}.localharness.xyz/"))
                            .collect::<Vec<_>>(),
                    }))
                }
                Err(e) => Err(crate::error::Error::other(format!(
                    "batch create failed: {e}"
                ))),
            }
        },
    )
}

/// `list_subdomains()` — enumerate every subdomain this agent's owner
/// holds (their identity's holdings). Read-only.
fn list_subdomains_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "list_subdomains",
        "List every subdomain owned by this agent's owner (their identity's holdings on \
         the registry). Read-only. Use when the user asks what subdomains/agents they have.",
        serde_json::json!({ "type": "object", "properties": {} }),
        |_args: serde_json::Value, _ctx| async move {
            let name = match super::tenant::current() {
                super::tenant::Host::Tenant(n) => n,
                _ => return Err(crate::error::Error::other("not running on a subdomain")),
            };
            let owner = super::registry::owner_of_name(&name)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| crate::error::Error::other("no on-chain owner"))?;
            let tokens = super::registry::list_owned_tokens(&owner)
                .await
                .map_err(crate::error::Error::other)?;
            let subdomains: Vec<_> = tokens
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "url": format!("https://{}.localharness.xyz/", t.name),
                        "token_id": t.token_id,
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "owner": owner,
                "count": subdomains.len(),
                "subdomains": subdomains,
            }))
        },
    )
}

/// `discover_agents(query)` — find peer agents by capability/persona. The
/// browser twin of the `localharness discover` CLI command: a read-only
/// registry scan (no `$LH`, no tx) that reuses [`registry::discover_agents`]
/// (which ranks `(name, persona)` matches — name hits above persona hits). The
/// agent uses it to LOCATE a peer to delegate to, then `call_agent`s it.
/// Returns `{ agents: [{ name, persona }], count }`; persona snippets are
/// truncated to a char-safe ~160-char preview. Safe to grant broadly.
fn discover_agents_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    /// Char-safe truncation of a persona to a short preview (never splits a
    /// UTF-8 codepoint; appends an ellipsis when clipped).
    fn snippet(persona: &str) -> String {
        const MAX: usize = 160;
        let trimmed = persona.trim();
        if trimmed.chars().count() <= MAX {
            return trimmed.to_string();
        }
        let mut s: String = trimmed.chars().take(MAX).collect();
        s.push('…');
        s
    }
    ClosureTool::new(
        "discover_agents",
        "Find peer agents by capability or persona. Read-only registry scan: \
         returns the agents whose subdomain NAME or on-chain persona matches \
         `query` (ranked — name matches first, then persona matches). Use this \
         to LOCATE an agent to delegate to, then call_agent it. Returns \
         { agents: [ { name, persona } ], count } (persona is a short preview).",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "What to look for — a capability, topic, or \
                        keyword matched (case-insensitively) against agent names \
                        and personas (e.g. \"solidity\", \"image\", \"research\"). \
                        Empty returns recent agents."
                }
            },
            "required": ["query"]
        }),
        |args: serde_json::Value, _ctx| async move {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Reuse the registry's ranked discovery (same core as the
            // `localharness discover` CLI). 100 = how many recent agents to scan.
            let matches = super::registry::discover_agents(&query, 100)
                .await
                .map_err(crate::error::Error::other)?;
            let agents: Vec<_> = matches
                .iter()
                .map(|(name, persona)| {
                    serde_json::json!({
                        "name": name,
                        "persona": snippet(persona),
                    })
                })
                .collect();
            Ok(serde_json::json!({
                "count": agents.len(),
                "agents": agents,
            }))
        },
    )
}

/// `send_lh(recipient, amount)` — transfer real `$LH` credits from the owner's
/// wallet. `recipient` is either a raw `0x…` address or a subdomain name (whose
/// on-chain OWNER address receives the funds). `amount` is a human-typed `$LH`
/// figure (18-decimal token; "5", "1.5", "0.000001"). Builds an ERC-20
/// `transfer(to, amount_wei)` against the `$LH` token and routes it through the
/// SAME sponsored Tempo path as the per-turn payment + the "act" panel
/// (`run_sponsored_tempo_call`): the owner's apex wallet signs the intent, the
/// bundle sponsor pays gas in AlphaUSD. NOT granted to subagents (it moves
/// value). No typed-confirmation gate — a transfer is an intended action, unlike
/// the destructive `release_subdomain` burn — but the amount must parse to > 0.
fn send_lh_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "recipient": {
                "type": "string",
                "description": "Who receives the $LH: either a raw 0x… 20-byte \
                    address, or a subdomain name like \"alice\" (the funds go to \
                    that subdomain's on-chain OWNER address)."
            },
            "amount": {
                "type": "string",
                "description": "Amount of $LH to send, as a decimal string \
                    (e.g. \"5\", \"1.5\", \"0.01\"). Must be greater than 0."
            }
        },
        "required": ["recipient", "amount"]
    });
    ClosureTool::new(
        "send_lh",
        "Transfer real $LH credits from the owner's wallet to a recipient. \
         `recipient` is a raw 0x… address OR a subdomain name (funds go to that \
         name's on-chain owner). `amount` is a decimal $LH figure (must be > 0). \
         Moves value: confirm the recipient + amount with the owner before \
         calling. Returns { amount, recipient (input), resolved_recipient, \
         tx_hash }.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            use crate::encoding::{parse_token_amount, Recipient};

            let recipient_arg = args
                .get("recipient")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let amount_arg = args
                .get("amount")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            // Amount: parse to 18-decimal wei (same units as the act panel /
            // per-turn payment), reject zero / garbage.
            let amount_wei = parse_token_amount(&amount_arg).ok_or_else(|| {
                crate::error::Error::other(format!(
                    "could not parse amount \"{amount_arg}\" — pass a decimal $LH \
                     figure like \"5\" or \"1.5\""
                ))
            })?;
            if amount_wei == 0 {
                return Err(crate::error::Error::other(
                    "amount must be greater than 0",
                ));
            }

            // Recipient: address used directly; name → on-chain owner address.
            let kind = crate::encoding::classify_recipient(&recipient_arg)
                .map_err(crate::error::Error::other)?;
            let to_hex = match kind {
                Recipient::Address(addr) => addr,
                Recipient::Name(name) => super::registry::owner_of_name(&name)
                    .await
                    .map_err(crate::error::Error::other)?
                    .ok_or_else(|| {
                        crate::error::Error::other(format!(
                            "no on-chain owner for subdomain \"{name}\" — is it registered?"
                        ))
                    })?,
            };

            // Sender = this subdomain's on-chain owner (the apex wallet that
            // signs via the iframe), matching list_subdomains / bulk_release.
            let tenant = match super::tenant::current() {
                super::tenant::Host::Tenant(n) => n,
                _ => {
                    return Err(crate::error::Error::other(
                        "not running on a subdomain — no owner wallet to send from",
                    ))
                }
            };
            let from = super::registry::owner_of_name(&tenant)
                .await
                .map_err(crate::error::Error::other)?
                .ok_or_else(|| crate::error::Error::other("no on-chain owner"))?;

            // ERC-20 transfer(to, amount_wei) against the $LH token — the exact
            // calldata shape the per-turn payment + act panel build.
            let to_bytes = crate::encoding::parse_address(&to_hex)
                .map_err(crate::error::Error::other)?;
            let mut to_padded = [0u8; 32];
            to_padded[12..].copy_from_slice(&to_bytes);
            let mut calldata = Vec::with_capacity(4 + 32 + 32);
            calldata.extend_from_slice(&transfer_selector());
            calldata.extend_from_slice(&to_padded);
            calldata.extend_from_slice(&u256_be(amount_wei));

            let token_addr =
                crate::encoding::parse_address(crate::registry::LOCALHARNESS_TOKEN_ADDRESS)
                    .map_err(crate::error::Error::other)?;
            let call = crate::tempo_tx::TempoCall {
                to: token_addr,
                value_wei: 0,
                input: calldata,
            };

            let amount_display = amount_arg.clone();
            let purpose = format!("send {amount_display} $LH to {to_hex}");
            // 500k mirrors the per-turn payment's ERC-20 transfer budget; the
            // sponsor is billed on gas USED, not the limit.
            let tx_hash = super::events::run_sponsored_tempo_call(
                &from,
                vec![call],
                500_000,
                &purpose,
            )
            .await
            .map_err(|e| crate::error::Error::other(format!("send_lh failed: {e}")))?;

            Ok(serde_json::json!({
                "amount": amount_display,
                "recipient": recipient_arg,
                "resolved_recipient": to_hex,
                "tx_hash": tx_hash,
            }))
        },
    )
}

/// `clear_context()` — erase the entire conversation history and the visible
/// chat, starting a fresh empty context. Deferred: sets `PENDING_CLEAR`,
/// drained post-turn in [`run_send`] (clearing mid-turn would corrupt the
/// in-flight turn this tool runs inside). Withheld from subagents — a
/// detached subagent must never wipe the main tab's chat.
fn clear_context_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "clear_context",
        "Erase the ENTIRE conversation history and clear the visible chat, starting a \
         brand-new empty context. Use when the user asks to clear, reset, wipe, or start a \
         fresh chat/context. Irreversible. The screen clears the moment this turn ends.",
        serde_json::json!({ "type": "object", "properties": {} }),
        |_args: serde_json::Value, _ctx| async move {
            PENDING_CLEAR.with(|c| c.set(true));
            Ok(serde_json::json!({
                "status": "scheduled",
                "note": "the conversation will be cleared as soon as this turn ends"
            }))
        },
    )
}

/// `compact_context()` — summarise older turns into a short note while
/// keeping recent turns verbatim, freeing context-window budget. Deferred
/// like [`clear_context_tool`]; the post-turn drain also collapses the
/// visible scrollback to mirror the compacted state.
fn compact_context_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    ClosureTool::new(
        "compact_context",
        "Compact the conversation: summarise older messages into a short note while keeping \
         the most recent turns verbatim, freeing context-window budget. Use when the user \
         asks to compact, summarise, condense, or shrink the context. Takes effect the \
         moment this turn ends; the visible chat collapses to match.",
        serde_json::json!({ "type": "object", "properties": {} }),
        |_args: serde_json::Value, _ctx| async move {
            PENDING_COMPACT.with(|c| c.set(true));
            Ok(serde_json::json!({
                "status": "scheduled",
                "note": "the context will be compacted as soon as this turn ends"
            }))
        },
    )
}

/// `submit_feedback(text)` — submit feedback on-chain via the FeedbackFacet.
fn submit_feedback_tool() -> std::sync::Arc<dyn crate::tools::Tool> {
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "text": {
                "type": "string",
                "description": "Feedback text to submit on-chain. Keep it short — a \
                    few sentences, under ~2000 bytes. Summarize rather than pasting a \
                    long multi-paragraph report. Hard cap is 2048 bytes; longer text \
                    is rejected before the on-chain tx."
            }
        },
        "required": ["text"]
    });
    ClosureTool::new(
        "submit_feedback",
        "Submit feedback on-chain via the FeedbackFacet on the localharness registry. \
         Emits a FeedbackSubmitted event. Use this when the user asks to leave feedback \
         or when you want to report an issue about another agent.",
        schema,
        |args: serde_json::Value, _ctx| async move {
            let text = args.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if text.is_empty() {
                return Err(crate::error::Error::other("feedback text cannot be empty"));
            }
            if text.len() > 2048 {
                return Err(crate::error::Error::other(format!(
                    "feedback too long: {} bytes (max 2048) — please shorten",
                    text.len()
                )));
            }
            let from_hex = super::APP.with(|cell| {
                use super::VerifyState;
                match &cell.borrow().verify_state {
                    VerifyState::Verified { address } => Some(address.clone()),
                    VerifyState::Visitor { visitor_address, .. } => Some(visitor_address.clone()),
                    _ => cell.borrow().wallet.as_ref().map(|w| w.address_hex()),
                }
            });
            let from_hex = from_hex.ok_or_else(|| {
                crate::error::Error::other("no identity — claim a subdomain first")
            })?;
            match super::feedback::submit_feedback_onchain(&from_hex, text).await {
                Ok(tx_hash) => Ok(serde_json::json!({
                    "status": "submitted",
                    "tx_hash": tx_hash,
                })),
                Err(e) => Err(crate::error::Error::other(format!("feedback failed: {e}"))),
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
fn spawn_recursive_subagent_tool(
    api_key: String,
    base_url: Option<url::Url>,
) -> std::sync::Arc<dyn crate::tools::Tool> {
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
            let base_url = base_url.clone();
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
                let mut cfg = GeminiAgentConfig::new(api_key.clone())
                    .with_capabilities(CapabilitiesConfig::unrestricted())
                    .with_policies(vec![policy::allow_all()])
                    .with_filesystem(super::shared_opfs())
                    .with_system_instructions(system.to_string())
                    .with_tool(create_subdomain_tool())
                    .with_tool(create_and_publish_app_tool())
                    .with_tool(spawn_recursive_subagent_tool(api_key.clone(), base_url.clone()));
                // Credits mode: subagents reach Gemini through the same proxy.
                if let Some(b) = &base_url {
                    cfg = cfg.with_base_url(b.clone());
                }
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

#[cfg(test)]
mod tests {
    use super::{classify_turn, TurnOutcome, MAX_AUTO_CONTINUATIONS};

    // --- Turn classification (the continuous-execution loop's decision) -----

    #[test]
    fn finish_wins_over_everything() {
        // finish + a goal-step tool in the same turn is still "done".
        assert_eq!(
            classify_turn(true, false, true, true),
            TurnOutcome::Finished
        );
        // finish alone.
        assert_eq!(
            classify_turn(true, false, false, true),
            TurnOutcome::Finished
        );
        // finish even alongside a question.
        assert_eq!(
            classify_turn(true, true, true, true),
            TurnOutcome::Finished
        );
    }

    #[test]
    fn ask_question_stops_the_loop_not_incomplete() {
        // REGRESSION: a blocking ask_question used to be read as a goal step
        // (saw_tool_call) → Incomplete → auto-continue, spamming the model and
        // never letting the user answer. It must stop like a FinalAnswer.
        assert_eq!(
            classify_turn(false, true, false, true),
            TurnOutcome::FinalAnswer
        );
        // A question accompanied by some other goal-step tool still stops:
        // the question is the blocking signal.
        assert_eq!(
            classify_turn(false, true, true, true),
            TurnOutcome::FinalAnswer
        );
    }

    #[test]
    fn goal_step_tool_only_auto_continues() {
        assert_eq!(
            classify_turn(false, false, true, true),
            TurnOutcome::Incomplete
        );
    }

    #[test]
    fn pure_text_reply_is_final_answer() {
        assert_eq!(
            classify_turn(false, false, false, true),
            TurnOutcome::FinalAnswer
        );
    }

    #[test]
    fn nothing_visible_is_empty() {
        assert_eq!(
            classify_turn(false, false, false, false),
            TurnOutcome::Empty
        );
        // No-visible takes precedence over a stray tool flag (can't have run a
        // tool with nothing rendered, but the ordering must be deterministic).
        assert_eq!(
            classify_turn(false, false, true, false),
            TurnOutcome::Empty
        );
    }

    /// The only outcome that auto-continues is `Incomplete`. This guards the
    /// loop-termination invariant: every other classification breaks the
    /// continuous-execution loop, so the loop can only spin via `Incomplete`,
    /// which is hard-bounded by `MAX_AUTO_CONTINUATIONS`.
    #[test]
    fn only_incomplete_continues() {
        let continues = |o: TurnOutcome| o == TurnOutcome::Incomplete;
        assert!(!continues(classify_turn(true, false, false, true))); // Finished
        assert!(!continues(classify_turn(false, true, false, true))); // FinalAnswer (question)
        assert!(!continues(classify_turn(false, false, false, true))); // FinalAnswer (text)
        assert!(!continues(classify_turn(false, false, false, false))); // Empty
        assert!(continues(classify_turn(false, false, true, true))); // Incomplete
    }

    /// Mirrors the loop's increment/break: an always-`Incomplete` turn can fire
    /// at most `MAX_AUTO_CONTINUATIONS` auto-continuations, then the cap stops
    /// it. Proves no infinite loop when a confused model never finishes.
    #[test]
    fn auto_continuation_is_bounded() {
        let mut auto: u32 = 0;
        let mut iterations = 0u32;
        loop {
            iterations += 1;
            // Always Incomplete (the worst case for the loop).
            if matches!(classify_turn(false, false, true, true), TurnOutcome::Incomplete) {
                if auto >= MAX_AUTO_CONTINUATIONS {
                    break;
                }
                auto += 1;
            } else {
                break;
            }
            // Safety net for the test itself.
            assert!(iterations < MAX_AUTO_CONTINUATIONS + 5, "loop did not terminate");
        }
        // First turn + MAX_AUTO_CONTINUATIONS continuations, then the cap break.
        assert_eq!(auto, MAX_AUTO_CONTINUATIONS);
        assert_eq!(iterations, MAX_AUTO_CONTINUATIONS + 1);
    }
}
