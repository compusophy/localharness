//! The generic streaming TURN ENGINE (roadmap R7) — ONE copy of the
//! turn-loop scaffold the streaming backends (gemini / anthropic / openai)
//! each re-implemented ~600 lines of.
//!
//! The engine owns everything that was byte-identical across the three loops:
//! the idle/cancel atomics, the pre-turn hook gate (deny BEFORE the prompt
//! enters history), the history push, the retry-wrapped stream open
//! ([`crate::backends::retry::open_stream_with_retry`]), the idle-stall arm
//! ([`crate::backends::stream_timeout`]), `MAX_TOOL_ROUNDS`, the `finish`-tool
//! special case + `finish_summary` capture, per-round usage `merge_round`, the
//! terminal `Step::turn_complete`, post-turn hooks, the compaction trigger,
//! the idle notify, and every error-exit path.
//!
//! Everything wire-specific lives behind [`TurnProvider`] — a STATIC-DISPATCH
//! trait on a zero-sized marker with associated `Message`/`Accum` types (the
//! proven [`crate::backends::compaction::CompactionModel`] pattern: no
//! async-trait objects, no `Send` bounds, wasm-safe by construction). The two
//! genuinely-async provider concerns — opening the model stream and running
//! compaction — are passed into [`run_turn`] as closures (exactly how the
//! compaction engine takes its `summarize` request), so the engine stays
//! client-agnostic and the spawned turn future's `Send`-ness is inferred at
//! monomorphization on native while staying `?Send`-clean on wasm.
//!
//! Two control-flow HOOKS (with engine defaults) cover the real divergence
//! found in the 3-way loop diff:
//! - [`TurnProvider::on_stream_end`] — anthropic's `pause_turn` resume
//!   (re-request against identical history, accumulators retained).
//! - [`TurnProvider::on_cancel_with_pending_calls`] — anthropic #82 / openai
//!   L22 tool-result balancing so a cancelled turn never leaves a dangling
//!   tool call that 400s the next request.
//!
//! R7 is COMPLETE: phase 1 migrated openai, phase 2 anthropic (proving BOTH
//! control-flow hooks: `pause_turn` resume + the #82 cancel balancing), and
//! phase 3 gemini (the always-on default path). All three streaming backends
//! ride this one loop — a scaffold fix lands HERE, once.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use futures_core::Stream;
use serde_json::{json, Value};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::backends::dispatch::{dispatch_post_turn, dispatch_tool_call, gate_pre_turn};
use crate::builtins::FINISH_TOOL_NAME;
use crate::backends::loop_util::extract_canonical_path;
use crate::backends::state::LoopState;
use crate::backends::stream_timeout::{
    idle_timeout_ms, next_with_idle_timeout_or_cancel, NextChunk,
};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    Step, StepStatus, StreamChunk, ToolCall as NeutralToolCall, ToolResult, UsageMetadata,
};

/// Maximum dispatch rounds per turn — cap runaway tool loops. (Hoisted from
/// the identical per-backend consts.)
pub(crate) const MAX_TOOL_ROUNDS: u32 = 16;

/// One tool call resolved out of a round's stream accumulators, in wire
/// order. `id` is the provider's correlation id (`None` on Gemini, which
/// correlates by name); `parse_error` carries the malformed-streamed-args
/// message per the `loop_util::resolve_tool_args` convention.
pub(crate) struct ResolvedCall {
    pub id: Option<String>,
    pub name: String,
    pub args: Value,
    pub parse_error: Option<String>,
}

/// One dispatched call's outcome, handed back to the provider to shape into
/// wire tool-result message(s). `value` is the JSON surfaced to the model
/// (result / `{"ok":true}` finish sentinel / `{"error":..}` envelope);
/// `is_error` marks failures for wires that type them (anthropic `is_error`).
pub(crate) struct DispatchedResult {
    pub call: ResolvedCall,
    pub value: Value,
    /// Read by the anthropic provider (typed `tool_result.is_error`) —
    /// the gemini/openai wires have no error typing on tool messages, so
    /// this is dead unless `anthropic` is enabled.
    #[allow(dead_code)]
    pub is_error: bool,
}

/// What to do when a round's stream ends (see [`TurnProvider::on_stream_end`]).
/// `Resume`/`ProceedAndEndTurn` are constructed by the anthropic provider's
/// `pause_turn` handling and the engine tests — gemini/openai use the default
/// `Proceed`, so those variants are dead unless `anthropic` is enabled.
#[allow(dead_code)]
pub(crate) enum StreamEnd {
    /// The normal path: resolve calls, persist the assistant turn, dispatch.
    Proceed,
    /// Re-open the stream against IDENTICAL history, retaining the round's
    /// accumulators (anthropic `pause_turn`). Ignored while cancelled — a
    /// cancelled pause persists what streamed and ends the turn instead.
    Resume,
    /// Persist what streamed, then END the turn without dispatching tools
    /// (anthropic's pause-resume cap).
    ProceedAndEndTurn,
}

/// Per-round emit surface handed to [`TurnProvider::fold_event`]: the engine
/// owns the accumulated visible text (identical across backends) and the
/// `Step` delta plumbing; the provider just pushes what it decoded.
pub(crate) struct EmitCtx<'a, M> {
    state: &'a LoopState<M>,
    trajectory_id: &'a str,
    step_index: u32,
    text: String,
}

impl<M> EmitCtx<'_, M> {
    /// Append visible text and emit its streaming delta step.
    pub fn push_text(&mut self, t: &str) {
        if !t.is_empty() {
            self.text.push_str(t);
            self.state
                .emit(Step::text_delta(self.trajectory_id, self.step_index, t));
        }
    }

    /// Emit a thinking delta step (the provider keeps any thinking it must
    /// echo back — e.g. anthropic signed blocks — in its own accumulator).
    /// Consumed by the gemini (`thought: true` parts) and anthropic
    /// (`thinking_delta`) providers.
    pub fn push_thought(&mut self, t: &str) {
        if !t.is_empty() {
            self.state
                .emit(Step::thought_delta(self.trajectory_id, self.step_index, t));
        }
    }
}

/// The per-backend seam: everything the engine needs to know about a wire.
/// Implemented on a zero-sized marker; the engine is monomorphized over it
/// (static dispatch — no trait objects, no async methods).
pub(crate) trait TurnProvider {
    /// The wire history message type (`openai::wire::Message`, …).
    type Message: Clone;
    /// The backend's `LoopConfig`.
    type Config;
    /// The request body sent per round.
    type Request: Clone;
    /// One decoded stream item (chunk/event).
    type Event;
    /// Per-round stream accumulator (tool-call fragments, finish reason,
    /// wire usage, provider-owned extras like anthropic thinking blocks).
    type Accum: Default;

    /// Build one round's request from config + a history snapshot.
    fn build_request(config: &Self::Config, history: &[Self::Message]) -> Self::Request;

    /// The compaction trigger threshold (`None` disables).
    fn compaction_threshold(config: &Self::Config) -> Option<u32>;

    /// Fold ONE stream event into the round accumulator, emitting deltas via
    /// `ctx`. An `Err` fails the turn through the shared stream-error path
    /// (anthropic's in-band `error` event).
    fn fold_event(
        acc: &mut Self::Accum,
        ctx: &mut EmitCtx<'_, Self::Message>,
        event: Self::Event,
    ) -> Result<()>;

    /// Drain the accumulated tool-call fragments into ordered, parsed calls
    /// (the `resolve_tool_args` convention: empty args are a valid no-arg
    /// call; a non-empty fragment that fails to parse carries `parse_error`).
    fn resolve_pending_calls(acc: &mut Self::Accum) -> Vec<ResolvedCall>;

    /// The round's usage in neutral form (default = "none reported").
    fn round_usage(acc: &Self::Accum) -> UsageMetadata;

    /// Map the round's provider finish/stop reason onto the terminal step's
    /// `(status, error message)`.
    fn map_finish_reason(acc: &Self::Accum) -> (StepStatus, &'static str);

    /// Assemble the assistant turn to persist (text + tool calls; anthropic
    /// leads with its signed thinking blocks out of `acc`). `None` skips the
    /// push (nothing streamed).
    fn assemble_assistant_message(
        acc: Self::Accum,
        text: &str,
        calls: &[ResolvedCall],
    ) -> Option<Self::Message>;

    /// Shape dispatched results into wire tool-result message(s) to append:
    /// openai returns one `role:"tool"` message PER call; gemini/anthropic
    /// return one batched user turn.
    fn tool_result_messages(results: Vec<DispatchedResult>) -> Vec<Self::Message>;

    /// HOOK: called when a round's stream ends, before resolving calls.
    /// Default: proceed. Anthropic overrides for `pause_turn` (`Resume` under
    /// its `MAX_PAUSE_RESUMES` cap via `pause_resumes`, else
    /// `ProceedAndEndTurn`).
    fn on_stream_end(_acc: &mut Self::Accum, _pause_resumes: u32) -> StreamEnd {
        StreamEnd::Proceed
    }

    /// HOOK: called when the turn is cancelled AFTER the assistant message
    /// carrying tool calls was persisted but BEFORE dispatch. Returns
    /// balancing messages to append so history stays wire-valid (openai L22 /
    /// anthropic #82 — a dangling tool call 400s the next request). Default:
    /// nothing (gemini's wire tolerates the dangle).
    fn on_cancel_with_pending_calls(_calls: &[ResolvedCall]) -> Vec<Self::Message> {
        Vec::new()
    }
}

/// Deps the engine needs per turn — the backend's `TurnDeps` minus the client
/// (the client rides inside the `open`/`compact` closures).
pub(crate) struct EngineDeps<P: TurnProvider> {
    pub config: P::Config,
    pub state: Arc<LoopState<P::Message>>,
    pub tool_runner: Option<Arc<ToolRunner>>,
    pub hook_runner: Option<Arc<HookRunner>>,
    pub session_ctx: Option<SessionContext>,
}

/// Emit the shared turn-failure shape (error step + idle release) and hand
/// the error back for the `return Err(..)` at the call site.
fn turn_fail<M>(state: &LoopState<M>, e: Error) -> Error {
    state.emit_error(e.to_string());
    state.idle.store(true, Ordering::Release);
    state.idle_notify.notify_waiters();
    e
}

/// Drive ONE user-initiated turn to completion: optionally many model ↔ tool
/// round-trips, terminating when the model stops with no tool calls (or calls
/// `finish`). `open` opens one model stream for a request (retried by the
/// shared policy); `compact` runs the provider's compaction fold (fired only
/// when the threshold trips).
pub(crate) async fn run_turn<P, St, Open, OFut, Compact, CFut>(
    deps: EngineDeps<P>,
    user: P::Message,
    prompt: Content,
    open: Open,
    compact: Compact,
) -> Result<()>
where
    P: TurnProvider,
    St: Stream<Item = Result<P::Event>> + Unpin,
    Open: Fn(P::Request) -> OFut,
    OFut: Future<Output = Result<St>>,
    Compact: FnOnce() -> CFut,
    CFut: Future<Output = ()>,
{
    deps.state.idle.store(false, Ordering::Release);
    // Fresh turn starts uncancelled — clear any stale stop from before.
    deps.state.cancel.store(false, Ordering::Release);

    // ONE turn context shared by the pre-turn gate, the per-call tool hooks,
    // and the post-turn hooks of this turn.
    let turn_ctx = deps
        .session_ctx
        .as_ref()
        .map(|s| s.child())
        .unwrap_or_default();

    // Pre-turn gate — BEFORE the prompt enters history, so a denied prompt
    // never pollutes context. On deny the model is never called; the
    // turn_error Step becomes a stream `Err` via `subscribe_step_stream`.
    if let Some(denied) = gate_pre_turn(deps.hook_runner.as_ref(), &turn_ctx, &prompt).await {
        return Err(turn_fail(&deps.state, Error::other(denied)));
    }

    deps.state.history.lock().push(user);
    *deps.state.last_turn_usage.lock() = Some(UsageMetadata::default());
    *deps.state.last_structured_output.lock() = None;

    let mut rounds = 0u32;
    let mut last_text = String::new();
    let mut last_status: (StepStatus, &'static str) = (StepStatus::Done, "");
    // The model called `finish` this turn — flags the terminal step as Finish.
    let mut finished_turn = false;
    // The closing `summary` arg from a `finish` call — painted on the terminal
    // step so a tool-only turn still ends with a reply, not an empty bubble.
    let mut finish_summary: Option<String> = None;
    let trajectory_id = Uuid::new_v4().to_string();

    loop {
        rounds += 1;
        if rounds > MAX_TOOL_ROUNDS {
            warn!(rounds, "exceeded MAX_TOOL_ROUNDS; forcing turn end");
            break;
        }
        // Stop requested before this round's model call — end the turn.
        if deps.state.cancel.load(Ordering::Acquire) {
            debug!("turn cancelled before model call");
            break;
        }

        let step_index = deps.state.alloc_step_index();
        let mut acc = P::Accum::default();
        let mut ctx = EmitCtx {
            state: &deps.state,
            trajectory_id: &trajectory_id,
            step_index,
            text: String::new(),
        };
        let mut pause_resumes = 0u32;

        // Stream(-resume) loop: normally one pass; `StreamEnd::Resume`
        // re-requests against identical history with accumulators retained.
        // Break value: end the turn after persisting (skip tool dispatch).
        let end_after_persist = 'request: loop {
            let request = P::build_request(&deps.config, &deps.state.history.lock());
            // Retry the stream OPEN on a transient transport/5xx/timeout (ONE
            // shared policy+wrapper, #29). A mid-stream error and
            // auth/credits/rate-limit still fail fast. The open ALSO races the
            // cancel flag (tick-6): a Stop pressed while the POST is in flight
            // (no response headers yet) drops the open future — aborting the
            // request, same drop semantics as the mid-stream cancel (#33) —
            // and NEVER retries; the turn then ends exactly like a mid-stream
            // cancel (nothing streamed, no tools pending).
            let mut stream = match crate::backends::retry::open_stream_with_retry_or_cancel(
                || open(request.clone()),
                &deps.state.cancel,
            )
            .await
            {
                crate::backends::retry::OpenOutcome::Opened(s) => s,
                crate::backends::retry::OpenOutcome::Cancelled => {
                    debug!("turn cancelled during stream open — dropping the request");
                    break 'request false;
                }
                crate::backends::retry::OpenOutcome::Failed(e) => {
                    return Err(turn_fail(&deps.state, e))
                }
            };

            // Idle-stall guard: a fresh `idle_ms` timer is armed for EACH
            // event so a steadily streaming response never trips it — only
            // `idle_ms` of total silence does. On a stall we end the stream
            // with an Err so the turn returns via the normal error path and
            // the one-turn guard releases (vs. hanging on a dead socket the
            // cooperative cancel can't reach).
            let idle_ms = idle_timeout_ms();
            loop {
                // The cancel flag is honoured even while the stream is SILENT
                // (model thinking, stalled socket): the helper re-checks it
                // every `CANCEL_POLL_MS`, so the stop button no longer waits
                // for the next chunk to arrive (telemetry #33).
                let ev_res = match next_with_idle_timeout_or_cancel(
                    &mut stream,
                    idle_ms,
                    &deps.state.cancel,
                )
                .await
                {
                    NextChunk::Item(item) => item,
                    NextChunk::End => break,
                    // Stop pressed: break NOW. `stream` drops at the end of
                    // this 'request pass, which ABORTS the in-flight HTTP
                    // request — reqwest-wasm's AbortGuard rides inside
                    // `bytes_stream` (drop → AbortController.abort()); native
                    // drops the hyper body/connection.
                    NextChunk::Cancelled => {
                        debug!("turn cancelled mid-stream — dropping the in-flight response");
                        break;
                    }
                    NextChunk::IdleTimeout => {
                        // A dead/stalled socket IS a transport failure —
                        // Transport's LH3007 fallback buckets it as network
                        // for telemetry (it never reaches the open-retry).
                        let e = Error::transport(format!(
                            "model stream stalled — no data for {}s",
                            idle_ms / 1000
                        ));
                        return Err(turn_fail(&deps.state, e));
                    }
                };
                let ev = match ev_res {
                    Ok(c) => c,
                    Err(e) => return Err(turn_fail(&deps.state, e)),
                };
                if let Err(e) = P::fold_event(&mut acc, &mut ctx, ev) {
                    return Err(turn_fail(&deps.state, e));
                }
            }

            let cancelled = deps.state.cancel.load(Ordering::Acquire);
            match P::on_stream_end(&mut acc, pause_resumes) {
                StreamEnd::Resume if !cancelled => {
                    pause_resumes += 1;
                    debug!(pause_resumes, "provider resumed the stream");
                    continue 'request;
                }
                // A cancelled pause never resumes: persist what streamed and
                // end the turn (mirrors anthropic's `paused` break).
                StreamEnd::Resume | StreamEnd::ProceedAndEndTurn => break 'request true,
                StreamEnd::Proceed => break 'request false,
            }
        };

        // Resolve the accumulated tool calls, map the finish reason, and
        // persist the assistant turn.
        let pending_calls = P::resolve_pending_calls(&mut acc);
        last_status = P::map_finish_reason(&acc);
        let usage = P::round_usage(&acc);
        if let Some(msg) = P::assemble_assistant_message(acc, &ctx.text, &pending_calls) {
            deps.state.history.lock().push(msg);
        }

        // Accumulate usage across rounds.
        if usage != UsageMetadata::default() {
            let mut slot = deps.state.last_turn_usage.lock();
            match slot.as_mut() {
                Some(a) => a.merge_round(&usage),
                None => *slot = Some(usage),
            }
        }

        last_text = ctx.text;

        // No tool calls (or the provider ended the round) → turn over.
        if pending_calls.is_empty() || end_after_persist {
            break;
        }

        // Stop requested while streaming — end now instead of executing the
        // tools. The assistant message carrying these calls is already in
        // history; let the provider balance them so history stays wire-valid.
        if deps.state.cancel.load(Ordering::Acquire) {
            debug!("turn cancelled before tool dispatch");
            let balance = P::on_cancel_with_pending_calls(&pending_calls);
            deps.state.history.lock().extend(balance);
            break;
        }

        // Dispatch every tool call; results are shaped back onto the wire by
        // the provider at the end of the round. The cancel flag is re-checked
        // BETWEEN dispatches (telemetry #33): a stop pressed while one tool
        // runs skips the remaining calls instead of executing them all —
        // the skipped calls are balanced below so history stays wire-valid.
        let mut results: Vec<DispatchedResult> = Vec::with_capacity(pending_calls.len());
        let mut saw_finish = false;
        let mut undispatched: Vec<ResolvedCall> = Vec::new();
        let mut queue: VecDeque<ResolvedCall> = pending_calls.into();
        while let Some(call) = queue.pop_front() {
            if deps.state.cancel.load(Ordering::Acquire) {
                undispatched.push(call);
                undispatched.extend(queue);
                break;
            }
            // Streamed args failed to parse — surface a clear tool error to
            // the model instead of running the tool with `{}`. Skip execution.
            if let Some(msg) = call.parse_error.clone() {
                let post_result = ToolResult {
                    name: call.name.clone(),
                    id: call.id.clone(),
                    result: Some(json!({ "error": msg.clone() })),
                    error: Some(msg.clone()),
                };
                deps.state
                    .emit_chunk_step(StreamChunk::ToolResult(post_result));
                results.push(DispatchedResult {
                    call,
                    value: json!({ "error": msg }),
                    is_error: true,
                });
                continue;
            }
            // `finish` is special: capture structured_output + the closing
            // summary, mark the turn complete, but still produce a result so
            // the model history stays well-formed.
            if call.name == FINISH_TOOL_NAME {
                if let Some(out) = call.args.get("output").cloned() {
                    *deps.state.last_structured_output.lock() = Some(out);
                }
                if let Some(sm) = call.args.get("summary").and_then(|v| v.as_str()) {
                    if !sm.is_empty() {
                        finish_summary = Some(sm.to_string());
                    }
                }
                saw_finish = true;
                results.push(DispatchedResult {
                    call,
                    value: json!({ "ok": true }),
                    is_error: false,
                });
                continue;
            }

            let tool_call = NeutralToolCall {
                name: call.name.clone(),
                args: call.args.clone(),
                id: call.id.clone(),
                canonical_path: extract_canonical_path(&call.args),
            };
            deps.state
                .emit_chunk_step(StreamChunk::ToolCall(tool_call.clone()));

            // The shared pipeline: pre-hooks → execute → error-lift →
            // post-hooks. `post_result.id` carries the tool-call id so
            // results correlate on id-keyed wires.
            let post_result = dispatch_tool_call(
                deps.tool_runner.as_ref(),
                deps.hook_runner.as_ref(),
                &turn_ctx,
                &tool_call,
            )
            .await;
            let value = post_result.result.clone().unwrap_or(Value::Null);
            let is_error = post_result.error.is_some();
            deps.state
                .emit_chunk_step(StreamChunk::ToolResult(post_result));

            results.push(DispatchedResult {
                call,
                value,
                is_error,
            });
        }

        // Stop pressed mid-dispatch: fold every never-dispatched call into the
        // SAME results batch as a synthetic error result, so ONE
        // `tool_result_messages` covers every functionCall in the model turn.
        // A separate balance append left gemini (whose
        // `on_cancel_with_pending_calls` is the empty engine default) with an
        // unbalanced call/response turn that 400s every later message — and
        // the unbalanced history PERSISTS to OPFS, bricking the chat.
        let cancelled_mid_dispatch = !undispatched.is_empty();
        for call in undispatched.drain(..) {
            results.push(DispatchedResult {
                call,
                value: json!({ "error": "cancelled" }),
                is_error: true,
            });
        }

        // Push the tool results back into history in the provider's shape.
        // Guarded on non-empty: a cancel on the FIRST iteration dispatches
        // nothing, and an empty batch could shape into an empty (wire-invalid)
        // tool-results message on some providers.
        if !results.is_empty() {
            deps.state
                .history
                .lock()
                .extend(P::tool_result_messages(results));
        }

        if cancelled_mid_dispatch {
            debug!("turn cancelled between tool dispatches");
            break;
        }

        if saw_finish {
            finished_turn = true;
            break;
        }
        // Otherwise loop and let the model react to the tool results.
    }

    let usage = deps.state.last_turn_usage.lock().clone().unwrap_or_default();
    let usage_opt = if usage == UsageMetadata::default() {
        None
    } else {
        Some(usage.clone())
    };

    let (status, error_msg) = last_status;
    let structured = deps.state.last_structured_output.lock().clone();
    let terminal = Step::turn_complete(
        trajectory_id,
        deps.state.alloc_step_index(),
        status,
        last_text.as_str(),
        error_msg,
        finished_turn,
        structured,
        usage_opt,
    )
    .with_finish_summary(finish_summary);
    deps.state.emit(terminal);

    // Post-turn hooks observe the completed turn's final text — fired after
    // the terminal step, never on denied or errored turns.
    dispatch_post_turn(deps.hook_runner.as_ref(), &turn_ctx, &last_text).await;

    // Compaction: if the turn pushed prompt tokens over the threshold,
    // summarize the old prefix before the next turn starts.
    let used = usage.prompt_token_count;
    if crate::backends::compaction::should_compact(
        used,
        P::compaction_threshold(&deps.config),
    ) {
        debug!(used, "compaction triggered");
        compact().await;
    }

    deps.state.idle.store(true, Ordering::Release);
    deps.state.idle_notify.notify_waiters();
    debug!(rounds, "turn complete");
    Ok(())
}

/// Test-only: fold a batch of events through a provider with a throwaway
/// [`EmitCtx`], so per-backend tests can exercise their fold seam directly
/// (the ctx fields are module-private by design).
#[cfg(test)]
pub(crate) fn test_fold_events<P: TurnProvider>(
    state: &LoopState<P::Message>,
    acc: &mut P::Accum,
    events: Vec<P::Event>,
) {
    let mut ctx = EmitCtx {
        state,
        trajectory_id: "test",
        step_index: 0,
        text: String::new(),
    };
    for ev in events {
        P::fold_event(acc, &mut ctx, ev).expect("fold_event ok");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::TurnContext;
    use crate::types::{HookResult, StepSource, StepType};
    use parking_lot::Mutex;
    use std::collections::VecDeque;
    use tokio::sync::broadcast;

    /// A scripted stream event for the mock wire.
    #[derive(Clone)]
    enum Ev {
        Text(&'static str),
        Call {
            id: &'static str,
            name: &'static str,
            args: &'static str,
        },
        /// Ask `on_stream_end` to resume (the anthropic pause_turn shape).
        Resume,
        /// Flip the cancel atomic mid-stream (reachable via `ctx.state`).
        Cancel,
    }

    #[derive(Default)]
    struct Accum {
        calls: Vec<(String, String, String)>,
        resume: bool,
    }

    /// Mock wire: `Message = String` history entries with readable tags.
    struct MockProvider;
    impl TurnProvider for MockProvider {
        type Message = String;
        type Config = ();
        type Request = usize;
        type Event = Ev;
        type Accum = Accum;

        fn build_request(_c: &(), history: &[String]) -> usize {
            history.len()
        }
        fn compaction_threshold(_c: &()) -> Option<u32> {
            None
        }
        fn fold_event(acc: &mut Accum, ctx: &mut EmitCtx<'_, String>, ev: Ev) -> Result<()> {
            match ev {
                Ev::Text(t) => ctx.push_text(t),
                Ev::Call { id, name, args } => {
                    acc.calls.push((id.into(), name.into(), args.into()))
                }
                Ev::Resume => acc.resume = true,
                Ev::Cancel => ctx.state.cancel.store(true, Ordering::Release),
            }
            Ok(())
        }
        fn resolve_pending_calls(acc: &mut Accum) -> Vec<ResolvedCall> {
            std::mem::take(&mut acc.calls)
                .into_iter()
                .map(|(id, name, args)| {
                    let (args, parse_error) =
                        crate::backends::loop_util::resolve_tool_args(&name, &args);
                    ResolvedCall {
                        id: Some(id),
                        name,
                        args,
                        parse_error,
                    }
                })
                .collect()
        }
        fn round_usage(_acc: &Accum) -> UsageMetadata {
            UsageMetadata::default()
        }
        fn map_finish_reason(_acc: &Accum) -> (StepStatus, &'static str) {
            (StepStatus::Done, "")
        }
        fn assemble_assistant_message(
            _acc: Accum,
            text: &str,
            calls: &[ResolvedCall],
        ) -> Option<String> {
            (!text.is_empty() || !calls.is_empty())
                .then(|| format!("assistant:{text}:{}", calls.len()))
        }
        fn tool_result_messages(results: Vec<DispatchedResult>) -> Vec<String> {
            results
                .into_iter()
                .map(|r| format!("tool:{}:{}", r.call.id.unwrap_or_default(), r.value))
                .collect()
        }
        fn on_stream_end(acc: &mut Accum, pause_resumes: u32) -> StreamEnd {
            if std::mem::take(&mut acc.resume) {
                if pause_resumes < 2 {
                    StreamEnd::Resume
                } else {
                    StreamEnd::ProceedAndEndTurn
                }
            } else {
                StreamEnd::Proceed
            }
        }
        fn on_cancel_with_pending_calls(calls: &[ResolvedCall]) -> Vec<String> {
            calls
                .iter()
                .map(|c| format!("cancelled:{}", c.id.as_deref().unwrap_or_default()))
                .collect()
        }
    }

    type Steps = broadcast::Receiver<Step>;

    /// Run a turn over scripted streams: each inner Vec is one stream's
    /// events (`open` pops the next). Returns (state, steps rx, open count).
    async fn run(
        streams: Vec<Vec<Ev>>,
        hook_runner: Option<Arc<HookRunner>>,
    ) -> (Arc<LoopState<String>>, Steps, u32) {
        let (tx, rx) = broadcast::channel::<Step>(64);
        let state = Arc::new(LoopState::new(tx));
        let deps = EngineDeps::<MockProvider> {
            config: (),
            state: state.clone(),
            tool_runner: None,
            hook_runner,
            session_ctx: None,
        };
        let script = Mutex::new(streams.into_iter().collect::<VecDeque<_>>());
        let opens = std::sync::atomic::AtomicU32::new(0);
        let prompt = Content::text("hi");
        let res = run_turn::<MockProvider, _, _, _, _, _>(
            deps,
            "user:hi".to_string(),
            prompt,
            |_req| {
                opens.fetch_add(1, Ordering::SeqCst);
                let evs = script.lock().pop_front().unwrap_or_default();
                async move {
                    Ok(futures_util::stream::iter(
                        evs.into_iter().map(Ok::<_, Error>),
                    ))
                }
            },
            || async {},
        )
        .await;
        // Only the deny test expects Err; others assert on state.
        let _ = res;
        (state, rx, opens.load(Ordering::SeqCst))
    }

    struct DenyAllTurns;
    #[async_trait::async_trait]
    impl crate::hooks::PreTurnHook for DenyAllTurns {
        fn name(&self) -> &str {
            "test::deny_all_turns"
        }
        async fn run(&self, _ctx: &TurnContext, _prompt: &Content) -> Result<HookResult> {
            Ok(HookResult::deny("nope"))
        }
    }

    /// THE history-pollution invariant, pinned against the ENGINE's copy of
    /// the scaffold (mirrors the gemini loop's pinning test): a denied turn
    /// never pushes the prompt, never opens a stream, emits the System/Error
    /// turn_error shape, and releases the idle guard.
    #[tokio::test]
    async fn pre_turn_deny_keeps_prompt_out_of_history() {
        let hooks = Arc::new(HookRunner::new());
        hooks.register_pre_turn(Arc::new(DenyAllTurns));
        let (state, mut rx, opens) = run(vec![vec![Ev::Text("never")]], Some(hooks)).await;

        assert!(state.history.lock().is_empty(), "denied prompt must not enter history");
        assert_eq!(opens, 0, "the model must never be called on deny");
        assert!(state.idle.load(Ordering::Acquire), "idle guard must release");
        let step = rx.recv().await.expect("a step was broadcast");
        assert_eq!(step.source, StepSource::System);
        assert_eq!(step.status, StepStatus::Error);
        assert!(step.error.contains("turn denied by hook: nope"));
    }

    /// The finish-tool special case: structured output + summary captured,
    /// the tool result persisted through the provider, terminal step Finish.
    #[tokio::test]
    async fn finish_tool_ends_turn_with_summary_and_structured_output() {
        let (state, mut rx, opens) = run(
            vec![vec![
                Ev::Text("working"),
                Ev::Call {
                    id: "c1",
                    name: FINISH_TOOL_NAME,
                    args: r#"{"summary":"all done","output":{"x":1}}"#,
                },
            ]],
            None,
        )
        .await;

        assert_eq!(opens, 1);
        let hist = state.history.lock().clone();
        assert_eq!(
            hist,
            vec![
                "user:hi".to_string(),
                "assistant:working:1".to_string(),
                "tool:c1:{\"ok\":true}".to_string(),
            ],
            "user + assistant + finish result persisted in order"
        );
        // Drain to the terminal step.
        let mut terminal = None;
        while let Ok(s) = rx.try_recv() {
            if s.is_complete_response == Some(true) {
                terminal = Some(s);
            }
        }
        let t = terminal.expect("terminal step emitted");
        assert_eq!(t.kind, StepType::Finish);
        assert_eq!(t.finish_summary.as_deref(), Some("all done"));
        assert_eq!(t.structured_output, Some(json!({"x": 1})));
        assert_eq!(t.content, "working");
        assert!(state.idle.load(Ordering::Acquire));
    }

    /// `on_stream_end` Resume re-opens against identical history with the
    /// accumulators retained; the resume cap (`ProceedAndEndTurn`) persists
    /// what streamed and ends the turn WITHOUT dispatching the pending call.
    #[tokio::test]
    async fn stream_end_resume_reopens_then_end_turn_skips_dispatch() {
        let (state, _rx, opens) = run(
            vec![
                vec![Ev::Text("a"), Ev::Resume],
                vec![Ev::Text("b"), Ev::Resume],
                vec![
                    Ev::Call { id: "c9", name: "view_file", args: "{}" },
                    Ev::Resume, // cap (2 resumes done) → ProceedAndEndTurn
                ],
            ],
            None,
        )
        .await;

        assert_eq!(opens, 3, "one open + two resumes");
        let hist = state.history.lock().clone();
        assert_eq!(
            hist,
            vec!["user:hi".to_string(), "assistant:ab:1".to_string()],
            "text accumulates across resumes; the pending call persists but is NOT dispatched"
        );
    }

    /// Cancel between the assistant push and dispatch routes through
    /// `on_cancel_with_pending_calls` so the provider can balance history.
    #[tokio::test]
    async fn cancel_with_pending_calls_appends_the_providers_balance() {
        let (state, _rx, _opens) = run(
            vec![vec![
                Ev::Call { id: "c1", name: "view_file", args: r#"{"path":"a.rs"}"# },
                Ev::Cancel,
            ]],
            None,
        )
        .await;

        let hist = state.history.lock().clone();
        assert_eq!(
            hist,
            vec![
                "user:hi".to_string(),
                "assistant::1".to_string(),
                "cancelled:c1".to_string(),
            ],
            "the pending call is balanced, never dispatched"
        );
        assert!(state.idle.load(Ordering::Acquire));
    }

    /// Telemetry #33 (stalled stream): cancel fires while the model stream is
    /// SILENT (no chunk to piggyback the check on). The engine must observe it
    /// within the cancel poll slice — dropping the in-flight response — instead
    /// of waiting out the 120s idle window / the next chunk.
    #[tokio::test]
    async fn cancel_breaks_a_silent_stream_without_waiting_for_a_chunk() {
        let (tx, _rx) = broadcast::channel::<Step>(64);
        let state = Arc::new(LoopState::new(tx));
        let deps = EngineDeps::<MockProvider> {
            config: (),
            state: state.clone(),
            tool_runner: None,
            hook_runner: None,
            session_ctx: None,
        };
        let cancel = state.cancel.clone();
        tokio::spawn(async move {
            crate::runtime::sleep_ms(30).await;
            cancel.store(true, Ordering::Release);
        });
        let t0 = std::time::Instant::now();
        run_turn::<MockProvider, _, _, _, _, _>(
            deps,
            "user:hi".to_string(),
            Content::text("hi"),
            |_req| async { Ok(futures_util::stream::pending::<Result<Ev>>()) },
            || async {},
        )
        .await
        .expect("a cancelled turn ends Ok, not Err");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(30),
            "cancel must interrupt the silent stream promptly (not the idle window)"
        );
        assert!(state.idle.load(Ordering::Acquire), "idle guard released");
    }

    /// Tick-6 E2E corner: cancel fires during the stream-OPEN await (POST
    /// sent, no response headers yet — before any chunk phase exists). The
    /// engine must end the turn promptly, drop the open future, make exactly
    /// ONE open attempt (a cancel never retries the open), and release the
    /// idle guard — exactly like a mid-stream cancel.
    #[tokio::test]
    async fn cancel_during_stream_open_ends_the_turn_without_retry() {
        let (tx, _rx) = broadcast::channel::<Step>(64);
        let state = Arc::new(LoopState::new(tx));
        let deps = EngineDeps::<MockProvider> {
            config: (),
            state: state.clone(),
            tool_runner: None,
            hook_runner: None,
            session_ctx: None,
        };
        let cancel = state.cancel.clone();
        tokio::spawn(async move {
            crate::runtime::sleep_ms(30).await;
            cancel.store(true, Ordering::Release);
        });
        let opens = std::sync::atomic::AtomicU32::new(0);
        let t0 = std::time::Instant::now();
        run_turn::<MockProvider, _, _, _, _, _>(
            deps,
            "user:hi".to_string(),
            Content::text("hi"),
            |_req| {
                opens.fetch_add(1, Ordering::SeqCst);
                async {
                    // The open never resolves — no headers ever arrive.
                    std::future::pending::<()>().await;
                    Ok(futures_util::stream::iter(std::iter::empty::<Result<Ev>>()))
                }
            },
            || async {},
        )
        .await
        .expect("a turn cancelled during open ends Ok, not Err");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(30),
            "cancel must interrupt the pending open promptly"
        );
        assert_eq!(opens.load(Ordering::SeqCst), 1, "cancel must not retry the open");
        assert!(state.idle.load(Ordering::Acquire), "idle guard released");
        assert_eq!(
            state.history.lock().clone(),
            vec!["user:hi".to_string()],
            "nothing streamed — no assistant message persisted"
        );
    }

    /// Telemetry #33 (long tool calls): cancel fires WHILE a tool runs. The
    /// remaining calls of the round are skipped and balanced via the provider
    /// hook — Stop no longer executes the whole batch to completion.
    #[tokio::test]
    async fn cancel_during_a_tool_call_skips_and_balances_the_rest() {
        let (tx, _rx) = broadcast::channel::<Step>(64);
        let state = Arc::new(LoopState::new(tx));
        let runner = Arc::new(ToolRunner::new());
        let cancel = state.cancel.clone();
        runner.register(crate::tools::ClosureTool::new(
            "stopper",
            "flips cancel mid-run (simulates Stop during a long tool call)",
            json!({"type": "object", "properties": {}}),
            move |_args, _ctx| {
                let cancel = cancel.clone();
                async move {
                    cancel.store(true, Ordering::Release);
                    Ok(json!({ "ok": true }))
                }
            },
        ));
        let deps = EngineDeps::<MockProvider> {
            config: (),
            state: state.clone(),
            tool_runner: Some(runner),
            hook_runner: None,
            session_ctx: None,
        };
        let script = Mutex::new(VecDeque::from(vec![vec![
            Ev::Call { id: "c1", name: "stopper", args: "{}" },
            Ev::Call { id: "c2", name: "stopper", args: "{}" },
        ]]));
        run_turn::<MockProvider, _, _, _, _, _>(
            deps,
            "user:hi".to_string(),
            Content::text("hi"),
            |_req| {
                let evs = script.lock().pop_front().unwrap_or_default();
                async move {
                    Ok(futures_util::stream::iter(
                        evs.into_iter().map(Ok::<_, Error>),
                    ))
                }
            },
            || async {},
        )
        .await
        .expect("turn ok");
        let hist = state.history.lock().clone();
        assert_eq!(
            hist,
            vec![
                "user:hi".to_string(),
                "assistant::2".to_string(),
                "tool:c1:{\"ok\":true}".to_string(),
                "tool:c2:{\"error\":\"cancelled\"}".to_string(),
            ],
            "c1 dispatched (its result persisted); c2 skipped, folded into the same results batch"
        );
        assert!(state.idle.load(Ordering::Acquire));
    }
}
