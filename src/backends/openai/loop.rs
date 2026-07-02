//! Agent loop for the OpenAI Chat Completions backend — the first backend on
//! the shared [`crate::backends::turn_engine`] (R7 phase 1).
//!
//! The turn scaffold (idle/cancel atomics, pre-turn gate, retry-wrapped
//! stream open, idle-stall arm, MAX_TOOL_ROUNDS, the finish-tool special
//! case, usage folding, terminal step, compaction trigger) lives in the
//! engine; this module supplies only the OpenAI-specific wire behavior via
//! [`OpenAiProvider`]:
//!
//! - Request shape: full history as `messages` with a leading `system`
//!   message (OpenAI has no top-level `system` field).
//! - Stream fold: accumulate `delta.content` text and INDEX-KEYED
//!   `delta.tool_calls` fragments — the `id`/`function.name` land on the
//!   first fragment for an index, and `function.arguments` arrives as STRING
//!   fragments concatenated per tool-call `index` (the #1 OpenAI gotcha).
//! - Tool results: ONE `tool`-role message PER call, matched by
//!   `tool_call_id` (unlike gemini/anthropic's batched user turn).

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use tracing::warn;
use uuid::Uuid;

use crate::backends::loop_util::resolve_tool_args;
use crate::backends::openai::api::SharedClient;
use crate::backends::openai::wire::{
    ChatChunk, ChatRequest, FinishReason, FunctionCall, FunctionDef, Message, Role, StreamOptions,
    ToolCall, ToolChoice, ToolDef, WireUsage,
};
use crate::backends::turn_engine::{
    self, DispatchedResult, EmitCtx, EngineDeps, ResolvedCall, TurnProvider,
};
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{StepStatus, SystemInstructions, UsageMetadata};

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

/// One round's stream accumulators (the [`TurnProvider::Accum`]).
#[derive(Default)]
pub(crate) struct RoundAccum {
    /// Per-tool-call-index accumulators. THE OpenAI-specific contract:
    /// `delta.tool_calls` fragments are keyed by `index`, the id+name land
    /// on the first fragment for an index, and `function.arguments` arrives
    /// as string fragments to concatenate across chunks.
    tool_accum: BTreeMap<u32, ToolCallAccum>,
    finish_reason: Option<FinishReason>,
    usage: WireUsage,
}

/// The OpenAI side of the [`TurnProvider`] seam — a zero-sized marker the
/// engine is monomorphized over (static dispatch; see `turn_engine`).
pub(crate) struct OpenAiProvider;

impl TurnProvider for OpenAiProvider {
    type Message = Message;
    type Config = LoopConfig;
    type Request = ChatRequest;
    type Event = ChatChunk;
    type Accum = RoundAccum;

    fn build_request(config: &LoopConfig, history: &[Message]) -> ChatRequest {
        build_request(config, history)
    }

    fn compaction_threshold(config: &LoopConfig) -> Option<u32> {
        config.compaction_threshold
    }

    fn fold_event(
        acc: &mut RoundAccum,
        ctx: &mut EmitCtx<'_, Message>,
        chunk: ChatChunk,
    ) -> Result<()> {
        if let Some(u) = chunk.usage {
            accumulate_wire_usage(&mut acc.usage, &u);
        }
        for choice in chunk.choices {
            if let Some(text) = choice.delta.content {
                ctx.push_text(&text);
            }
            // Accumulate the index-keyed tool-call fragments.
            for frag in choice.delta.tool_calls {
                let a = acc.tool_accum.entry(frag.index).or_default();
                if let Some(id) = frag.id {
                    if !id.is_empty() {
                        a.id = id;
                    }
                }
                if let Some(f) = frag.function {
                    if let Some(name) = f.name {
                        if !name.is_empty() {
                            a.name = name;
                        }
                    }
                    if let Some(args) = f.arguments {
                        a.args_json.push_str(&args);
                    }
                }
            }
            if let Some(fr) = choice.finish_reason {
                acc.finish_reason = Some(fr);
            }
        }
        Ok(())
    }

    /// Resolve tool-call accumulators into ordered calls (parse the
    /// concatenated args JSON). An EMPTY/absent fragment is a valid no-arg
    /// call → `{}`. A NON-EMPTY fragment that FAILS to parse (truncated
    /// stream) must NOT silently run with `{}` — carry the parse error so
    /// dispatch surfaces it as a tool error to the model instead.
    fn resolve_pending_calls(acc: &mut RoundAccum) -> Vec<ResolvedCall> {
        let mut out = Vec::new();
        for (idx, a) in std::mem::take(&mut acc.tool_accum) {
            if a.name.is_empty() {
                // Never silently drop accumulated args: a name-less accumulator
                // that still carried args means a truncated or malformed stream
                // — surface it instead of vanishing the call.
                if !a.args_json.trim().is_empty() {
                    warn!("openai: tool-call fragment index {idx} has args but no name; dropping");
                }
                continue;
            }
            // OpenAI does not always supply an id (rare); synthesize one so the
            // tool message can correlate.
            let id = if a.id.is_empty() {
                format!("call_{}", Uuid::new_v4().simple())
            } else {
                a.id
            };
            let (args, parse_error) = resolve_tool_args(&a.name, &a.args_json);
            out.push(ResolvedCall {
                id: Some(id),
                name: a.name,
                args,
                parse_error,
            });
        }
        out
    }

    fn round_usage(acc: &RoundAccum) -> UsageMetadata {
        acc.usage.clone().into()
    }

    fn map_finish_reason(acc: &RoundAccum) -> (StepStatus, &'static str) {
        match acc.finish_reason {
            Some(FinishReason::ContentFilter) => (StepStatus::Error, "stopped by content filter"),
            Some(FinishReason::Length) => (StepStatus::Done, "stopped at max tokens"),
            _ => (StepStatus::Done, ""),
        }
    }

    /// Build the assistant-turn message. An assistant turn that only calls
    /// tools has `content: None`; one with text only has no tool_calls. Both
    /// are valid OpenAI shapes.
    fn assemble_assistant_message(
        _acc: RoundAccum,
        text: &str,
        calls: &[ResolvedCall],
    ) -> Option<Message> {
        let tool_calls: Vec<ToolCall> = calls
            .iter()
            .map(|c| ToolCall {
                id: c.id.clone().unwrap_or_default(),
                kind: "function".to_string(),
                function: FunctionCall {
                    name: c.name.clone(),
                    // Re-serialize the parsed args (compact, canonical). For a
                    // parse-error call the args are `{}` (the error is surfaced
                    // separately as the tool result).
                    arguments: c.args.to_string(),
                },
            })
            .collect();
        if text.is_empty() && tool_calls.is_empty() {
            return None;
        }
        Some(Message {
            role: Role::Assistant,
            content: (!text.is_empty()).then(|| text.to_string()),
            tool_calls,
            tool_call_id: None,
        })
    }

    /// Each result is its OWN `tool`-role message correlated by
    /// `tool_call_id` (unlike gemini/anthropic's single batched user turn).
    fn tool_result_messages(results: Vec<DispatchedResult>) -> Vec<Message> {
        results
            .into_iter()
            .map(|r| {
                Message::tool_result(r.call.id.unwrap_or_default(), tool_result_content(&r.value))
            })
            .collect()
    }

    /// REGRESSION guard (L22, mirrors Anthropic #82): OpenAI 400s the NEXT
    /// request if an assistant `tool_calls` message isn't answered by a
    /// `tool` message per tool_call_id — balance every pending call with a
    /// cancelled tool_result so history stays valid.
    fn on_cancel_with_pending_calls(calls: &[ResolvedCall]) -> Vec<Message> {
        calls
            .iter()
            .map(|c| {
                Message::tool_result(
                    c.id.clone().unwrap_or_default(),
                    json!({ "error": "cancelled" }).to_string(),
                )
            })
            .collect()
    }
}

/// Drive one turn through the shared engine, plugging in the OpenAI client
/// for the stream open and the compaction fold (the engine stays
/// client-agnostic — the async edges ride in as closures, exactly like the
/// compaction engine's `summarize`).
pub(crate) async fn run_turn(deps: TurnDeps, user: Message, prompt: Content) -> Result<()> {
    let TurnDeps {
        client,
        config,
        state,
        tool_runner,
        hook_runner,
        session_ctx,
    } = deps;
    let model = config.model.clone();
    let engine_deps = EngineDeps::<OpenAiProvider> {
        config,
        state: state.clone(),
        tool_runner,
        hook_runner,
        session_ctx,
    };
    let open_client = client.clone();
    turn_engine::run_turn::<OpenAiProvider, _, _, _, _, _>(
        engine_deps,
        user,
        prompt,
        move |req: ChatRequest| {
            let client = open_client.clone();
            async move { client.stream_chat(&req).await }
        },
        move || async move {
            crate::backends::openai::compaction::try_compact(&state.history, &client, &model)
                .await;
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CustomSystemInstructions, Step, StreamChunk, SystemInstructions};
    use tokio::sync::broadcast;

    #[test]
    fn render_system_custom() {
        let s = SystemInstructions::Custom(CustomSystemInstructions {
            text: "be terse".into(),
        });
        assert_eq!(render_system(&s), "be terse");
    }

    // `resolve_tool_args` tests: deduped into the canonical suite in
    // `backends/loop_util.rs` (this loop consumes that one impl).

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

    /// The SAME contract exercised through the provider's real seam methods:
    /// `fold_event` reassembles the fragments and `resolve_pending_calls`
    /// parses them in order (this is the path the engine actually drives).
    #[test]
    fn provider_fold_and_resolve_reassemble_fragments() {
        use crate::backends::openai::wire::{ChunkChoice, Delta, FunctionDelta, ToolCallDelta};

        let frag = |index: u32, id: Option<&str>, name: Option<&str>, args: &str| ChatChunk {
            choices: vec![ChunkChoice {
                index: 0,
                delta: Delta {
                    tool_calls: vec![ToolCallDelta {
                        index,
                        id: id.map(Into::into),
                        kind: None,
                        function: Some(FunctionDelta {
                            name: name.map(Into::into),
                            arguments: Some(args.into()),
                        }),
                    }],
                    ..Default::default()
                },
                finish_reason: None,
            }],
            ..Default::default()
        };

        let (tx, _rx) = broadcast::channel::<Step>(8);
        let state = LoopState::new(tx);
        let mut acc = RoundAccum::default();
        crate::backends::turn_engine::test_fold_events::<OpenAiProvider>(
            &state,
            &mut acc,
            vec![
                frag(0, Some("call_a"), Some("view_file"), "{\"path\":"),
                frag(1, Some("call_b"), Some("list_subdomains"), ""),
                frag(0, None, None, "\"a.rs\"}"),
            ],
        );

        let calls = OpenAiProvider::resolve_pending_calls(&mut acc);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id.as_deref(), Some("call_a"));
        assert_eq!(calls[0].name, "view_file");
        assert_eq!(calls[0].args, json!({"path": "a.rs"}));
        assert!(calls[0].parse_error.is_none());
        assert_eq!(calls[1].id.as_deref(), Some("call_b"));
        assert_eq!(calls[1].args, json!({}), "empty args are a valid no-arg call");
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

    /// REGRESSION (L22, mirrors Anthropic #82): the assistant message carrying
    /// `tool_calls` is pushed to history BEFORE tools dispatch. If the turn is
    /// cancelled in that window the engine must balance every pending call
    /// with a cancelled `tool` message (correlated by id) so history stays
    /// valid — OpenAI 400s the NEXT request otherwise ("must be followed by
    /// tool messages responding to each tool_call_id"). Exercises the
    /// provider's `on_cancel_with_pending_calls` (the hook the engine calls).
    #[test]
    fn cancelled_turn_balances_pending_tool_calls_with_tool_results() {
        let pending_calls = vec![
            ResolvedCall {
                id: Some("call_a".into()),
                name: "view_file".into(),
                args: json!({"path": "a.rs"}),
                parse_error: None,
            },
            ResolvedCall {
                id: Some("call_b".into()),
                name: "list_subdomains".into(),
                args: json!({}),
                parse_error: None,
            },
        ];

        let cancelled = OpenAiProvider::on_cancel_with_pending_calls(&pending_calls);

        // One `tool` message per pending call, ids preserved, content marks cancel.
        assert_eq!(cancelled.len(), 2);
        let ids: Vec<&str> = cancelled
            .iter()
            .map(|m| {
                assert_eq!(m.role, Role::Tool, "balancing message must be a tool result");
                assert!(
                    m.content.as_deref().unwrap().contains("cancelled"),
                    "content should mark the call cancelled"
                );
                m.tool_call_id.as_deref().expect("tool_call_id correlates")
            })
            .collect();
        assert_eq!(ids, vec!["call_a", "call_b"], "every tool_call_id answered");
    }
}
