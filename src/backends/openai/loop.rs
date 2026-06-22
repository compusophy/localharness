//! Agent loop for the OpenAI Chat Completions backend.
//!
//! Mirrors `backends/anthropic/loop.rs` in control flow; only the wire shapes
//! differ. Each `run_turn` drives one user-initiated turn to completion:
//! optionally many model ↔ tool round-trips, terminating when the model stops
//! with no tool calls (or calls `finish`).
//!
//! The dispatch loop:
//!
//! 1. Build a `ChatRequest` from history + tool declarations.
//! 2. Stream the response. Accumulate `delta.content` text and INDEX-KEYED
//!    `delta.tool_calls` fragments — the `id`/`function.name` land on the
//!    first fragment for an index, and `function.arguments` arrives as STRING
//!    fragments concatenated per tool-call `index` (the #1 OpenAI gotcha).
//!    Parse each completed call's args at stream end.
//! 3. Persist the assistant turn (content + tool_calls) into history.
//! 4. If no tool calls — emit terminal Step, done.
//! 5. Else, dispatch each call through hooks → tool_runner. Append ONE `tool`-
//!    role message PER call (matched by `tool_call_id`) to history.
//! 6. Loop back to step 1.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use base64::Engine as _;
use serde_json::{json, Value};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::backends::dispatch::{dispatch_post_turn, dispatch_tool_call, gate_pre_turn};
use crate::backends::openai::api::SharedClient;
use crate::backends::openai::wire::{
    ChatRequest, FinishReason, FunctionCall, FunctionDef, Message, Role, StreamOptions, ToolCall,
    ToolChoice, ToolDef, WireUsage,
};
use crate::backends::gemini::tools::FINISH_TOOL_NAME;
use crate::backends::stream_timeout::{idle_timeout_ms, next_with_idle_timeout, NextChunk};
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    Step, StepStatus, StreamChunk, SystemInstructions, ToolCall as NeutralToolCall, ToolResult,
    UsageMetadata,
};

/// Maximum dispatch rounds per turn — cap runaway tool loops.
const MAX_TOOL_ROUNDS: u32 = 16;

#[derive(Clone)]
pub(crate) struct LoopConfig {
    pub model: String,
    /// Flattened system prompt — prepended as a `role:"system"` message
    /// (OpenAI has no top-level `system` field, unlike Anthropic).
    pub system: Option<String>,
    pub temperature: Option<f32>,
    /// `max_completion_tokens` for the response. `None` → omit (model default).
    pub max_tokens: Option<u32>,
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
        temperature: Option<f32>,
        max_tokens: Option<u32>,
        tool_declarations: Vec<ToolDef>,
        compaction_threshold: Option<u32>,
    ) -> Result<Self> {
        let system = system.map(render_system);
        Ok(Self {
            model,
            system,
            temperature,
            max_tokens,
            tool_declarations,
            compaction_threshold,
        })
    }
}

// Flatten `SystemInstructions` into a plain string — the shared backend-neutral
// renderer (also used by the Anthropic + local backends).
pub(crate) use crate::backends::render_system;

/// Per-connection mutable state — the shared generic container specialised to
/// OpenAI's wire-history shape (`Vec<Message>`), analogous to Anthropic's.
pub(crate) type LoopState = crate::backends::state::LoopState<Message>;

/// Convert SDK `Content` into an OpenAI user-turn `Message`. OpenAI's text
/// message takes a plain string; media parts are flattened to a data-URL note
/// inside the text (the platform's tools are text-first — image INPUT support
/// is a follow-up; we never silently drop a part).
pub(crate) fn to_wire_user_content(content: Content) -> Result<Message> {
    let mut text = String::new();
    for p in content.parts {
        match p {
            ApiPart::Text(t) => text.push_str(&t),
            ApiPart::Media(m) => {
                // Flatten to a data URL so the part isn't silently dropped.
                let b64 = base64::engine::general_purpose::STANDARD.encode(m.data.as_ref());
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(&format!("[image data:{};base64,{}]", m.mime_type, b64));
            }
        }
    }
    if text.is_empty() {
        return Err(Error::config("empty content"));
    }
    Ok(Message::user_text(text))
}

/// Per-turn dispatcher dependencies. Cloned cheaply (`Arc`s) into the spawned
/// turn task.
#[derive(Clone)]
pub(crate) struct TurnDeps {
    pub client: SharedClient,
    pub config: LoopConfig,
    pub state: Arc<LoopState>,
    pub tool_runner: Option<Arc<ToolRunner>>,
    pub hook_runner: Option<Arc<HookRunner>>,
    pub session_ctx: Option<SessionContext>,
}

/// A tool call accumulated across streamed `delta.tool_calls` fragments, keyed
/// by `index`. `id`/`name` land on the first fragment for an index;
/// `args_json` concatenates every `function.arguments` fragment.
#[derive(Default)]
struct ToolCallAccum {
    id: String,
    name: String,
    /// Concatenated `function.arguments` fragments — parsed once the stream
    /// completes.
    args_json: String,
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
    // never pollutes context.
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
    let mut last_finish: Option<FinishReason> = None;
    // The model called `finish` this turn — flags the terminal step as Finish.
    let mut finished_turn = false;
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
        // Per-tool-call-index accumulators. THE OpenAI-specific contract:
        // `delta.tool_calls` fragments are keyed by `index`, the id+name land
        // on the first fragment for an index, and `function.arguments` arrives
        // as string fragments to concatenate across chunks.
        let mut tool_accum: BTreeMap<u32, ToolCallAccum> = BTreeMap::new();
        let mut finish_reason: Option<FinishReason> = None;
        let mut round_usage = WireUsage::default();

        let request = build_request(&deps.config, &deps.state.history.lock());
        let mut stream = match deps.client.stream_chat(&request).await {
            Ok(s) => s,
            Err(e) => {
                emit_error(&deps.state, e.to_string());
                deps.state.idle.store(true, Ordering::Release);
                deps.state.idle_notify.notify_waiters();
                return Err(e);
            }
        };

        // Idle-stall guard: a fresh `idle_ms` timer is armed for EACH event so
        // a steadily streaming response never trips it — only `idle_ms` of
        // total silence does. On a stall we end the stream with an Err so the
        // turn returns via the normal error path and the one-turn guard
        // releases (vs. hanging on a dead socket the cooperative cancel can't
        // reach).
        let idle_ms = idle_timeout_ms();
        loop {
            let chunk_res = match next_with_idle_timeout(&mut stream, idle_ms).await {
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
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(e) => {
                    emit_error(&deps.state, e.to_string());
                    deps.state.idle.store(true, Ordering::Release);
                    deps.state.idle_notify.notify_waiters();
                    return Err(e);
                }
            };

            if let Some(u) = chunk.usage {
                accumulate_wire_usage(&mut round_usage, &u);
            }
            for choice in chunk.choices {
                if let Some(text) = choice.delta.content {
                    if !text.is_empty() {
                        accumulated_text.push_str(&text);
                        deps.state
                            .emit(Step::text_delta(&trajectory_id, step_index, &text));
                    }
                }
                // Accumulate the index-keyed tool-call fragments.
                for frag in choice.delta.tool_calls {
                    let acc = tool_accum.entry(frag.index).or_default();
                    if let Some(id) = frag.id {
                        if !id.is_empty() {
                            acc.id = id;
                        }
                    }
                    if let Some(f) = frag.function {
                        if let Some(name) = f.name {
                            if !name.is_empty() {
                                acc.name = name;
                            }
                        }
                        if let Some(args) = f.arguments {
                            acc.args_json.push_str(&args);
                        }
                    }
                }
                if let Some(fr) = choice.finish_reason {
                    finish_reason = Some(fr);
                }
            }
        }

        // Resolve tool-call accumulators into ordered calls (parse the
        // concatenated args JSON). An EMPTY/absent fragment is a valid no-arg
        // call → `{}`. A NON-EMPTY fragment that FAILS to parse (truncated
        // stream) must NOT silently run with `{}` — carry the parse error so
        // dispatch surfaces it as a tool error to the model instead.
        let mut pending_calls: Vec<(String, String, Value, Option<String>)> = Vec::new();
        for (_idx, acc) in tool_accum {
            if acc.name.is_empty() {
                continue;
            }
            // OpenAI does not always supply an id (rare); synthesize one so the
            // tool message can correlate.
            let id = if acc.id.is_empty() {
                format!("call_{}", Uuid::new_v4().simple())
            } else {
                acc.id
            };
            let (args, parse_error) = resolve_tool_args(&acc.name, &acc.args_json);
            pending_calls.push((id, acc.name, args, parse_error));
        }

        // Build the assistant-turn message and push. An assistant turn that
        // only calls tools has `content: None`; one with text only has no
        // tool_calls. Both are valid OpenAI shapes.
        let assistant_tool_calls: Vec<ToolCall> = pending_calls
            .iter()
            .map(|(id, name, args, _e)| ToolCall {
                id: id.clone(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: name.clone(),
                    // Re-serialize the parsed args (compact, canonical). For a
                    // parse-error call the args are `{}` (the error is surfaced
                    // separately as the tool result).
                    arguments: args.to_string(),
                },
            })
            .collect();
        if !accumulated_text.is_empty() || !assistant_tool_calls.is_empty() {
            deps.state.history.lock().push(Message {
                role: Role::Assistant,
                content: if accumulated_text.is_empty() {
                    None
                } else {
                    Some(accumulated_text.clone())
                },
                tool_calls: assistant_tool_calls,
                tool_call_id: None,
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
        last_finish = finish_reason;

        // No tool calls → turn over.
        if pending_calls.is_empty() {
            break;
        }

        if deps.state.cancel.load(Ordering::Acquire) {
            debug!("turn cancelled before tool dispatch");
            break;
        }

        // Dispatch every tool call; each result is appended as its OWN
        // `tool`-role message correlated by `tool_call_id`.
        let mut result_messages: Vec<Message> = Vec::with_capacity(pending_calls.len());
        let mut saw_finish = false;
        for (id, name, args, parse_error) in pending_calls {
            // Streamed args failed to parse — surface a clear tool error to the
            // model instead of running the tool with `{}`. Skip execution.
            if let Some(msg) = parse_error {
                let post_result = ToolResult {
                    name: name.clone(),
                    id: Some(id.clone()),
                    result: Some(json!({ "error": msg.clone() })),
                    error: Some(msg.clone()),
                };
                deps.state
                    .emit_chunk_step(StreamChunk::ToolResult(post_result));
                result_messages.push(Message::tool_result(
                    id,
                    json!({ "error": msg }).to_string(),
                ));
                continue;
            }
            if name == FINISH_TOOL_NAME {
                if let Some(out) = args.get("output").cloned() {
                    *deps.state.last_structured_output.lock() = Some(out);
                }
                saw_finish = true;
                result_messages.push(Message::tool_result(id, json!({ "ok": true }).to_string()));
                continue;
            }

            let tool_call = NeutralToolCall {
                name: name.clone(),
                args: args.clone(),
                // OpenAI correlates by id — set it (Gemini leaves None).
                id: Some(id.clone()),
                canonical_path: extract_canonical_path(&args),
            };
            deps.state
                .emit_chunk_step(StreamChunk::ToolCall(tool_call.clone()));

            // The shared pipeline: pre-hooks → execute → error-lift →
            // post-hooks. `post_result.id` carries the tool-call id so results
            // correlate OpenAI-style.
            let post_result = dispatch_tool_call(
                deps.tool_runner.as_ref(),
                deps.hook_runner.as_ref(),
                &turn_ctx,
                &tool_call,
            )
            .await;
            let result_value = post_result.result.clone().unwrap_or(Value::Null);
            deps.state
                .emit_chunk_step(StreamChunk::ToolResult(post_result.clone()));

            result_messages.push(Message::tool_result(id, tool_result_content(&result_value)));
        }

        // Push every tool-result message back (each is its own `tool` turn).
        deps.state.history.lock().extend(result_messages);

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

    let (status, error_msg): (StepStatus, &str) = match last_finish {
        Some(FinishReason::ContentFilter) => (StepStatus::Error, "stopped by content filter"),
        Some(FinishReason::Length) => (StepStatus::Done, "stopped at max tokens"),
        _ => (StepStatus::Done, ""),
    };

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
    );
    deps.state.emit(terminal);

    // Post-turn hooks observe the completed turn's final text.
    dispatch_post_turn(deps.hook_runner.as_ref(), &turn_ctx, &last_text).await;

    // Compaction: if the turn pushed prompt tokens over the threshold,
    // summarize the old prefix before the next turn starts.
    let used = usage.prompt_token_count;
    if crate::backends::openai::compaction::should_compact(used, deps.config.compaction_threshold) {
        debug!(
            used,
            threshold = ?deps.config.compaction_threshold,
            "compaction triggered"
        );
        crate::backends::openai::compaction::try_compact(
            &deps.state.history,
            &deps.client,
            &deps.config.model,
        )
        .await;
    }

    deps.state.idle.store(true, Ordering::Release);
    deps.state.idle_notify.notify_waiters();
    debug!(?last_finish, rounds, "turn complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve a tool call's concatenated `arguments` fragment into parsed args.
/// An EMPTY/absent fragment is a valid no-arg call → `({}, None)`. A NON-EMPTY
/// fragment that fails to parse returns `({}, Some(error))`: the caller
/// surfaces that error to the model as a tool error rather than running the
/// tool with empty args silently.
fn resolve_tool_args(name: &str, args_json: &str) -> (Value, Option<String>) {
    if args_json.trim().is_empty() {
        return (json!({}), None);
    }
    match serde_json::from_str(args_json) {
        Ok(v) => (v, None),
        Err(e) => {
            let msg = format!("malformed tool arguments for '{name}': {e} (got: {args_json})");
            warn!(error = %e, name = %name, "tool_call args not valid JSON; surfacing tool error");
            (json!({}), Some(msg))
        }
    }
}

/// Normalize a tool's return `Value` into a `tool`-message `content` STRING.
/// OpenAI's `tool` message content is a string; tools here return arbitrary
/// JSON, so a non-string Value is emitted as its compact JSON text (a string
/// the model parses fine); a Value that's already a string passes through
/// verbatim (no double-quoting).
fn tool_result_content(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn build_request(config: &LoopConfig, history: &[Message]) -> ChatRequest {
    // OpenAI carries the system prompt as a leading `system` message.
    let mut messages: Vec<Message> = Vec::with_capacity(history.len() + 1);
    if let Some(sys) = &config.system {
        messages.push(Message::system(sys.clone()));
    }
    messages.extend_from_slice(history);

    let tool_choice = if config.tool_declarations.is_empty() {
        None
    } else {
        Some(ToolChoice::Auto)
    };

    ChatRequest {
        model: config.model.clone(),
        messages,
        tools: config.tool_declarations.clone(),
        tool_choice,
        stream: true,
        // Ask for the terminal usage chunk (streaming omits usage otherwise).
        stream_options: Some(StreamOptions { include_usage: true }),
        temperature: config.temperature,
        max_completion_tokens: config.max_tokens,
    }
}

/// Build the `tools` array from neutral tool declarations — wrap each
/// `input_schema` in OpenAI's `{type:"function", function:{...}}` shape.
pub(crate) fn tool_def_from(name: String, description: String, input_schema: Value) -> ToolDef {
    ToolDef {
        kind: "function".to_string(),
        function: FunctionDef {
            name,
            description,
            parameters: input_schema,
        },
    }
}

/// Fold a usage report from one stream chunk into the per-round accumulator.
/// OpenAI streaming emits usage only ONCE (the terminal chunk, when
/// `include_usage` is set), so last-writer-wins is exact and harmless.
fn accumulate_wire_usage(acc: &mut WireUsage, other: &WireUsage) {
    fn take_latest(a: &mut Option<i32>, b: Option<i32>) {
        if b.is_some() {
            *a = b;
        }
    }
    take_latest(&mut acc.prompt_tokens, other.prompt_tokens);
    take_latest(&mut acc.completion_tokens, other.completion_tokens);
    take_latest(&mut acc.total_tokens, other.total_tokens);
    if other.prompt_tokens_details.is_some() {
        acc.prompt_tokens_details = other.prompt_tokens_details.clone();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CustomSystemInstructions, SystemInstructions};
    use tokio::sync::broadcast;

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
        let (args, err) = resolve_tool_args("list_subdomains", "");
        assert!(err.is_none(), "empty args must NOT be treated as malformed");
        assert_eq!(args, json!({}));
        let (args2, err2) = resolve_tool_args("list_subdomains", "   ");
        assert!(err2.is_none());
        assert_eq!(args2, json!({}));
    }

    #[test]
    fn resolve_tool_args_malformed_surfaces_error_not_empty() {
        let (args, err) = resolve_tool_args("edit_file", r#"{"path":"a.rs","content":"#);
        assert!(err.is_some(), "malformed non-empty args must surface an error");
        assert!(err.unwrap().contains("malformed tool arguments for 'edit_file'"));
        assert_eq!(args, json!({}));
    }

    /// OpenAI's `tool` message content must be a STRING. A JSON object result
    /// becomes its compact JSON text; a string passes through verbatim.
    #[test]
    fn tool_result_content_objects_become_json_strings() {
        let obj = json!({"contents": "fn main() {}", "lines": 1});
        let wire = tool_result_content(&obj);
        let back: Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(back, obj);
        // Already-string passes through without double-quoting.
        assert_eq!(tool_result_content(&json!("plain")), "plain");
    }

    #[test]
    fn build_request_prepends_system_and_sets_tool_choice() {
        let config = LoopConfig {
            model: "gpt-5-nano".into(),
            system: Some("sys".into()),
            temperature: Some(0.3),
            max_tokens: Some(256),
            tool_declarations: vec![tool_def_from(
                "view_file".into(),
                "read".into(),
                json!({"type": "object"}),
            )],
            compaction_threshold: None,
        };
        let req = build_request(&config, &[Message::user_text("hi")]);
        assert_eq!(req.messages[0].role, Role::System);
        assert_eq!(req.messages[0].content.as_deref(), Some("sys"));
        assert_eq!(req.messages[1].role, Role::User);
        assert!(matches!(req.tool_choice, Some(ToolChoice::Auto)));
        assert_eq!(req.temperature, Some(0.3));
        assert_eq!(req.max_completion_tokens, Some(256));
        assert!(req.stream_options.is_some());
    }

    #[test]
    fn build_request_no_tools_omits_tool_choice() {
        let config = LoopConfig {
            model: "gpt-5-nano".into(),
            system: None,
            temperature: None,
            max_tokens: None,
            tool_declarations: Vec::new(),
            compaction_threshold: None,
        };
        let req = build_request(&config, &[Message::user_text("hi")]);
        assert!(req.tool_choice.is_none());
        // No system → history is the messages verbatim.
        assert_eq!(req.messages.len(), 1);
        assert_eq!(req.messages[0].role, Role::User);
    }

    /// THE OpenAI-specific contract, replicated against the loop's own
    /// accumulation: `delta.tool_calls` fragments are keyed by `index`, id+name
    /// land on the FIRST fragment, and `arguments` string fragments concatenate.
    /// Two interleaved tool calls (indices 0 and 1) must reassemble independently
    /// with no cross-bleed.
    #[test]
    fn tool_call_fragments_accumulate_by_index() {
        use crate::backends::openai::wire::{
            ChunkChoice, Delta, FunctionDelta, ToolCallDelta,
        };

        // Build the per-index accumulation EXACTLY as run_turn does, feeding a
        // sequence of streamed choices.
        let chunks: Vec<Vec<ChunkChoice>> = vec![
            // index 0: id + name, empty args start.
            vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: vec![ToolCallDelta {
                        index: 0,
                        id: Some("call_a".into()),
                        kind: Some("function".into()),
                        function: Some(FunctionDelta {
                            name: Some("view_file".into()),
                            arguments: Some(String::new()),
                        }),
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            }],
            // index 1 starts BEFORE index 0 finishes (interleaved).
            vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: vec![ToolCallDelta {
                        index: 1,
                        id: Some("call_b".into()),
                        kind: Some("function".into()),
                        function: Some(FunctionDelta {
                            name: Some("list_subdomains".into()),
                            arguments: Some(String::new()),
                        }),
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            }],
            // arg fragments for index 0.
            vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: vec![ToolCallDelta {
                        index: 0,
                        id: None,
                        kind: None,
                        function: Some(FunctionDelta {
                            name: None,
                            arguments: Some("{\"path\":".into()),
                        }),
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            }],
            vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: vec![ToolCallDelta {
                        index: 0,
                        id: None,
                        kind: None,
                        function: Some(FunctionDelta {
                            name: None,
                            arguments: Some("\"a.rs\"}".into()),
                        }),
                    }],
                    ..Default::default()
                },
                finish_reason: Some(FinishReason::ToolCalls),
            }],
        ];

        let mut tool_accum: BTreeMap<u32, ToolCallAccum> = BTreeMap::new();
        for choices in chunks {
            for choice in choices {
                for frag in choice.delta.tool_calls {
                    let acc = tool_accum.entry(frag.index).or_default();
                    if let Some(id) = frag.id {
                        if !id.is_empty() {
                            acc.id = id;
                        }
                    }
                    if let Some(f) = frag.function {
                        if let Some(name) = f.name {
                            if !name.is_empty() {
                                acc.name = name;
                            }
                        }
                        if let Some(args) = f.arguments {
                            acc.args_json.push_str(&args);
                        }
                    }
                }
            }
        }

        // index 0: full args reassembled.
        let zero = &tool_accum[&0];
        assert_eq!(zero.id, "call_a");
        assert_eq!(zero.name, "view_file");
        let parsed: Value = serde_json::from_str(&zero.args_json).unwrap();
        assert_eq!(parsed["path"], "a.rs");
        // index 1: id+name, empty args (a valid no-arg call) — no bleed.
        let one = &tool_accum[&1];
        assert_eq!(one.id, "call_b");
        assert_eq!(one.name, "list_subdomains");
        assert!(one.args_json.is_empty());
        let (args1, err1) = resolve_tool_args(&one.name, &one.args_json);
        assert!(err1.is_none());
        assert_eq!(args1, json!({}));
    }

    /// A tool call with NO `id` in the stream gets a synthesized one so the
    /// `tool`-role result message can correlate. (Replicates the run_turn
    /// fallback.)
    #[test]
    fn missing_tool_call_id_is_synthesized() {
        let acc = ToolCallAccum {
            id: String::new(),
            name: "list_subdomains".into(),
            args_json: String::new(),
        };
        let id = if acc.id.is_empty() {
            format!("call_{}", Uuid::new_v4().simple())
        } else {
            acc.id.clone()
        };
        assert!(id.starts_with("call_"));
        assert!(id.len() > "call_".len());
    }

    /// REGRESSION: the inline-dispatched tool's observability step MUST be Done,
    /// not Active — an Active step makes the Agent's spawn_tool_dispatcher
    /// re-execute the (already inline-dispatched) tool. See the gemini loop.
    #[tokio::test]
    async fn inline_tool_call_step_is_done_so_dispatcher_skips_it() {
        let (tx, mut rx) = broadcast::channel::<Step>(8);
        let state = LoopState::new(tx);
        state.emit_chunk_step(StreamChunk::ToolCall(crate::types::ToolCall {
            name: "create_file".into(),
            args: serde_json::Value::Null,
            id: None,
            canonical_path: None,
        }));
        let step = rx.recv().await.expect("a tool-call step was emitted");
        assert_eq!(
            step.status,
            StepStatus::Done,
            "inline-dispatched tool-call step must be Done, not Active",
        );
    }
}
