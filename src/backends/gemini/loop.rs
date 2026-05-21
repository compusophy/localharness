//! Agent loop for the Gemini backend.
//!
//! Each `run_turn` call drives one user-initiated turn to completion:
//! optionally many model ↔ tool round-trips, terminating when the model
//! emits no further `functionCall` parts (or calls `finish`).
//!
//! The dispatch loop:
//!
//! 1. Build a `GenerateContentRequest` from history + tool declarations.
//! 2. Stream the response. Accumulate text, thoughts, and function calls.
//! 3. Persist the model turn (text + functionCalls) into history.
//! 4. If no function calls — emit terminal Step, done.
//! 5. Else, dispatch each call through hooks → tool_runner. Build a
//!    `user`-role `functionResponse` content and append it to history.
//! 6. Loop back to step 1.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use base64::Engine as _;
use futures_util::stream::StreamExt;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Notify};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::tools::FINISH_TOOL_NAME;
use crate::backends::gemini::wire::{
    self, ContentRole, FinishReason, FunctionCall, FunctionResponse, GenerateContentRequest,
    GenerationConfig as WireGenConfig, Part, ThinkingConfig,
};
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    Step, StepSource, StepStatus, StepTarget, StepType, StreamChunk, SystemInstructions,
    ThinkingLevel, ToolCall, ToolResult, UsageMetadata,
};

/// Maximum dispatch rounds per turn. The model can loop indefinitely
/// alternating tool calls; cap to prevent runaway costs.
const MAX_TOOL_ROUNDS: u32 = 16;

#[derive(Clone)]
pub(crate) struct LoopConfig {
    pub model: String,
    pub system_instruction: Option<wire::Content>,
    pub thinking: Option<ThinkingLevel>,
    pub response_schema: Option<Value>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
    pub tool_declarations: Vec<wire::FunctionDeclaration>,
}

impl LoopConfig {
    pub fn from_system(
        model: String,
        system: Option<&SystemInstructions>,
        thinking: Option<ThinkingLevel>,
        response_schema: Option<&str>,
        tool_declarations: Vec<wire::FunctionDeclaration>,
    ) -> Result<Self> {
        let system_instruction = system.map(|s| match s {
            SystemInstructions::Custom(c) => wire::Content::system_text(c.text.clone()),
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
                wire::Content::system_text(buf.trim().to_string())
            }
        });

        let response_schema = match response_schema {
            Some(s) => Some(
                serde_json::from_str::<Value>(s)
                    .map_err(|e| Error::config(format!("response_schema not valid JSON: {e}")))?,
            ),
            None => None,
        };

        Ok(Self {
            model,
            system_instruction,
            thinking,
            response_schema,
            temperature: None,
            max_output_tokens: None,
            tool_declarations,
        })
    }
}

/// Per-connection mutable state.
pub(crate) struct LoopState {
    pub history: Mutex<Vec<wire::Content>>,
    pub idle: Arc<AtomicBool>,
    pub idle_notify: Arc<Notify>,
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

/// Convert SDK `Content` into Gemini's user-turn `Content`.
pub(crate) fn to_wire_user_content(content: Content) -> Result<wire::Content> {
    let mut parts: Vec<Part> = Vec::with_capacity(content.parts.len().max(1));
    for p in content.parts {
        match p {
            ApiPart::Text(t) => parts.push(Part::Text { text: t }),
            ApiPart::Media(m) => parts.push(Part::InlineData {
                inline_data: wire::InlineData {
                    mime_type: m.mime_type,
                    data: base64::engine::general_purpose::STANDARD.encode(m.data.as_ref()),
                },
            }),
        }
    }
    if parts.is_empty() {
        return Err(Error::config("empty content"));
    }
    Ok(wire::Content {
        role: ContentRole::User,
        parts,
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

pub(crate) async fn run_turn(deps: TurnDeps, user: wire::Content) -> Result<()> {
    deps.state.idle.store(false, Ordering::Release);
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
    let mut last_finish: Option<FinishReason> = None;
    let trajectory_id = Uuid::new_v4().to_string();

    loop {
        rounds += 1;
        if rounds > MAX_TOOL_ROUNDS {
            warn!(rounds, "exceeded MAX_TOOL_ROUNDS; forcing turn end");
            break;
        }

        let request = build_request(&deps.config, &deps.state.history.lock());
        let mut stream = match deps.client.stream_generate(&deps.config.model, &request).await {
            Ok(s) => s,
            Err(e) => {
                emit_error(&deps.state, e.to_string());
                deps.state.idle.store(true, Ordering::Release);
                deps.state.idle_notify.notify_waiters();
                return Err(e);
            }
        };

        let step_index = deps.state.alloc_step_index();
        let mut accumulated_text = String::new();
        let mut accumulated_thought = String::new();
        let mut pending_calls: Vec<FunctionCall> = Vec::new();
        let mut finish_reason: Option<FinishReason> = None;
        let mut last_usage: Option<wire::WireUsage> = None;

        while let Some(chunk_res) = stream.next().await {
            let chunk = match chunk_res {
                Ok(c) => c,
                Err(e) => {
                    emit_error(&deps.state, e.to_string());
                    deps.state.idle.store(true, Ordering::Release);
                    deps.state.idle_notify.notify_waiters();
                    return Err(e);
                }
            };

            for cand in chunk.candidates {
                if let Some(content) = cand.content {
                    for part in content.parts {
                        match part {
                            Part::Text { text } => {
                                if !text.is_empty() {
                                    accumulated_text.push_str(&text);
                                    deps.state
                                        .emit(text_delta_step(&trajectory_id, step_index, &text));
                                }
                            }
                            Part::Thought {
                                thought: true,
                                text: Some(t),
                                ..
                            } => {
                                if !t.is_empty() {
                                    accumulated_thought.push_str(&t);
                                    deps.state.emit(thought_delta_step(
                                        &trajectory_id,
                                        step_index,
                                        &t,
                                    ));
                                }
                            }
                            Part::FunctionCall { function_call } => {
                                pending_calls.push(function_call);
                            }
                            _ => {}
                        }
                    }
                }
                if let Some(reason) = cand.finish_reason {
                    finish_reason = Some(reason);
                }
            }
            if let Some(u) = chunk.usage_metadata {
                last_usage = Some(u);
            }
        }

        // Build the model-turn content (text + functionCalls) and push to history.
        let mut model_parts: Vec<Part> = Vec::new();
        if !accumulated_text.is_empty() {
            model_parts.push(Part::Text {
                text: accumulated_text.clone(),
            });
        }
        for call in &pending_calls {
            model_parts.push(Part::FunctionCall {
                function_call: call.clone(),
            });
        }
        if !model_parts.is_empty() {
            deps.state.history.lock().push(wire::Content {
                role: ContentRole::Model,
                parts: model_parts,
            });
        }

        // Accumulate usage.
        if let Some(u) = last_usage {
            let usage: UsageMetadata = u.into();
            let mut slot = deps.state.last_turn_usage.lock();
            match slot.as_mut() {
                Some(acc) => acc.accumulate(&usage),
                None => *slot = Some(usage),
            }
        }

        last_text = accumulated_text;
        last_finish = finish_reason;

        // If the model didn't call any tools, the turn is over.
        if pending_calls.is_empty() {
            break;
        }

        // Dispatch every function call. The loop continues afterwards
        // unless `finish` was called (or we've hit the cap).
        let mut response_parts: Vec<Part> = Vec::with_capacity(pending_calls.len());
        let mut saw_finish = false;
        for call in pending_calls {
            // `finish` is special: capture structured_output, mark the
            // turn complete, but still produce a function_response so
            // the model history is well-formed.
            if call.name == FINISH_TOOL_NAME {
                if let Some(out) = call.args.get("output").cloned() {
                    *deps.state.last_structured_output.lock() = Some(out);
                }
                saw_finish = true;
                response_parts.push(Part::FunctionResponse {
                    function_response: FunctionResponse {
                        name: call.name.clone(),
                        response: json!({ "ok": true }),
                    },
                });
                continue;
            }

            let tool_call = ToolCall {
                name: call.name.clone(),
                args: call.args.clone(),
                id: None,
                canonical_path: extract_canonical_path(&call.args),
            };
            deps.state.emit_chunk_step(StreamChunk::ToolCall(tool_call.clone()));

            let (decision, op_ctx) = if let Some(hooks) = deps.hook_runner.as_ref() {
                hooks.dispatch_pre_tool_call(&turn_ctx, &tool_call).await
            } else {
                (crate::types::HookResult::allow(), turn_ctx.clone())
            };

            let result_value: Value = if !decision.allow {
                json!({ "error": decision.message })
            } else if let Some(runner) = deps.tool_runner.as_ref() {
                match runner.execute(&call.name, call.args.clone()).await {
                    Ok(v) => v,
                    Err(e) => json!({ "error": e.to_string() }),
                }
            } else {
                json!({ "error": format!("no tool runner registered for '{}'", call.name) })
            };

            let post_result = ToolResult {
                name: tool_call.name.clone(),
                id: None,
                result: Some(result_value.clone()),
                error: None,
            };
            if let Some(hooks) = deps.hook_runner.as_ref() {
                hooks.dispatch_post_tool_call(&op_ctx, &post_result).await;
            }

            response_parts.push(Part::FunctionResponse {
                function_response: FunctionResponse {
                    name: call.name,
                    response: result_value,
                },
            });
        }

        // Push the function_response back into history as a user turn.
        deps.state.history.lock().push(wire::Content {
            role: ContentRole::User,
            parts: response_parts,
        });

        if saw_finish {
            break;
        }
        // Otherwise: loop and let the model react to the tool results.
    }

    // Final usage snapshot is already in last_turn_usage.
    let usage = deps.state.last_turn_usage.lock().clone().unwrap_or_default();
    let usage_opt = if usage == UsageMetadata::default() {
        None
    } else {
        Some(usage)
    };

    let (status, error_msg): (StepStatus, &str) = match last_finish {
        Some(FinishReason::Safety) => (StepStatus::Error, "stopped by safety policy"),
        Some(FinishReason::Blocklist) => (StepStatus::Error, "stopped by blocklist"),
        Some(FinishReason::ProhibitedContent) => {
            (StepStatus::Error, "stopped by prohibited-content filter")
        }
        Some(FinishReason::Recitation) => (StepStatus::Done, "stopped to avoid recitation"),
        Some(FinishReason::MaxTokens) => (StepStatus::Done, "stopped at max tokens"),
        Some(FinishReason::MalformedFunctionCall) => {
            (StepStatus::Error, "malformed function call")
        }
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

    deps.state.idle.store(true, Ordering::Release);
    deps.state.idle_notify.notify_waiters();
    debug!(?last_finish, rounds, "turn complete");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_request(config: &LoopConfig, history: &[wire::Content]) -> GenerateContentRequest {
    let thinking_config = config.thinking.map(thinking_level_to_config);
    let response_mime_type = config
        .response_schema
        .as_ref()
        .map(|_| "application/json".to_string());
    let generation_config = if thinking_config.is_some()
        || response_mime_type.is_some()
        || config.temperature.is_some()
        || config.max_output_tokens.is_some()
    {
        Some(WireGenConfig {
            thinking_config,
            response_mime_type,
            response_schema: config.response_schema.clone(),
            temperature: config.temperature,
            max_output_tokens: config.max_output_tokens,
        })
    } else {
        None
    };

    let tools = if config.tool_declarations.is_empty() {
        Vec::new()
    } else {
        vec![wire::ToolDecl {
            function_declarations: config.tool_declarations.clone(),
        }]
    };

    GenerateContentRequest {
        system_instruction: config.system_instruction.clone(),
        contents: history.to_vec(),
        tools,
        tool_config: None,
        generation_config,
    }
}

fn thinking_level_to_config(level: ThinkingLevel) -> ThinkingConfig {
    let budget = match level {
        ThinkingLevel::Minimal => 256,
        ThinkingLevel::Low => 1024,
        ThinkingLevel::Medium => 4096,
        ThinkingLevel::High => 16384,
    };
    ThinkingConfig {
        thinking_budget: budget,
        include_thoughts: Some(true),
    }
}

fn extract_canonical_path(args: &Value) -> Option<String> {
    args.get("path")
        .and_then(|v| v.as_str())
        .and_then(|s| dunce::canonicalize(s).ok())
        .map(|p| p.display().to_string())
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
        // Wrap a StreamChunk as a Step so it flows through the same
        // broadcast. Today we only do this for ToolCall — the dispatched
        // tool result is reflected in the model's next turn.
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
