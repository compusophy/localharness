//! Agent loop for the Anthropic backend.
//!
//! Mirrors `backends/gemini/loop.rs` 1:1 in control flow; only the wire
//! shapes differ. Each `run_turn` drives one user-initiated turn to
//! completion: optionally many model ↔ tool round-trips, terminating when
//! the model stops with no `tool_use` blocks (or calls `finish`).
//!
//! The dispatch loop:
//!
//! 1. Build a `MessagesRequest` from history + tool declarations.
//! 2. Stream the response. Accumulate text, thinking, and `tool_use`
//!    blocks — tool args arrive as `input_json_delta.partial_json`
//!    FRAGMENTS concatenated per block `index`, parsed at
//!    `content_block_stop`.
//! 3. Persist the assistant turn (text + tool_use) into history.
//! 4. If no tool calls — emit terminal Step, done. (`pause_turn` instead
//!    re-requests to resume.)
//! 5. Else, dispatch each call through hooks → tool_runner. Build a
//!    `user`-role message of `tool_result` blocks (matched by `id`) and
//!    append it to history.
//! 6. Loop back to step 1.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Notify};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::backends::dispatch::{dispatch_post_turn, dispatch_tool_call, gate_pre_turn};
use crate::backends::anthropic::api::SharedClient;
use crate::backends::anthropic::wire::{
    Block, BlockDelta, ImageSource, Message, MessagesRequest, Role, StopReason, StreamEvent,
    ThinkingConfig, ToolDef, WireUsage, DEFAULT_MAX_TOKENS,
};
use crate::backends::gemini::tools::FINISH_TOOL_NAME;
use crate::backends::stream_timeout::{idle_timeout_ms, next_with_idle_timeout, NextChunk};
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    Step, StepStatus, StreamChunk, SystemInstructions, ThinkingLevel, ToolCall, ToolResult,
    UsageMetadata,
};

/// Maximum dispatch rounds per turn — cap runaway tool loops.
const MAX_TOOL_ROUNDS: u32 = 16;

/// Hard cap on `pause_turn` resumes within a single round, so a backend
/// stuck emitting `pause_turn` can't spin forever.
const MAX_PAUSE_RESUMES: u32 = 8;

#[derive(Clone)]
pub(crate) struct LoopConfig {
    pub model: String,
    pub system: Option<String>,
    pub thinking: Option<ThinkingLevel>,
    pub temperature: Option<f32>,
    pub max_tokens: u32,
    pub tool_declarations: Vec<ToolDef>,
    /// Token threshold; when the last turn's cumulative prompt-token count
    /// exceeds this, summarize the old prefix of history (compaction).
    /// `None` disables.
    pub compaction_threshold: Option<u32>,
}

impl LoopConfig {
    pub fn from_system(
        model: String,
        system: Option<&SystemInstructions>,
        thinking: Option<ThinkingLevel>,
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        tool_declarations: Vec<ToolDef>,
        compaction_threshold: Option<u32>,
    ) -> Result<Self> {
        let system = system.map(render_system);
        Ok(Self {
            model,
            system,
            thinking,
            temperature,
            max_tokens: max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            tool_declarations,
            compaction_threshold,
        })
    }
}

// Flatten `SystemInstructions` into Anthropic's top-level `system` String —
// the shared backend-neutral renderer (also used by the local backend).
pub(crate) use crate::backends::render_system;

/// Per-connection mutable state. History is `Vec<Message>` (Anthropic's
/// shape) — analogous to Gemini's `Vec<wire::Content>`.
pub(crate) struct LoopState {
    pub history: Mutex<Vec<Message>>,
    pub idle: Arc<AtomicBool>,
    pub idle_notify: Arc<Notify>,
    pub cancel: Arc<AtomicBool>,
    pub steps: broadcast::Sender<Step>,
    pub next_step_index: AtomicU32,
    pub last_turn_usage: Mutex<Option<UsageMetadata>>,
    pub last_structured_output: Mutex<Option<Value>>,
}

impl LoopState {
    pub fn new(steps: broadcast::Sender<Step>) -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            idle: Arc::new(AtomicBool::new(true)),
            idle_notify: Arc::new(Notify::new()),
            cancel: Arc::new(AtomicBool::new(false)),
            steps,
            next_step_index: AtomicU32::new(0),
            last_turn_usage: Mutex::new(None),
            last_structured_output: Mutex::new(None),
        }
    }

    fn alloc_step_index(&self) -> u32 {
        self.next_step_index.fetch_add(1, Ordering::Relaxed)
    }

    fn emit(&self, step: Step) {
        let _ = self.steps.send(step);
    }
}

/// Convert SDK `Content` into an Anthropic user-turn `Message`.
pub(crate) fn to_wire_user_content(content: Content) -> Result<Message> {
    let mut blocks: Vec<Block> = Vec::with_capacity(content.parts.len().max(1));
    for p in content.parts {
        match p {
            ApiPart::Text(t) => blocks.push(Block::Text { text: t }),
            ApiPart::Media(m) => blocks.push(Block::Image {
                source: ImageSource {
                    source_type: "base64".to_string(),
                    media_type: m.mime_type,
                    data: base64::engine::general_purpose::STANDARD.encode(m.data.as_ref()),
                },
            }),
        }
    }
    if blocks.is_empty() {
        return Err(Error::config("empty content"));
    }
    Ok(Message {
        role: Role::User,
        content: blocks,
    })
}

/// Per-turn dispatcher dependencies. Cloned cheaply (`Arc`s) into the
/// spawned turn task.
#[derive(Clone)]
pub(crate) struct TurnDeps {
    pub client: SharedClient,
    pub config: LoopConfig,
    pub state: Arc<LoopState>,
    pub tool_runner: Option<Arc<ToolRunner>>,
    pub hook_runner: Option<Arc<HookRunner>>,
    pub session_ctx: Option<SessionContext>,
}

/// A completed `tool_use` block accumulated across streamed deltas.
#[derive(Default)]
struct ToolUseAccum {
    id: String,
    name: String,
    /// Concatenated `partial_json` fragments — parsed once the block stops.
    args_json: String,
}

/// A completed `thinking` block accumulated across streamed deltas. The
/// `thinking` text arrives as `thinking_delta`s; the `signature` (required
/// to echo the block back alongside a tool_use) arrives as a trailing
/// `signature_delta` for the same block index.
#[derive(Default)]
struct ThinkingAccum {
    thinking: String,
    signature: String,
}

pub(crate) async fn run_turn(deps: TurnDeps, user: Message, prompt: Content) -> Result<()> {
    deps.state.idle.store(false, Ordering::Release);
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
        emit_error(&deps.state, denied.clone());
        deps.state.idle.store(true, Ordering::Release);
        deps.state.idle_notify.notify_waiters();
        return Err(Error::other(denied));
    }

    {
        let mut hist = deps.state.history.lock();
        hist.push(user);
    }
    *deps.state.last_turn_usage.lock() = Some(UsageMetadata::default());
    *deps.state.last_structured_output.lock() = None;

    let mut rounds = 0u32;
    let mut last_text = String::new();
    let mut last_stop: Option<StopReason> = None;
    let trajectory_id = Uuid::new_v4().to_string();

    loop {
        rounds += 1;
        if rounds > MAX_TOOL_ROUNDS {
            warn!(rounds, "exceeded MAX_TOOL_ROUNDS; forcing turn end");
            break;
        }
        if deps.state.cancel.load(Ordering::Acquire) {
            debug!("turn cancelled before model call");
            break;
        }

        let step_index = deps.state.alloc_step_index();
        let mut accumulated_text = String::new();
        // Per-index thinking accumulators. With extended thinking enabled,
        // Anthropic REQUIRES the assistant turn's thinking block(s) — WITH
        // their `signature` — to be echoed back verbatim in the next request
        // whenever that turn also contains `tool_use`; otherwise the follow-up
        // round 400s (`messages.N: ... thinking blocks must be preserved`).
        // So we accumulate each thinking block's text + signature by block
        // index and re-emit them (in index order, ahead of text/tool_use) into
        // the persisted assistant message. (`signature` arrives on a separate
        // `signature_delta`, after the `thinking_delta`s for the same index.)
        let mut thinking_blocks: BTreeMap<u32, ThinkingAccum> = BTreeMap::new();
        // Per-index block accumulators — text blocks need no state, tool_use
        // blocks accumulate id/name/args across deltas.
        let mut tool_blocks: BTreeMap<u32, ToolUseAccum> = BTreeMap::new();
        let mut stop_reason: Option<StopReason> = None;
        let mut round_usage = WireUsage::default();
        // pause_turn resume loop — re-request with the SAME history until a
        // non-pause stop reason (or the resume cap) is reached. The loop's
        // break value is `paused` (true iff we stopped while still in
        // pause_turn, e.g. the resume cap was hit).
        let mut pause_resumes = 0u32;

        let paused = 'request: loop {
            let request = build_request(&deps.config, &deps.state.history.lock());
            let mut stream = match deps.client.stream_messages(&request).await {
                Ok(s) => s,
                Err(e) => {
                    emit_error(&deps.state, e.to_string());
                    deps.state.idle.store(true, Ordering::Release);
                    deps.state.idle_notify.notify_waiters();
                    return Err(e);
                }
            };

            // Idle-stall guard: a fresh `idle_ms` timer is armed for EACH event
            // (re-armed every time data arrives), so a steadily streaming
            // response never trips it — only `idle_ms` of total silence does.
            // On a stall we end the stream with an Err so the turn returns via
            // the normal error path and the one-turn guard releases (vs.
            // hanging on a dead socket the cooperative cancel below can't reach).
            let idle_ms = idle_timeout_ms();
            loop {
                let ev_res = match next_with_idle_timeout(&mut stream, idle_ms).await {
                    NextChunk::Item(item) => item,
                    NextChunk::End => break,
                    NextChunk::IdleTimeout => {
                        let e = Error::other(format!(
                            "model stream stalled — no data for {}s",
                            idle_ms / 1000
                        ));
                        emit_error(&deps.state, e.to_string());
                        deps.state.idle.store(true, Ordering::Release);
                        deps.state.idle_notify.notify_waiters();
                        return Err(e);
                    }
                };
                if deps.state.cancel.load(Ordering::Acquire) {
                    break;
                }
                let ev = match ev_res {
                    Ok(e) => e,
                    Err(e) => {
                        emit_error(&deps.state, e.to_string());
                        deps.state.idle.store(true, Ordering::Release);
                        deps.state.idle_notify.notify_waiters();
                        return Err(e);
                    }
                };

                match ev {
                    StreamEvent::MessageStart { message } => {
                        if let Some(u) = message.usage {
                            accumulate_wire_usage(&mut round_usage, &u);
                        }
                    }
                    StreamEvent::ContentBlockStart {
                        index,
                        content_block,
                    } => {
                        match content_block {
                            Block::ToolUse { id, name, .. } => {
                                tool_blocks.insert(
                                    index,
                                    ToolUseAccum {
                                        id,
                                        name,
                                        args_json: String::new(),
                                    },
                                );
                            }
                            Block::Text { text } => {
                                // content_block_start for a text block may
                                // carry a non-empty seed (rare).
                                if !text.is_empty() {
                                    accumulated_text.push_str(&text);
                                    deps.state.emit(Step::text_delta(
                                        &trajectory_id,
                                        step_index,
                                        &text,
                                    ));
                                }
                            }
                            _ => {}
                        }
                    }
                    StreamEvent::ContentBlockDelta { index, delta } => match delta {
                        BlockDelta::TextDelta { text } => {
                            if !text.is_empty() {
                                accumulated_text.push_str(&text);
                                deps.state
                                    .emit(Step::text_delta(&trajectory_id, step_index, &text));
                            }
                        }
                        BlockDelta::ThinkingDelta { thinking } => {
                            if !thinking.is_empty() {
                                thinking_blocks
                                    .entry(index)
                                    .or_default()
                                    .thinking
                                    .push_str(&thinking);
                                deps.state.emit(Step::thought_delta(
                                    &trajectory_id,
                                    step_index,
                                    &thinking,
                                ));
                            }
                        }
                        BlockDelta::SignatureDelta { signature } => {
                            // The cryptographic signature for the thinking
                            // block at this index — required when echoing the
                            // block back alongside a tool_use.
                            thinking_blocks.entry(index).or_default().signature = signature;
                        }
                        BlockDelta::InputJsonDelta { partial_json } => {
                            if let Some(acc) = tool_blocks.get_mut(&index) {
                                acc.args_json.push_str(&partial_json);
                            }
                        }
                        _ => {}
                    },
                    StreamEvent::ContentBlockStop { .. } => {}
                    StreamEvent::MessageDelta { delta, usage } => {
                        if let Some(r) = delta.stop_reason {
                            stop_reason = Some(r);
                        }
                        if let Some(u) = usage {
                            accumulate_wire_usage(&mut round_usage, &u);
                        }
                    }
                    StreamEvent::MessageStop => {}
                    StreamEvent::Error { error } => {
                        let msg = format!("anthropic stream error [{}]: {}", error.kind, error.message);
                        emit_error(&deps.state, msg.clone());
                        deps.state.idle.store(true, Ordering::Release);
                        deps.state.idle_notify.notify_waiters();
                        return Err(Error::other(msg));
                    }
                    StreamEvent::Ping | StreamEvent::Unknown => {}
                }
            }

            // pause_turn: the model paused mid-turn (e.g. a server-side
            // tool). Re-request with identical history to resume. Anything
            // already streamed (text/tool blocks) stays accumulated.
            if matches!(stop_reason, Some(StopReason::PauseTurn))
                && !deps.state.cancel.load(Ordering::Acquire)
                && pause_resumes < MAX_PAUSE_RESUMES
            {
                pause_resumes += 1;
                debug!(pause_resumes, "anthropic pause_turn; resuming");
                stop_reason = None;
                continue 'request;
            }
            break 'request matches!(stop_reason, Some(StopReason::PauseTurn));
        };

        // Resolve tool_use accumulators into ordered ToolCalls (parse the
        // concatenated args JSON). An EMPTY/absent fragment is a valid
        // no-arg call → `{}`. A NON-EMPTY fragment that FAILS to parse
        // (truncated stream, malformed concat) must NOT silently run the
        // tool with `{}` — that executes with wrong args invisibly. Carry
        // the parse error so dispatch surfaces it as a tool error to the
        // model instead (which can then retry the call correctly).
        let mut pending_calls: Vec<(String, String, Value, Option<String>)> = Vec::new();
        for (_idx, acc) in tool_blocks {
            let (args, parse_error) = resolve_tool_args(&acc.name, &acc.args_json);
            pending_calls.push((acc.id, acc.name, args, parse_error));
        }

        // Build the assistant-turn content and push. Block order matters:
        // Anthropic requires thinking block(s) to lead the turn (ahead of
        // text/tool_use), each carrying its `signature`. We only persist a
        // thinking block once its signature has arrived — an unsigned thinking
        // block echoed back would itself 400. (A thinking block with no
        // signature can only occur on a truncated/cancelled stream, in which
        // case we drop it; the turn won't be replayed correctly anyway.)
        let mut assistant_blocks: Vec<Block> = Vec::new();
        for (_idx, acc) in std::mem::take(&mut thinking_blocks) {
            if !acc.thinking.is_empty() && !acc.signature.is_empty() {
                assistant_blocks.push(Block::Thinking {
                    thinking: acc.thinking,
                    signature: Some(acc.signature),
                });
            }
        }
        if !accumulated_text.is_empty() {
            assistant_blocks.push(Block::Text {
                text: accumulated_text.clone(),
            });
        }
        for (id, name, args, _parse_error) in &pending_calls {
            assistant_blocks.push(Block::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: args.clone(),
            });
        }
        if !assistant_blocks.is_empty() {
            deps.state.history.lock().push(Message {
                role: Role::Assistant,
                content: assistant_blocks,
            });
        }

        // Accumulate usage.
        let usage: UsageMetadata = round_usage.into();
        if usage != UsageMetadata::default() {
            let mut slot = deps.state.last_turn_usage.lock();
            match slot.as_mut() {
                Some(acc) => acc.merge_round(&usage),
                None => *slot = Some(usage),
            }
        }

        last_text = accumulated_text;
        last_stop = stop_reason;

        // No tool calls → turn over. (If paused without resolving, also
        // stop — we hit the resume cap or cancellation.)
        if pending_calls.is_empty() || paused {
            break;
        }

        if deps.state.cancel.load(Ordering::Acquire) {
            debug!("turn cancelled before tool dispatch");
            break;
        }

        // Dispatch every tool call; results return as a user message of
        // tool_result blocks matched by id.
        let mut result_blocks: Vec<Block> = Vec::with_capacity(pending_calls.len());
        let mut saw_finish = false;
        for (id, name, args, parse_error) in pending_calls {
            // Streamed args failed to parse — surface a clear tool error to
            // the model (matching the dispatch error convention) instead of
            // running the tool with `{}`. Skip execution entirely.
            if let Some(msg) = parse_error {
                let post_result = ToolResult {
                    name: name.clone(),
                    id: Some(id.clone()),
                    result: Some(json!({ "error": msg.clone() })),
                    error: Some(msg.clone()),
                };
                deps.state
                    .emit_chunk_step(StreamChunk::ToolResult(post_result));
                result_blocks.push(Block::ToolResult {
                    tool_use_id: id,
                    content: tool_result_content(&json!({ "error": msg })),
                    is_error: Some(true),
                });
                continue;
            }
            if name == FINISH_TOOL_NAME {
                if let Some(out) = args.get("output").cloned() {
                    *deps.state.last_structured_output.lock() = Some(out);
                }
                saw_finish = true;
                result_blocks.push(Block::ToolResult {
                    tool_use_id: id,
                    content: tool_result_content(&json!({ "ok": true })),
                    is_error: None,
                });
                continue;
            }

            let tool_call = ToolCall {
                name: name.clone(),
                args: args.clone(),
                // Anthropic correlates by id — set it (Gemini leaves None).
                id: Some(id.clone()),
                canonical_path: extract_canonical_path(&args),
            };
            deps.state
                .emit_chunk_step(StreamChunk::ToolCall(tool_call.clone()));

            // The shared pipeline: pre-hooks → execute → error-lift →
            // post-hooks. `post_result.id` carries the tool_use id (set on
            // `tool_call` above) so results correlate Anthropic-style.
            let post_result = dispatch_tool_call(
                deps.tool_runner.as_ref(),
                deps.hook_runner.as_ref(),
                &turn_ctx,
                &tool_call,
            )
            .await;
            let result_value = post_result.result.clone().unwrap_or(Value::Null);
            let is_error = post_result.error.is_some();
            deps.state
                .emit_chunk_step(StreamChunk::ToolResult(post_result.clone()));

            result_blocks.push(Block::ToolResult {
                tool_use_id: id,
                content: tool_result_content(&result_value),
                is_error: is_error.then_some(true),
            });
        }

        // Push the tool_result blocks back as a user turn.
        deps.state.history.lock().push(Message {
            role: Role::User,
            content: result_blocks,
        });

        if saw_finish {
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

    let (status, error_msg): (StepStatus, &str) = match last_stop {
        Some(StopReason::Refusal) => (StepStatus::Error, "stopped by refusal"),
        Some(StopReason::MaxTokens) => (StepStatus::Done, "stopped at max tokens"),
        Some(StopReason::PauseTurn) => (StepStatus::Done, "paused (resume cap reached)"),
        _ => (StepStatus::Done, ""),
    };

    let structured = deps.state.last_structured_output.lock().clone();
    let terminal = Step::turn_complete(
        trajectory_id,
        deps.state.alloc_step_index(),
        status,
        last_text.as_str(),
        error_msg,
        structured,
        usage_opt,
    );
    deps.state.emit(terminal);

    // Post-turn hooks observe the completed turn's final text — fired after
    // the terminal step, never on denied or errored turns.
    dispatch_post_turn(deps.hook_runner.as_ref(), &turn_ctx, &last_text).await;

    // Compaction: if the turn pushed prompt tokens over the threshold,
    // summarize the old prefix before the next turn starts.
    let used = usage.prompt_token_count;
    if crate::backends::anthropic::compaction::should_compact(used, deps.config.compaction_threshold)
    {
        debug!(
            used,
            threshold = ?deps.config.compaction_threshold,
            "compaction triggered"
        );
        crate::backends::anthropic::compaction::try_compact(
            &deps.state.history,
            &deps.client,
            &deps.config.model,
        )
        .await;
    }

    deps.state.idle.store(true, Ordering::Release);
    deps.state.idle_notify.notify_waiters();
    debug!(?last_stop, rounds, "turn complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a tool_use block's concatenated `partial_json` fragment into
/// parsed args. An EMPTY/absent fragment is a valid no-arg call → `({}, None)`.
/// A NON-EMPTY fragment that fails to parse returns `({}, Some(error))`: the
/// caller surfaces that error to the model as a tool error rather than running
/// the tool with empty args silently.
fn resolve_tool_args(name: &str, args_json: &str) -> (Value, Option<String>) {
    if args_json.trim().is_empty() {
        return (json!({}), None);
    }
    match serde_json::from_str(args_json) {
        Ok(v) => (v, None),
        Err(e) => {
            let msg = format!("malformed tool arguments for '{name}': {e} (got: {args_json})");
            warn!(error = %e, name = %name, "tool_use args not valid JSON; surfacing tool error");
            (json!({}), Some(msg))
        }
    }
}

/// Normalize a tool's return `Value` into a wire-valid `tool_result.content`.
///
/// Anthropic's `tool_result.content` is typed `string | content_block[]` — a
/// bare JSON object/array/number is REJECTED with a 400 (`tool_result.content:
/// Input should be a valid string`). Tools here return arbitrary JSON (objects
/// like `{"contents": "..."}`, the finish sentinel `{"ok": true}`, error
/// envelopes), so an unwrapped object would break EVERY tool round-trip. We map
/// any non-string Value to its compact JSON text (a string the model parses
/// fine); a Value that's already a string passes through verbatim (no double
/// quoting). The result is always a `Value::String`, which the API accepts.
fn tool_result_content(v: &Value) -> Value {
    match v {
        Value::String(_) => v.clone(),
        other => Value::String(other.to_string()),
    }
}

pub(crate) fn build_request(config: &LoopConfig, history: &[Message]) -> MessagesRequest {
    // Clamp thinking budget below max_tokens (Anthropic requires
    // max_tokens > budget_tokens, budget >= 1024).
    let (thinking, max_tokens) = match config.thinking.map(thinking_level_to_budget) {
        Some(budget) => {
            // Ensure max_tokens strictly exceeds the budget.
            let max = config.max_tokens.max(budget + 1024);
            (Some(ThinkingConfig::enabled(budget)), max)
        }
        None => (None, config.max_tokens),
    };

    MessagesRequest {
        model: config.model.clone(),
        max_tokens,
        system: config.system.clone(),
        messages: history.to_vec(),
        tools: config.tool_declarations.clone(),
        tool_choice: None,
        stream: true,
        // Thinking and temperature are mutually exclusive on Anthropic;
        // drop temperature when thinking is on.
        temperature: if thinking.is_some() {
            None
        } else {
            config.temperature
        },
        thinking,
    }
}

/// Map a neutral thinking level onto an Anthropic `budget_tokens` (>= 1024).
fn thinking_level_to_budget(level: ThinkingLevel) -> u32 {
    match level {
        ThinkingLevel::Minimal => 1024,
        ThinkingLevel::Low => 2048,
        ThinkingLevel::Medium => 8192,
        ThinkingLevel::High => 16384,
    }
}

/// Fold a usage report from one stream event into the per-round accumulator.
///
/// Anthropic's streaming usage is CUMULATIVE, not incremental: `message_start`
/// reports `input_tokens` + cache fields + a small PLACEHOLDER `output_tokens`
/// (the envelope, typically 1-4), and each `message_delta` reports the running
/// TOTAL `output_tokens` so far. The terminal value is therefore the LATEST
/// reported value, NOT the sum — summing would double-count the placeholder
/// every round (and over-count badly if the model emits several
/// `message_delta`s). So every field is "take the latest non-`None` value"
/// (last-writer-wins), which is correct for cumulative counters and harmless
/// for the report-once fields (`input_tokens`, cache) that only ever appear in
/// `message_start`.
fn accumulate_wire_usage(acc: &mut WireUsage, other: &WireUsage) {
    fn take_latest(a: &mut Option<i32>, b: Option<i32>) {
        if b.is_some() {
            *a = b;
        }
    }
    take_latest(&mut acc.input_tokens, other.input_tokens);
    take_latest(&mut acc.output_tokens, other.output_tokens);
    take_latest(
        &mut acc.cache_read_input_tokens,
        other.cache_read_input_tokens,
    );
    take_latest(
        &mut acc.cache_creation_input_tokens,
        other.cache_creation_input_tokens,
    );
}

fn extract_canonical_path(args: &Value) -> Option<String> {
    let path_str = args.get("path").and_then(|v| v.as_str())?;
    let path = std::path::Path::new(path_str);
    if let Ok(p) = dunce::canonicalize(path) {
        return Some(p.display().to_string());
    }
    let parent = path.parent()?;
    let file = path.file_name()?;
    let parent = if parent.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        parent
    };
    dunce::canonicalize(parent)
        .ok()
        .map(|p| p.join(file).display().to_string())
}

fn emit_error(state: &LoopState, message: String) {
    state.emit(Step::turn_error(state.alloc_step_index(), message));
}

impl LoopState {
    fn emit_chunk_step(&self, chunk: StreamChunk) {
        if let StreamChunk::ToolCall(tc) = chunk {
            self.emit(Step::tool_call(
                self.alloc_step_index(),
                tc,
                StepStatus::Active,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CustomSystemInstructions, SystemInstructions};

    #[test]
    fn render_system_custom() {
        let s = SystemInstructions::Custom(CustomSystemInstructions {
            text: "be terse".into(),
        });
        assert_eq!(render_system(&s), "be terse");
    }

    #[test]
    fn resolve_tool_args_valid_json_parses() {
        let (args, err) = resolve_tool_args("view_file", r#"{"path":"main.rs"}"#);
        assert!(err.is_none());
        assert_eq!(args["path"], "main.rs");
    }

    #[test]
    fn resolve_tool_args_empty_is_valid_no_arg_call() {
        // A legitimately no-arg tool sends no partial_json → {} with no error.
        let (args, err) = resolve_tool_args("list_subdomains", "");
        assert!(err.is_none(), "empty args must NOT be treated as malformed");
        assert_eq!(args, json!({}));
        // Whitespace-only is also "no args".
        let (args2, err2) = resolve_tool_args("list_subdomains", "   ");
        assert!(err2.is_none());
        assert_eq!(args2, json!({}));
    }

    #[test]
    fn resolve_tool_args_malformed_surfaces_error_not_empty() {
        // Non-empty but invalid JSON (e.g. truncated stream) → error, NOT a
        // silent empty-object execution.
        let (args, err) = resolve_tool_args("edit_file", r#"{"path":"a.rs","content":"#);
        assert!(err.is_some(), "malformed non-empty args must surface an error");
        let msg = err.unwrap();
        assert!(msg.contains("malformed tool arguments for 'edit_file'"));
        // args fall back to {} for the assistant-turn echo, but the error
        // (set above) makes dispatch skip execution and report the failure.
        assert_eq!(args, json!({}));
    }

    /// REGRESSION: Anthropic's `tool_result.content` is typed
    /// `string | content_block[]`; a bare JSON object/array/number is
    /// REJECTED with a 400 (`tool_result.content: Input should be a valid
    /// string`). The fs/agent tools here return JSON OBJECTS (e.g.
    /// `{"contents": "..."}`) and the loop echoed them straight into
    /// `tool_result.content`, which would 400 on EVERY tool round-trip.
    /// `tool_result_content` must serialize a non-string Value to its JSON
    /// text (a valid string), and pass a string Value through verbatim (no
    /// double-quoting).
    #[test]
    fn tool_result_content_objects_become_json_strings() {
        // An object → a JSON string the model can read.
        let obj = json!({"contents": "fn main() {}", "lines": 1});
        let wire = tool_result_content(&obj);
        assert!(wire.is_string(), "object must serialize to a string, got {wire}");
        // Round-trips back to the same object.
        let back: Value = serde_json::from_str(wire.as_str().unwrap()).unwrap();
        assert_eq!(back, obj);

        // The finish/error sentinels are objects too — also stringified.
        assert!(tool_result_content(&json!({"ok": true})).is_string());
        assert!(tool_result_content(&json!({"error": "boom"})).is_string());

        // An array result is likewise stringified (also not a valid bare
        // content value).
        assert!(tool_result_content(&json!(["a", "b"])).is_string());

        // A Value that is ALREADY a string passes through unchanged — we must
        // NOT double-quote it into "\"hi\"".
        let s = json!("plain text result");
        assert_eq!(tool_result_content(&s), json!("plain text result"));
    }

    #[test]
    fn build_request_clamps_thinking_below_max_tokens() {
        let config = LoopConfig {
            model: "claude-haiku-4-5-20251001".into(),
            system: None,
            thinking: Some(ThinkingLevel::High),
            temperature: Some(0.7),
            max_tokens: 8192, // below budget+1024 (16384+1024) → must bump
            tool_declarations: Vec::new(),
            compaction_threshold: None,
        };
        let req = build_request(&config, &[Message::user_text("hi")]);
        let thinking = req.thinking.expect("thinking enabled");
        assert!(
            req.max_tokens > thinking.budget_tokens,
            "max_tokens ({}) must exceed budget ({})",
            req.max_tokens,
            thinking.budget_tokens
        );
        // Thinking on → temperature dropped (mutually exclusive).
        assert!(req.temperature.is_none());
    }

    #[test]
    fn build_request_no_thinking_keeps_temperature() {
        let config = LoopConfig {
            model: "claude-haiku-4-5-20251001".into(),
            system: Some("sys".into()),
            thinking: None,
            temperature: Some(0.3),
            max_tokens: 4096,
            tool_declarations: Vec::new(),
            compaction_threshold: None,
        };
        let req = build_request(&config, &[Message::user_text("hi")]);
        assert!(req.thinking.is_none());
        assert_eq!(req.temperature, Some(0.3));
        assert_eq!(req.max_tokens, 4096);
        assert_eq!(req.system.as_deref(), Some("sys"));
    }

    /// REGRESSION: Anthropic splits usage across two events — `message_start`
    /// carries `input_tokens` + a small PLACEHOLDER `output_tokens` (typically
    /// 1-4, the envelope), and `message_delta` carries the CUMULATIVE final
    /// `output_tokens` for the whole message. `run_turn` folds both into
    /// `round_usage` exactly as this test does. `output_tokens` must end up
    /// equal to the `message_delta` value (the cumulative total), NOT
    /// `message_start.output_tokens + message_delta.output_tokens` — adding
    /// them double-counts the placeholder and over-reports billed output
    /// tokens every turn (and compounds per round in a multi-round tool turn).
    #[test]
    fn usage_does_not_double_count_message_start_output_placeholder() {
        // Replicates the exact accumulation `run_turn` performs in one round.
        let mut round = WireUsage::default();
        // message_start.message.usage
        accumulate_wire_usage(
            &mut round,
            &WireUsage {
                input_tokens: Some(12),
                output_tokens: Some(1), // placeholder envelope count
                cache_read_input_tokens: Some(8),
                cache_creation_input_tokens: None,
            },
        );
        // message_delta.usage — cumulative final output count.
        accumulate_wire_usage(
            &mut round,
            &WireUsage {
                input_tokens: None,
                output_tokens: Some(33),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            },
        );

        assert_eq!(round.input_tokens, Some(12), "input from message_start");
        assert_eq!(
            round.cache_read_input_tokens,
            Some(8),
            "cache_read from message_start"
        );
        assert_eq!(
            round.output_tokens,
            Some(33),
            "output_tokens must be the cumulative message_delta value (33), \
             not message_start placeholder (1) + 33 = 34"
        );

        // The neutral mapping then reports the corrected totals.
        let neutral: UsageMetadata = round.into();
        assert_eq!(neutral.candidates_token_count, Some(33));
        assert_eq!(neutral.prompt_token_count, Some(12));
        assert_eq!(neutral.total_token_count, Some(45)); // 12 + 33
    }

    /// Replicate `run_turn`'s assistant-block assembly for a thinking-enabled
    /// turn that also makes a tool call. Anthropic REQUIRES the signed thinking
    /// block(s) to be echoed back (and lead the assistant turn) whenever the
    /// turn contains a tool_use — omitting them 400s the follow-up round. The
    /// block order must be: thinking (with signature) → text → tool_use.
    #[test]
    fn assistant_turn_preserves_signed_thinking_block_before_tool_use() {
        // Mirror the per-index accumulators run_turn fills from the stream.
        let mut thinking_blocks: BTreeMap<u32, ThinkingAccum> = BTreeMap::new();
        // Block 0: a thinking block with text + a trailing signature.
        thinking_blocks.insert(
            0,
            ThinkingAccum {
                thinking: "Let me reason about this.".into(),
                signature: "sig_abc".into(),
            },
        );
        let accumulated_text = "I'll read it.".to_string();
        let pending_calls: Vec<(String, String, Value, Option<String>)> =
            vec![("toolu_1".into(), "view_file".into(), json!({"path": "a.rs"}), None)];

        // --- assembly copied from run_turn (thinking → text → tool_use) ---
        let mut assistant_blocks: Vec<Block> = Vec::new();
        for (_idx, acc) in std::mem::take(&mut thinking_blocks) {
            if !acc.thinking.is_empty() && !acc.signature.is_empty() {
                assistant_blocks.push(Block::Thinking {
                    thinking: acc.thinking,
                    signature: Some(acc.signature),
                });
            }
        }
        if !accumulated_text.is_empty() {
            assistant_blocks.push(Block::Text {
                text: accumulated_text.clone(),
            });
        }
        for (id, name, args, _e) in &pending_calls {
            assistant_blocks.push(Block::ToolUse {
                id: id.clone(),
                name: name.clone(),
                input: args.clone(),
            });
        }

        // Thinking must be FIRST and carry its signature.
        match &assistant_blocks[0] {
            Block::Thinking { thinking, signature } => {
                assert_eq!(thinking, "Let me reason about this.");
                assert_eq!(signature.as_deref(), Some("sig_abc"));
            }
            other => panic!("expected leading Thinking block, got {other:?}"),
        }
        assert!(matches!(assistant_blocks[1], Block::Text { .. }));
        assert!(matches!(assistant_blocks[2], Block::ToolUse { .. }));

        // Serializes to the wire shape the API requires for replay.
        let wire = serde_json::to_value(&assistant_blocks[0]).unwrap();
        assert_eq!(wire["type"], "thinking");
        assert_eq!(wire["thinking"], "Let me reason about this.");
        assert_eq!(wire["signature"], "sig_abc");
    }

    /// A thinking block whose `signature_delta` never arrived (truncated /
    /// cancelled stream) must be DROPPED, not echoed back unsigned — an
    /// unsigned thinking block sent back is itself a 400.
    #[test]
    fn unsigned_thinking_block_is_dropped() {
        let mut thinking_blocks: BTreeMap<u32, ThinkingAccum> = BTreeMap::new();
        thinking_blocks.insert(
            0,
            ThinkingAccum {
                thinking: "partial reasoning".into(),
                signature: String::new(), // never signed
            },
        );
        let mut assistant_blocks: Vec<Block> = Vec::new();
        for (_idx, acc) in std::mem::take(&mut thinking_blocks) {
            if !acc.thinking.is_empty() && !acc.signature.is_empty() {
                assistant_blocks.push(Block::Thinking {
                    thinking: acc.thinking,
                    signature: Some(acc.signature),
                });
            }
        }
        assert!(
            assistant_blocks.is_empty(),
            "unsigned thinking must not be persisted"
        );
    }

    /// Multiple `message_delta` events (the spec permits more than one) each
    /// carry the CUMULATIVE output count, so the final reported value must be
    /// the LAST one, not the sum of all of them.
    #[test]
    fn usage_takes_latest_cumulative_output_across_multiple_deltas() {
        let mut round = WireUsage::default();
        accumulate_wire_usage(
            &mut round,
            &WireUsage {
                input_tokens: Some(20),
                output_tokens: Some(2),
                ..Default::default()
            },
        );
        // Two message_delta events, cumulative: 10 then 25.
        accumulate_wire_usage(
            &mut round,
            &WireUsage {
                output_tokens: Some(10),
                ..Default::default()
            },
        );
        accumulate_wire_usage(
            &mut round,
            &WireUsage {
                output_tokens: Some(25),
                ..Default::default()
            },
        );
        assert_eq!(
            round.output_tokens,
            Some(25),
            "cumulative deltas: final reported output is the LAST value (25), not 2+10+25"
        );
        assert_eq!(round.input_tokens, Some(20));
    }
}
