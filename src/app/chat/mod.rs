//! Chat-turn orchestration. Driven by the `send` action in
//! [`super::events`]; entirely HTMX-style — every UI mutation is a
//! `swap_inner` / `append_html` on a targeted `id=`. We never walk the
//! DOM looking for nodes; element identity is established up-front via
//! ids we allocate and templates we render.

use std::collections::VecDeque;

use futures_util::StreamExt;
use maud::html;
use wasm_bindgen::JsCast;
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
mod confirm_guard;
mod dedup;
mod plan_state;
mod prompt;
mod router_wire;
mod session;
mod stage;
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
    /// `Date::now()` at the start of the user's send — the origin for the phase
    /// benchmark (see `run_send` / `stream_turn`). Lets `stream_turn` report
    /// time-to-first-output without threading a timestamp through every call.
    static TURN_T0: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
    /// One-shot guard so the TTFT line logs once per turn (the stream loop runs
    /// many iterations).
    static TTFT_LOGGED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Emit a phase-benchmark line to BOTH the console (desktop devtools) and the
/// `?debug=1` breadcrumb overlay (mobile, which has no console) — so the
/// "why is `starting` slow" question gets real per-phase numbers. Cheap
/// (one `Date::now()` + a format) and always on; the line is prefixed `perf:`.
fn perf_log(msg: &str) {
    let line = format!("perf: {msg}");
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(&line));
    crate::app::debuglog::log(&line);
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
/// The engine observes the flag within its cancel poll slice (~100ms — see
/// `backends::stream_timeout::CANCEL_POLL_MS`) and drops the in-flight HTTP
/// response; only a tool call already mid-execution runs to its end. The FIRST
/// click also paints an optimistic acknowledgment (telemetry #33: the button
/// looked dead, so users hammered it): the pulsing stage cue stops and the
/// status line reads "stopping…" immediately; the transcript's "Stopped." note
/// reconciles when the turn actually ends (`TurnGuard` restores the button).
pub(crate) fn request_stop_turn() {
    let already_requested = TURN_CANCEL.with(|c| c.replace(true));
    if let Some(agent) = APP.with(|cell| cell.borrow().agent.clone()) {
        agent.cancel_turn();
    }
    if already_requested || !TURN_ACTIVE.with(|c| c.get()) {
        return;
    }
    stage::end();
    dom::set_status(STOPPING_STATUS, false);
}

/// The optimistic status painted on the FIRST stop click; `TurnGuard` clears
/// it (by exact match, so real errors are never wiped) when the turn ends.
const STOPPING_STATUS: &str = "stopping…";

/// Whether the running turn has been asked to stop (the stop button). Tools
/// that wait (e.g. `dwell`) poll this between chunks so Stop interrupts them
/// mid-call instead of having to run to completion (on-chain feedback).
pub(crate) fn turn_cancelled() -> bool {
    TURN_CANCEL.with(|c| c.get())
}

/// RAII cleanup for a turn: clears the active/cancel flags and restores
/// the send button (if the stop button is currently shown) on every
/// exit path, including early returns and future cancellation.
struct TurnGuard;
impl Drop for TurnGuard {
    fn drop(&mut self) {
        TURN_ACTIVE.with(|c| c.set(false));
        TURN_CANCEL.with(|c| c.set(false));
        // Never leave a stage line pulsing after the run — every exit path
        // (including panics-as-aborts aside, early returns) clears it.
        stage::end();
        if dom::by_id("terminal-stop").is_some() {
            dom::swap_outer("terminal-stop", &templates::send_button().into_string());
        }
        // Reconcile the optimistic stop acknowledgment: clear the status line
        // ONLY if it still reads the exact "stopping…" request_stop_turn
        // painted — the transcript's "Stopped." note supersedes it, and a real
        // error status set during the turn must survive.
        if let Some(el) = dom::by_id("system-status") {
            if el.text_content().as_deref() == Some(STOPPING_STATUS) {
                dom::set_status("", false);
            }
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
    // Cheap empty-input reject FIRST — a blank/whitespace-only send must be a
    // true zero-cost no-op, so it runs BEFORE resolve_credit_access (which mints
    // a proxy-auth token) and ensure_credit_meter (which can fire a sponsored
    // deposit tx). Silent — no explanatory validation text. (on-chain feedback)
    let prompt = prompt_area.value().trim().to_string();
    if prompt.is_empty() {
        return;
    }

    // INTENT ROUTER (`crate::router` pure core + `router_wire` painting): route
    // the OBVIOUS zero-model intents (balance/credits reads, open-files/display/
    // terminal commands, a tiny docs FAQ) to a FREE local answer BEFORE any
    // metered work — every message that reaches the proxy costs ~1 $LH, and
    // "show my balance" never needed a model. Runs BEFORE resolve_credit_access
    // (which mints an auth token / pops the key modal) so a free turn is a true
    // zero-cost path. CONSERVATIVE by contract: only exact allowlist phrasings
    // route free; everything else passes through untouched. A leading '!'
    // always forces the model (stripped here); the gate is ON by default
    // (tab-E2E'd), '/router off' opts the session out.
    let prompt = match router_wire::pre_route(&prompt).await {
        router_wire::PreRoute::Handled => {
            prompt_area.set_value("");
            return;
        }
        router_wire::PreRoute::SendToModel(p) => p,
    };
    if prompt.is_empty() {
        // A bare "!" force-prefix with nothing behind it — nothing to send.
        return;
    }

    // ── phase benchmark (perf:) ── t0 is the origin for every phase delta; the
    // TTFT line in `stream_turn` reads it back. Resets the one-shot TTFT guard.
    let t0 = js_sys::Date::now();
    TURN_T0.with(|c| c.set(t0));
    TTFT_LOGGED.with(|c| c.set(false));

    let access = match resolve_credit_access().await {
        Some(a) => a,
        None => {
            super::show_api_key_modal();
            return;
        }
    };
    let t_credit = js_sys::Date::now();
    // Credits mode (base_url set): top up the per-request meter from any WALLET
    // $LH — but in the BACKGROUND. This was `.await`ed on every turn, adding a
    // `token_balance_of` RPC round-trip to the "starting" phase for nothing: the
    // common case is a meter-funded user whose wallet is 0, so the read just
    // returns. The bridge itself only fires a TX when the wallet actually holds
    // $LH, and ONE deposit drains it to 0 — so it is at most once per funding
    // event, NEVER per turn (and redeem/buy already bridge synchronously on their
    // own paths). Fire-and-forget so the turn doesn't wait on the read.
    if access.base_url.is_some() {
        wasm_bindgen_futures::spawn_local(ensure_credit_meter());
    }
    let key = access.cfg_auth;

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

    // A fresh send wipes any stale status-line error (feedback #64: errors
    // never cleared without a reload). Real errors land in the transcript;
    // the status line is a transient mirror only.
    dom::set_status("", false);

    // Paint the user bubble + the PENDING assistant turn up front, BEFORE
    // the payment gate / session boot, so the pre-model phases show as a
    // stage line INSIDE the stream instead of an opaque pause (GitHub #19).
    // `stream_turn` reuses these ids for the first turn.
    let (user_turn_id, assistant_turn_id, first_seg_id) = APP.with(|cell| {
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
            html! {
                (templates::text_segment(first_seg_id, ""))
            },
            true,
        )
        .into_string(),
    );
    dom::scroll_to_bottom("transcript");
    stage::begin(&format!("turn-body-{assistant_turn_id}"));

    // Clear the prompt; the value is already captured above.
    prompt_area.set_value("");
    // Collapse the auto-grown height back to one row (the `input` listener only
    // fires on typing, so an empty value would otherwise keep the grown height).
    // The textarea is content-sized and the parent `.terminal-row` carries the
    // 38px-snapped height (events::autogrow_textarea) — reset BOTH so a cleared
    // multi-line input snaps back to a single resting box.
    let _ = prompt_area.style().set_property("height", "auto");
    if let Some(row) = prompt_area
        .parent_element()
        .and_then(|p| p.dyn_into::<web_sys::HtmlElement>().ok())
    {
        let _ = row.style().remove_property("height");
    }
    // Close the keyboard on send (on-chain #55): blur the input so the mobile
    // soft keyboard collapses — covers BOTH send paths (button + Enter), both of
    // which dispatch Action::Send → run_send. (Was `.focus()`, which kept the
    // keyboard up over the streaming reply on mobile.)
    dom::blur_prompt();

    // Swap the send arrow for the stop button for the whole (possibly
    // multi-turn) run; the guard / loop-end restores it.
    dom::swap_outer("terminal-send", &templates::stop_button().into_string());

    // Payment gate. If the agent's owner has set a per-turn price AND
    // we know this visitor is *not* the owner, collect payment via
    // the cross-origin iframe signer before the LLM call runs.
    // Owner-of-the-agent always sends free; verification-pending /
    // unregistered / failed states fall through without charging.
    // (The "paying" stage is entered inside the gate, only when it
    // actually collects.)
    match collect_payment_if_required().await {
        Ok(None) => {} // free or no gate
        Ok(Some(tx_hash)) => {
            dom::set_status(
                &format!("payment received ({}); sending…", short_hash(&tx_hash)),
                false,
            );
        }
        Err(err) => {
            fail_pending_turn(assistant_turn_id, &format!("payment failed: {err}"));
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
        stage::enter(crate::turn_stage::Stage::Starting);
        let t_boot = js_sys::Date::now();
        if let Err(err) = start_session(&key, access.base_url.clone(), &access.identity).await {
            fail_pending_turn(assistant_turn_id, &format!("session start failed: {err:?}"));
            dom::set_status("session start failed — see the message above", true);
            return;
        }
        perf_log(&format!("session boot {} ms (once per session)", (js_sys::Date::now() - t_boot) as u64));
    }
    // Pre-model client work done; the rest of the wait is the proxy + model.
    perf_log(&format!(
        "pre-model {} ms (credit-access {} ms; session {})",
        (js_sys::Date::now() - t0) as u64,
        (t_credit - t0) as u64,
        if session_needs_start { "booted (see above)" } else { "reused" },
    ));

    let Some(agent) = APP.with(|cell| cell.borrow().agent.clone()) else {
        fail_pending_turn(assistant_turn_id, "internal: agent not set after start_session");
        dom::set_status("internal: agent not set after start_session", true);
        return;
    };

    // === Continuous execution ===
    // The first turn carries the user's prompt and renders a user bubble.
    // After it ends, if the model made tool actions but did NOT signal
    // completion (no `finish`, no terminal question) we auto-continue with
    // a brief internal nudge — no user bubble, no Enter press — so the
    // agent drives a multi-step goal to the end instead of stopping after
    // the first step. Bounded by `MAX_AUTO_CONTINUATIONS`, and every
    // iteration cooperatively honours the stop button (TURN_CANCEL).
    // Record the user message for the typed-confirmation gate — a destructive
    // call only executes when its challenge code appears in THIS text (the
    // auto-continue nudges below never overwrite it).
    confirm_guard::note_user_message(&prompt);
    let mut next_input = TurnInput::User(prompt);
    let mut auto_continuations: u32 = 0;
    // The pre-painted shell above feeds the FIRST turn; auto-continuations
    // paint their own (one stage swap target per turn).
    let mut preallocated = Some((assistant_turn_id, first_seg_id));
    // Fresh request — clear the duplicate-action ledger.
    dedup::reset_run();
    loop {
        if TURN_CANCEL.with(|c| c.get()) {
            break;
        }
        // DIFFICULTY ROUTER: pick this turn's thinking budget from the prompt
        // (greeting/short read → Minimal; build/debug/code → High), clamped to
        // the session's ceiling so a routine turn is only ever DOWNGRADED, never
        // upgraded past the user's model choice. Auto-continuations are mid-task
        // (they follow tool/truncation activity) so they route Heavy and keep the
        // full budget. A no-op for backends without thinking control.
        apply_difficulty_route(&agent, &next_input);
        let outcome = stream_turn(&agent, next_input, preallocated.take()).await;

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
            plan_state::clear(); // the plan belongs to the wiped objective
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

    // Cancelled before the first turn ever streamed (stop pressed during the
    // payment gate / session boot): the pre-painted shell was never consumed —
    // finalize it so it doesn't pulse forever.
    if let Some((turn_id, _)) = preallocated {
        mark_turn_done(turn_id);
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

/// DIFFICULTY ROUTER (per-turn): classify `input` and set the agent's per-turn
/// thinking budget for the NEXT turn, clamped to the session ceiling recorded
/// at start (`App::session_thinking_ceiling`). A real user message classifies
/// off its own text with `last_turn_used_tools = false`; an internal
/// auto-continuation / truncated-retry nudge is a continuation of tool /
/// truncation activity, so it routes `Heavy` (full budget) — the router never
/// starves a turn that's mid-task.
///
/// The thinking override is applied via [`crate::Agent::set_thinking_override`],
/// the MODEL override via [`crate::Agent::set_model_override`] (#7) — both no-ops
/// on backends without the respective control. If the session has no ceiling
/// recorded (e.g. local Gemma), BOTH overrides are cleared and the agent falls
/// back to its configured level + model — so the routing is purely additive and
/// safe (the no-override default is byte-identical).
///
/// MODEL selection is the per-turn #7 follow-up to the #2 thinking budget: the
/// SAME tier drives both. [`crate::difficulty::route_model`] returns a cheaper
/// SAME-BACKEND model for a routine turn (clamped to the session model as the
/// ceiling) or `None` to keep the session model. It is structurally incapable of
/// crossing backends or upgrading past the session model, so a `claude-*`
/// session never gets a `gemini-*` id and `Heavy` always stays at the user's
/// pick. Gemini sessions always get `None` (single in-tab flash model).
fn apply_difficulty_route(agent: &Agent, input: &TurnInput) {
    let (ceiling, session_model) =
        APP.with(|cell| {
            let app = cell.borrow();
            (app.session_thinking_ceiling, app.session_model.clone())
        });
    let Some(ceiling) = ceiling else {
        // No thinking control for this backend (e.g. local) — make sure no
        // stale overrides linger, then leave it to the configured level + model.
        agent.set_thinking_override(None);
        agent.set_model_override(None);
        return;
    };
    let (prompt, last_turn_used_tools) = match input {
        TurnInput::User(p) => (p.as_str(), false),
        // A nudge always follows tool activity (Incomplete) or a truncated
        // answer — treat it as mid-task so it keeps the high budget.
        TurnInput::AutoContinue | TurnInput::ResumeTruncated => ("", true),
    };
    // ONE classification drives both the thinking budget and the model.
    let tier = crate::difficulty::classify_turn(prompt, last_turn_used_tools);
    let desired = crate::difficulty::route_tier(tier).thinking;
    let applied = crate::difficulty::clamp_thinking(desired, ceiling);
    agent.set_thinking_override(Some(applied));
    // Per-turn MODEL selection: cheaper same-backend model for routine turns,
    // clamped to the session model. `None` (no same-family cheaper rung, or the
    // session model already chosen) leaves the model unchanged — byte-identical.
    let model = session_model
        .as_deref()
        .and_then(|m| crate::difficulty::route_model(tier, m));
    agent.set_model_override(model);
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
/// The first turn of a run arrives with `pre` = the shell `run_send`
/// pre-painted (user bubble already rendered, stage line already live for
/// the pre-model phases); auto-continuations pass `None` and paint just an
/// assistant bubble so the internal nudge never shows.
async fn stream_turn(agent: &Agent, input: TurnInput, pre: Option<(u32, u32)>) -> TurnOutcome {
    let (prompt, render_user) = match input {
        TurnInput::User(p) => (p, true),
        TurnInput::AutoContinue => (AUTO_CONTINUE_NUDGE.to_string(), false),
        TurnInput::ResumeTruncated => (TRUNCATED_RETRY_NUDGE.to_string(), false),
    };

    // Reuse the pre-painted shell, or allocate + paint a fresh one (element
    // identity fixed before we touch the DOM). Each turn gets its OWN stage
    // swap target; `stage::begin` arms it.
    let (assistant_turn_id, mut seg_id) = match pre {
        Some(ids) => ids,
        None => {
            let (user_turn_id, assistant_turn_id, seg_id) = APP.with(|cell| {
                let mut app = cell.borrow_mut();
                (app.alloc_id(), app.alloc_id(), app.alloc_id())
            });
            if render_user {
                dom::append_html(
                    "transcript",
                    &templates::turn(user_turn_id, "user", html! { (prompt) }, false)
                        .into_string(),
                );
            }
            dom::append_html(
                "transcript",
                &templates::turn(
                    assistant_turn_id,
                    "assistant",
                    html! {
                        (templates::text_segment(seg_id, ""))
                    },
                    true,
                )
                .into_string(),
            );
            dom::scroll_to_bottom("transcript");
            stage::begin(&format!("turn-body-{assistant_turn_id}"));
            (assistant_turn_id, seg_id)
        }
    };

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

    // STATUS: the instant the request goes to the model, show THINKING — it fills
    // the formerly-BLANK gap between send (or `starting`) and the first chunk (the
    // model is connecting + reasoning, no output yet). Previously `Thinking` was
    // entered ONLY when a reasoning delta arrived, so that whole network+TTFT wait
    // had no stage and the cue/glyph went blank (user feedback: the blank step
    // should read as thinking). A real reasoning delta below re-enters Thinking (a
    // no-op) and sets `saw_thinking`; a text-first reply just moves on to Streaming.
    stage::enter(crate::turn_stage::Stage::Thinking);
    let response = match agent.chat(prompt).await {
        Ok(r) => r,
        Err(err) => {
            report_turn_error("agent.chat", &format!("{err}"), assistant_turn_id);
            return TurnOutcome::Error;
        }
    };
    let mut cursor = response.chunks();

    while let Some(item) = cursor.next().await {
        // First output of the turn → log time-to-first-output (the user-perceived
        // "starting" duration: pre-model client work + proxy gate + model TTFT).
        // One-shot per send (the guard is reset in `run_send`).
        if !TTFT_LOGGED.with(|c| c.replace(true)) {
            let t0 = TURN_T0.with(|c| c.get());
            if t0 > 0.0 {
                perf_log(&format!("ttft {} ms (send to first output)", (js_sys::Date::now() - t0) as u64));
            }
        }
        // Honor a stop request (checked per chunk — cooperative).
        if TURN_CANCEL.with(|c| c.get()) {
            break;
        }
        match item {
            Ok(StreamChunk::Text { text, .. }) => {
                if !text.is_empty() {
                    any_visible = true;
                    stage::enter(crate::turn_stage::Stage::Streaming);
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
                // `finish` is an internal completion CONTROL, not a goal step:
                // it ends the autonomous loop and its receipt card is a pure
                // artifact (the user never wants to read a "finish" pill). Mark
                // the turn done and skip rendering ANY card/result for it —
                // here and on history replay (history.rs). It contributes no
                // visible content, so `any_visible` / the stage are left as-is
                // (a bare-finish turn stays eligible for empty-turn removal).
                if call.name == "finish" {
                    saw_finish = true;
                    continue;
                }
                any_visible = true;
                stage::enter(crate::turn_stage::Stage::Tools);
                // `ask_question` is a blocking signal (waiting on the user), NOT
                // a goal step. Only a real goal-step tool marks the turn as
                // mid-goal / continuable.
                if call.name == "ask_question" {
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
                        // embed_app, run_cartridge (#52a) AND
                        // create_and_publish_app (close the cartridge loop)
                        // paint a canvas card and stashed cartridge bytes; now
                        // that the canvas is in the DOM, launch the cartridge
                        // into THIS card's canvas (scoped — older/replayed
                        // cards have their own canvases). No-op for every other
                        // tool / on replay (no stash). Gate = the SAME
                        // native-tested predicate the card renderer uses
                        // (`turn_flow::tool_result_embeds_cartridge`), so the
                        // embed is deterministic on tool success — never
                        // reliant on the model calling embed_app afterwards.
                        if crate::turn_flow::tool_result_embeds_cartridge(
                            &call.name,
                            result.result.as_ref(),
                            result.error.is_some(),
                        ) {
                            super::display::launch_pending_embed(&format!(
                                "tool-{tool_seg_id}-card"
                            ))
                            .await;
                        }
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
                stage::enter(crate::turn_stage::Stage::Thinking);
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

    // Did the model call `finish` this turn? Backends INTERCEPT `finish` and
    // never emit it as a ToolCall chunk, so `saw_finish` above (set on chunks)
    // can't catch it — read the terminal flag off the response instead. This is
    // the signal that the model explicitly declared the turn complete: it stops
    // the auto-continue loop (no redundant "continue toward the goal" sign-off)
    // and suppresses the empty-response bubble on a pure-tool / bare-finish turn.
    if response.finished() {
        saw_finish = true;
        // `finish` is the ABSOLUTE END of the turn — it must never require or
        // re-solicit a closing conversational reply (feedback #41). The model's
        // own text this turn (a pre-tool note / answer) IS that reply, so a
        // separate `finish` summary on top of it reads as redundant and
        // out-of-order (it lands AFTER the tool cards). Only fall back to the
        // `summary` when the turn was otherwise SILENT (a pure tool-only turn
        // with no text at all) — the genuinely-silent-completion case #28 added
        // it for. When anything visible already streamed, the summary is dropped.
        if !any_visible {
            if let Some(summary) = response.finish_summary().filter(|s| !s.is_empty()) {
                dom::append_html(
                    &assistant_body_id,
                    &templates::rendered_markdown(&summary).into_string(),
                );
                dom::scroll_to_bottom("transcript");
                any_visible = true;
            }
        }
        // A pure `finish` turn (no text, no summary, no other tool cards) has
        // nothing to show — the shell we pre-painted would render as an empty
        // bordered bubble. Drop it entirely so a silent completion leaves no
        // artifact.
        if !any_visible {
            dom::remove(&format!("turn-{assistant_turn_id}"));
        }
    }

    // The stream completed without error but produced no visible output.
    // Classify WHY (from the model's finish-reason note + whether it reasoned)
    // so the message names the likely cause + remedy, and so a TRUNCATED turn
    // (model ran out of output budget mid-answer) can be retried rather than
    // dead-ended as a flat "(empty response)".
    //
    // A turn that called `finish` is INTENTIONALLY silent (the tool cards + any
    // final text are the only artifacts) — never paint an empty-response bubble
    // for it, even when it produced no other visible output.
    let empty_kind = if !any_visible && !saw_finish && !TURN_CANCEL.with(|c| c.get()) {
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
        } else {
            // Truncated → no message AND no visible output, so the pre-painted
            // shell would linger as an empty bordered bubble until the retry
            // continuation paints a fresh one. Drop it (same as the pure-`finish`
            // case above) so the retry leaves no empty artifact behind.
            dom::remove(&format!("turn-{assistant_turn_id}"));
        }
        Some(kind)
    } else {
        None
    };

    // If the user hit stop, append a short redirect prompt.
    if TURN_CANCEL.with(|c| c.get()) {
        {
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
    // A confirm-gated tool that was DENIED (challenge issued, or the code
    // wasn't typed by the owner) emitted a ToolCall chunk → `saw_tool_call`,
    // which would classify as `Incomplete` and auto-continue. But the gate is
    // waiting on the OWNER to type the code in their NEXT message; re-running
    // the turn just re-supplies the same code and re-denies, burning credits up
    // to the cap. Stop instead — like a `FinalAnswer` — so control returns to
    // the user (whose next message refreshes `LAST_USER_MSG`).
    let awaiting_confirmation = confirm_guard::take_awaiting_confirmation();
    // `finish` is the end of the objective — a leftover plan must never hold a
    // later conversational turn open.
    if saw_finish {
        plan_state::clear();
    }
    let outcome = classify_turn(
        saw_finish,
        saw_question,
        saw_tool_call,
        any_visible,
        retryable_empty,
        plan_state::is_active(),
    );
    if awaiting_confirmation && matches!(outcome, TurnOutcome::Incomplete) {
        return TurnOutcome::FinalAnswer;
    }
    outcome
}

/// Surface a turn failure. Renders the error INTO the assistant bubble
/// (so a failed turn never looks like a silent blank reply) AND mirrors a
/// short form to the status line. If it looks like a credits/quota problem
/// or an auth / API-key problem (the most common first-run failures), the
/// in-bubble message explains the likely cause and the next step.
fn report_turn_error(context: &str, err: &str, assistant_turn_id: u32) {
    mark_turn_done(assistant_turn_id);
    let lower = err.to_lowercase();
    // Classify the raw error into a stable LHxxxx code — the single source of
    // truth shared by telemetry + this chat surface (src/error_codes.rs).
    use crate::error_codes::{BACKEND_AUTH, BACKEND_CREDITS, BACKEND_STALE_AUTH};
    let code = crate::error_codes::classify(err);
    // The proxy's token-freshness rejection is NOT an API-key problem — the only
    // remaining cause is a device clock off by more than the proxy's 5-minute
    // window. Don't pop the Gemini key modal at a platform-credits user for it.
    let stale_token = code == Some(BACKEND_STALE_AUTH);
    // Off-chain auto error reporting — code-stamped, redacted, deduped, fire-and-
    // forget, no-op when disabled. SKIP expected non-bug states: out-of-credits
    // (402), a stale device clock, and user cancels. A transient provider 429
    // (rate-limit/overload) is a REAL signal (the platform's quota, LH3001) and IS
    // reported — but an EXTERNAL billing spend-cap is the owner's Google-console
    // problem, not a code bug, and refiles every session as pure noise (it filed 4
    // dupe telemetry issues), so it is suppressed here.
    let spend_cap = lower.contains("spending cap") || lower.contains("spend cap");
    if crate::app::telemetry::enabled()
        && !stale_token
        && !spend_cap
        && code != Some(BACKEND_CREDITS)
        && !lower.contains("cancel")
    {
        let agent = crate::app::tenant::current_name().unwrap_or_else(|| "apex".to_string());
        let first = err.lines().next().unwrap_or(err);
        let title = format!(
            "turn error ({context}): {}",
            first.chars().take(120).collect::<String>()
        );
        let signature = crate::app::telemetry::signature_for(&agent, context, err);
        let freeform = format!("context: {context}\n\nerror:\n{err}");
        wasm_bindgen_futures::spawn_local(crate::app::telemetry::report_event(
            "error".to_string(),
            code,
            title,
            signature,
            freeform,
            String::new(),
        ));
    }
    let looks_like_auth = code == Some(BACKEND_AUTH);
    // A provider 429 / quota / spend-cap is the MODEL BACKEND rate-limiting us
    // (LH3001) — NOT the user being out of $LH. classify() separates it from
    // credits so a throttled provider no longer mis-renders the out-of-$LH card
    // (real incident: 183 $LH, "can't send"). The visible branch below says so.
    let looks_like_rate_limit = code == Some(crate::error_codes::BACKEND_RATE_LIMIT);
    // The credit proxy 402s when there's genuinely no active session / no $LH for
    // the signing address (LH3003) — the REAL out-of-$LH case only.
    let looks_like_credits = code == Some(BACKEND_CREDITS);

    // SELF-HEAL for "I have $LH but can't send": a VERIFIED OWNER whose master
    // seed isn't loaded on THIS subdomain chats as a per-origin device key with
    // an EMPTY meter, so the proxy 402s even though the owner's master EOA holds
    // the $LH (the account tab reads that EOA, so it correctly shows the balance —
    // the chat just signs the wrong identity). Pull the master seed via the apex
    // round-trip (the same self-heal mount runs) so credits resolve to the funded
    // EOA and the next send goes through. One-shot per tab (`lh_seed_repull_402`)
    // so a device that genuinely lacks the seed (apex bounces `seed_import=none`)
    // can't loop — after one attempt it just falls through to the card. We DON'T
    // early-return: the card still renders as the fallback; if the pull navigates,
    // the page unloads and the card is moot.
    if looks_like_credits {
        let owner_without_seed = crate::app::APP.with(|c| {
            let app = c.borrow();
            matches!(app.verify_state, crate::app::VerifyState::Verified { .. })
                && app.wallet.is_none()
        });
        if owner_without_seed {
            if let Some(st) = web_sys::window().and_then(|w| w.session_storage().ok().flatten()) {
                if st.get_item("lh_seed_repull_402").ok().flatten().is_none() {
                    let _ = st.set_item("lh_seed_repull_402", "1");
                    if let Some(name) = crate::app::tenant::current_name() {
                        wasm_bindgen_futures::spawn_local(async move {
                            crate::app::seed_pull::kick_export(&name).await;
                        });
                    }
                }
            }
        }
    }

    // Visible message in the transcript. The credits/402 case renders a clean,
    // actionable CARD (on-chain feedback: never dump the raw JSON 402 at the
    // user) — the raw error is logged to the console for debugging only. Every
    // other failure keeps the escaped error bubble (its raw text is dev-facing).
    let body_id = format!("turn-body-{assistant_turn_id}");
    if looks_like_rate_limit {
        // The MODEL backend is over its quota / rate-limited — say so plainly so a
        // funded user doesn't think their $LH vanished. A plain in-stream line (no
        // card), with the next step. Raw error to the console for debugging.
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(err));
        dom::append_html(
            &body_id,
            "<div style=\"color:var(--muted)\">the model backend is over its quota right now — \
             this is the shared model, not your $LH. retry in a moment, or switch model in settings.</div>",
        );
    } else if looks_like_credits {
        web_sys::console::warn_1(&wasm_bindgen::JsValue::from_str(err));
        dom::append_html(&body_id, &super::templates::out_of_credits_card().into_string());
    } else {
        // Prefix the human copy with the stable LHxxxx code + meaning so the
        // user (and we, in telemetry) can name the failure — rustlite-style
        // discipline. The raw error stays appended for devs.
        let headline = if stale_token {
            format!(
                "{} · request auth went stale — your device clock looks off by more \
                 than 5 minutes; sync it and retry",
                crate::error_codes::fmt_label(BACKEND_STALE_AUTH)
            )
        } else if looks_like_auth {
            format!(
                "{} · model rejected the API key — check your Gemini key",
                crate::error_codes::fmt_label(BACKEND_AUTH)
            )
        } else if let Some(c) = code {
            let e = crate::error_codes::lookup(c);
            format!(
                "{} · {} — {}",
                crate::error_codes::fmt_label(c),
                e.map(|x| x.meaning).unwrap_or("error"),
                e.map(|x| x.hint).unwrap_or("")
            )
        } else {
            format!("{context} failed")
        };
        let bubble = format!("{headline} (raw: {err})");
        dom::append_html(
            &body_id,
            &format!(
                "<div class=\"turn-error\">{}</div>",
                dom::msg_span(dom::Msg::Error, &bubble)
            ),
        );
    }
    dom::scroll_to_bottom("transcript");

    if stale_token {
        dom::set_status("auth token went stale — check your device clock, then retry", true);
    } else if looks_like_auth {
        dom::set_status("API key rejected — check your Gemini key.", true);
        super::show_api_key_modal();
    } else if looks_like_rate_limit {
        // The in-stream line already explains it; clear the status line (no
        // duplicate, and definitely not an out-of-$LH message).
        dom::set_status("", false);
    } else if looks_like_credits {
        // The out-of-$LH CARD in the transcript already says it cleanly; a second
        // red status line under it was redundant noise (user feedback). Clear any
        // lingering status instead of restating it.
        dom::set_status("", false);
    } else {
        // The bubble above already carries the full raw error — repeating it
        // here painted the same wall of JSON twice (once in the transcript,
        // once in the input container). Keep the status line to a short
        // marker so the aria-live region still announces the failure.
        dom::set_status("turn failed — see the message above", true);
    }
}

/// Surface a PRE-MODEL failure (payment gate / session boot / internal) in
/// the stream (feedback #64): errors belong in the chronological transcript,
/// not only the footer status line. The pending turn `run_send` pre-painted
/// is finalized (stage line cleared, spinner stopped) and the same
/// `.turn-error` shape `report_turn_error` uses lands INSIDE its body — the
/// error stays where the work was promised.
fn fail_pending_turn(turn_id: u32, text: &str) {
    mark_turn_done(turn_id);
    dom::append_html(
        &format!("turn-body-{turn_id}"),
        &format!(
            "<div class=\"turn-error\">{}</div>",
            dom::msg_span(dom::Msg::Error, text)
        ),
    );
    dom::scroll_to_bottom("transcript");
}

/// The in-tab auto-compaction ceiling — shared by the session config and the
/// context-fullness bar so the bar's "full" always means "compaction next".
pub(crate) const COMPACTION_THRESHOLD: u32 = 128_000;

fn mark_turn_done(turn_id: u32) {
    // The pipeline line is a PENDING-turn affordance — it disappears the
    // moment the turn completes; the final text / error remains.
    stage::end();
    let id = format!("turn-{turn_id}");
    if let Some(el) = dom::by_id(&id) {
        let cls = el.class_name();
        let new_cls: Vec<&str> =
            cls.split_whitespace().filter(|c| *c != "streaming").collect();
        el.set_class_name(&new_cls.join(" "));
    }
}
