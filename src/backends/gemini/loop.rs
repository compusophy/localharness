//! Agent loop for the Gemini backend.
//!
//! Phase 1: text-only. Receive a user prompt, send it to Gemini, stream
//! the response back as [`Step`]s, persist the assistant turn into the
//! conversation history. No tool dispatch yet; tools land in Phase 2.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::atomic::AtomicU32;

use futures_util::stream::StreamExt;
use parking_lot::Mutex;
use tokio::sync::{broadcast, Notify};
use tracing::{debug, warn};

use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::wire::{
    self, ContentRole, FinishReason, GenerateContentRequest,
    GenerationConfig as WireGenConfig, Part, ThinkingConfig,
};
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::types::{
    Step, StepSource, StepStatus, StepTarget, StepType, SystemInstructions, ThinkingLevel,
    UsageMetadata,
};

/// Configuration the loop needs on every turn. Cloned into the spawned
/// task — keep cheap.
#[derive(Clone)]
pub(crate) struct LoopConfig {
    pub model: String,
    pub system_instruction: Option<wire::Content>,
    pub thinking: Option<ThinkingLevel>,
    pub response_schema: Option<serde_json::Value>,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<u32>,
}

impl LoopConfig {
    pub fn from_system(
        model: String,
        system: Option<&SystemInstructions>,
        thinking: Option<ThinkingLevel>,
        response_schema: Option<&str>,
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
                serde_json::from_str::<serde_json::Value>(s)
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
        })
    }
}

/// Per-connection turn machinery. Held inside `GeminiConnection`.
pub(crate) struct LoopState {
    pub history: Mutex<Vec<wire::Content>>,
    pub idle: Arc<AtomicBool>,
    pub idle_notify: Arc<Notify>,
    pub steps: broadcast::Sender<Step>,
    pub next_step_index: AtomicU32,
    pub last_turn_usage: Mutex<Option<UsageMetadata>>,
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
        }
    }

    fn alloc_step_index(&self) -> u32 {
        self.next_step_index.fetch_add(1, Ordering::Relaxed)
    }

    fn emit(&self, step: Step) {
        // `send` returns Err when there are no subscribers; expected when
        // a turn happens before anyone called `subscribe_steps`.
        let _ = self.steps.send(step);
    }
}

/// Convert SDK `Content` into Gemini's user-turn `Content`.
///
/// Phase 1 supports text and inline media. Media is base64-encoded
/// inline using the existing `Bytes`-backed payload (no copy beyond the
/// base64 expansion).
pub(crate) fn to_wire_user_content(content: Content) -> Result<wire::Content> {
    use base64::Engine;
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

/// Run one turn against Gemini. Streams chunks into `state.steps` and
/// returns when the turn ends. Phase-1: no tool dispatch.
pub(crate) async fn run_turn(
    client: SharedClient,
    config: LoopConfig,
    state: Arc<LoopState>,
    user: wire::Content,
) -> Result<()> {
    // Mark busy, record user turn, reset per-turn counters.
    state.idle.store(false, Ordering::Release);
    {
        let mut hist = state.history.lock();
        hist.push(user);
    }
    *state.last_turn_usage.lock() = Some(UsageMetadata::default());

    let request = build_request(&config, &state.history.lock());
    let mut stream = match client.stream_generate(&config.model, &request).await {
        Ok(s) => s,
        Err(e) => {
            mark_idle_with_error(&state, e.to_string());
            return Err(e);
        }
    };

    let step_index = state.alloc_step_index();
    let trajectory_id = uuid::Uuid::new_v4().to_string();
    let mut accumulated_text = String::new();
    let mut accumulated_thought = String::new();
    let mut finish_reason: Option<FinishReason> = None;
    let mut last_usage: Option<wire::WireUsage> = None;

    while let Some(chunk_res) = stream.next().await {
        let chunk = match chunk_res {
            Ok(c) => c,
            Err(e) => {
                mark_idle_with_error(&state, e.to_string());
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
                                state.emit(text_delta_step(
                                    &trajectory_id,
                                    step_index,
                                    &text,
                                ));
                            }
                        }
                        Part::Thought { thought: true, text: Some(t), .. } => {
                            if !t.is_empty() {
                                accumulated_thought.push_str(&t);
                                state.emit(thought_delta_step(
                                    &trajectory_id,
                                    step_index,
                                    &t,
                                ));
                            }
                        }
                        Part::FunctionCall { .. } => {
                            // Phase 2: route through tool dispatcher.
                            warn!("function call received but tool dispatch is not yet implemented");
                        }
                        // FunctionResponse comes from us, not the server.
                        // InlineData / Thought-without-text: ignore here.
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

    // Persist the assistant turn into history.
    if !accumulated_text.is_empty() {
        let model_content = wire::Content {
            role: ContentRole::Model,
            parts: vec![Part::Text { text: accumulated_text.clone() }],
        };
        state.history.lock().push(model_content);
    }

    // Final usage snapshot.
    let usage: UsageMetadata = last_usage.map(Into::into).unwrap_or_default();
    if usage != UsageMetadata::default() {
        *state.last_turn_usage.lock() = Some(usage.clone());
    }

    // Terminal step.
    let terminal_status = match finish_reason {
        Some(FinishReason::Stop) | None => StepStatus::Done,
        Some(FinishReason::Safety | FinishReason::Blocklist | FinishReason::ProhibitedContent) => {
            StepStatus::Error
        }
        Some(_) => StepStatus::Done,
    };
    let terminal_error = match finish_reason {
        Some(FinishReason::Safety) => "stopped by safety policy",
        Some(FinishReason::Blocklist) => "stopped by blocklist",
        Some(FinishReason::ProhibitedContent) => "stopped by prohibited-content filter",
        Some(FinishReason::Recitation) => "stopped to avoid recitation",
        Some(FinishReason::MaxTokens) => "stopped at max tokens",
        Some(FinishReason::MalformedFunctionCall) => "malformed function call",
        _ => "",
    };

    let terminal = Step {
        id: trajectory_id.clone(),
        step_index,
        kind: StepType::TextResponse,
        source: StepSource::Model,
        target: StepTarget::User,
        status: terminal_status,
        content: accumulated_text,
        content_delta: String::new(),
        thinking: accumulated_thought,
        thinking_delta: String::new(),
        tool_calls: Vec::new(),
        error: terminal_error.to_string(),
        is_complete_response: Some(true),
        structured_output: None,
        usage_metadata: if usage == UsageMetadata::default() {
            None
        } else {
            Some(usage)
        },
    };
    state.emit(terminal);

    state.idle.store(true, Ordering::Release);
    state.idle_notify.notify_waiters();
    debug!(?finish_reason, "turn complete");
    Ok(())
}

fn mark_idle_with_error(state: &LoopState, message: String) {
    state.idle.store(true, Ordering::Release);
    state.idle_notify.notify_waiters();
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

    GenerateContentRequest {
        system_instruction: config.system_instruction.clone(),
        contents: history.to_vec(),
        tools: Vec::new(),
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
