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
use futures_util::stream::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Notify};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::backends::anthropic::api::SharedClient;
use crate::backends::anthropic::wire::{
    Block, BlockDelta, ImageSource, Message, MessagesRequest, Role, StopReason, StreamEvent,
    ThinkingConfig, ToolDef, WireUsage, DEFAULT_MAX_TOKENS,
};
use crate::backends::gemini::tools::FINISH_TOOL_NAME;
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    Step, StepSource, StepStatus, StepTarget, StepType, StreamChunk, SystemInstructions,
    ThinkingLevel, ToolCall, ToolResult, UsageMetadata,
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

/// Flatten `SystemInstructions` into Anthropic's top-level `system` String.
pub(crate) fn render_system(s: &SystemInstructions) -> String {
    match s {
        SystemInstructions::Custom(c) => c.text.clone(),
        SystemInstructions::Templated(t) => {
            let mut buf = String::new();
            if let Some(id) = &t.identity {
                buf.push_str(id);
                buf.push_str("\n\n");
            }
            for section in &t.sections {
                if !section.title.is_empty() {
                    buf.push_str("## ");
                    buf.push_str(&section.title);
                    buf.push('\n');
                }
                buf.push_str(&section.content);
                buf.push_str("\n\n");
            }
            buf.trim().to_string()
        }
    }
}

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

pub(crate) async fn run_turn(deps: TurnDeps, user: Message) -> Result<()> {
    deps.state.idle.store(false, Ordering::Release);
    deps.state.cancel.store(false, Ordering::Release);
    {
        let mut hist = deps.state.history.lock();
        hist.push(user);
    }
    *deps.state.last_turn_usage.lock() = Some(UsageMetadata::default());
    *deps.state.last_structured_output.lock() = None;

    let turn_ctx = deps
        .session_ctx
        .as_ref()
        .map(|s| s.child())
        .unwrap_or_default();

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
        let mut accumulated_thinking = String::new();
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

            while let Some(ev_res) = stream.next().await {
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
                                    deps.state.emit(text_delta_step(
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
                                    .emit(text_delta_step(&trajectory_id, step_index, &text));
                            }
                        }
                        BlockDelta::ThinkingDelta { thinking } => {
                            if !thinking.is_empty() {
                                accumulated_thinking.push_str(&thinking);
                                deps.state.emit(thought_delta_step(
                                    &trajectory_id,
                                    step_index,
                                    &thinking,
                                ));
                            }
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

        // Build the assistant-turn content (text + tool_use blocks) and push.
        let mut assistant_blocks: Vec<Block> = Vec::new();
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
                Some(acc) => acc.accumulate(&usage),
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
                    content: json!({ "error": msg }),
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
                    content: json!({ "ok": true }),
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

            let (decision, op_ctx) = if let Some(hooks) = deps.hook_runner.as_ref() {
                hooks.dispatch_pre_tool_call(&turn_ctx, &tool_call).await
            } else {
                (crate::types::HookResult::allow(), turn_ctx.clone())
            };

            let (result_value, post_result_error): (Value, Option<String>) = if !decision.allow {
                let msg = decision.message.clone();
                (json!({ "error": msg.clone() }), Some(msg))
            } else if let Some(runner) = deps.tool_runner.as_ref() {
                match runner.execute(&name, args.clone()).await {
                    Ok(v) => {
                        let err = v.get("error").and_then(|e| e.as_str()).map(String::from);
                        (v, err)
                    }
                    Err(e) => {
                        let s = e.to_string();
                        (json!({ "error": s.clone() }), Some(s))
                    }
                }
            } else {
                let s = format!("no tool runner registered for '{name}'");
                (json!({ "error": s.clone() }), Some(s))
            };

            let post_result = ToolResult {
                name: name.clone(),
                id: Some(id.clone()),
                result: Some(result_value.clone()),
                error: post_result_error.clone(),
            };
            if let Some(hooks) = deps.hook_runner.as_ref() {
                hooks.dispatch_post_tool_call(&op_ctx, &post_result).await;
            }
            deps.state
                .emit_chunk_step(StreamChunk::ToolResult(post_result.clone()));

            result_blocks.push(Block::ToolResult {
                tool_use_id: id,
                content: result_value,
                is_error: post_result_error.map(|_| true),
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
    let terminal = Step {
        id: trajectory_id,
        step_index: deps.state.alloc_step_index(),
        kind: if structured.is_some() {
            StepType::Finish
        } else {
            StepType::TextResponse
        },
        source: StepSource::Model,
        target: StepTarget::User,
        status,
        content: last_text,
        content_delta: String::new(),
        thinking: String::new(),
        thinking_delta: String::new(),
        tool_calls: Vec::new(),
        error: error_msg.to_string(),
        is_complete_response: Some(true),
        structured_output: structured,
        usage_metadata: usage_opt,
    };
    deps.state.emit(terminal);

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

fn accumulate_wire_usage(acc: &mut WireUsage, other: &WireUsage) {
    fn add(a: &mut Option<i32>, b: Option<i32>) {
        if let Some(v) = b {
            *a = Some(a.unwrap_or(0) + v);
        }
    }
    add(&mut acc.input_tokens, other.input_tokens);
    add(&mut acc.output_tokens, other.output_tokens);
    add(
        &mut acc.cache_read_input_tokens,
        other.cache_read_input_tokens,
    );
    add(
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
    let step = Step {
        id: String::new(),
        step_index: state.alloc_step_index(),
        kind: StepType::TextResponse,
        source: StepSource::System,
        target: StepTarget::User,
        status: StepStatus::Error,
        content: String::new(),
        content_delta: String::new(),
        thinking: String::new(),
        thinking_delta: String::new(),
        tool_calls: Vec::new(),
        error: message,
        is_complete_response: Some(true),
        structured_output: None,
        usage_metadata: None,
    };
    state.emit(step);
}

fn text_delta_step(traj: &str, idx: u32, delta: &str) -> Step {
    Step {
        id: traj.to_string(),
        step_index: idx,
        kind: StepType::TextResponse,
        source: StepSource::Model,
        target: StepTarget::User,
        status: StepStatus::Active,
        content: String::new(),
        content_delta: delta.to_string(),
        thinking: String::new(),
        thinking_delta: String::new(),
        tool_calls: Vec::new(),
        error: String::new(),
        is_complete_response: Some(false),
        structured_output: None,
        usage_metadata: None,
    }
}

fn thought_delta_step(traj: &str, idx: u32, delta: &str) -> Step {
    Step {
        id: traj.to_string(),
        step_index: idx,
        kind: StepType::TextResponse,
        source: StepSource::Model,
        target: StepTarget::User,
        status: StepStatus::Active,
        content: String::new(),
        content_delta: String::new(),
        thinking: String::new(),
        thinking_delta: delta.to_string(),
        tool_calls: Vec::new(),
        error: String::new(),
        is_complete_response: Some(false),
        structured_output: None,
        usage_metadata: None,
    }
}

impl LoopState {
    fn emit_chunk_step(&self, chunk: StreamChunk) {
        if let StreamChunk::ToolCall(tc) = chunk {
            let step = Step {
                id: String::new(),
                step_index: self.alloc_step_index(),
                kind: StepType::ToolCall,
                source: StepSource::Model,
                target: StepTarget::Environment,
                status: StepStatus::Active,
                content: String::new(),
                content_delta: String::new(),
                thinking: String::new(),
                thinking_delta: String::new(),
                tool_calls: vec![tc],
                error: String::new(),
                is_complete_response: Some(false),
                structured_output: None,
                usage_metadata: None,
            };
            self.emit(step);
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
}
