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
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Notify};
use tracing::{debug, warn};
use uuid::Uuid;

use crate::backends::dispatch::{dispatch_post_turn, dispatch_tool_call, gate_pre_turn};
use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::compaction::{self, should_compact};
use crate::backends::stream_timeout::{idle_timeout_ms, next_with_idle_timeout, NextChunk};
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
    Step, StepStatus, StreamChunk, SystemInstructions, ThinkingLevel, ToolCall, UsageMetadata,
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
    /// Token threshold; when the last turn's cumulative prompt-token
    /// count exceeds this, the loop summarizes the old prefix of
    /// history (see `compaction.rs`). `None` disables.
    pub compaction_threshold: Option<u32>,
}

impl LoopConfig {
    pub fn from_system(
        model: String,
        system: Option<&SystemInstructions>,
        thinking: Option<ThinkingLevel>,
        response_schema: Option<&str>,
        tool_declarations: Vec<wire::FunctionDeclaration>,
        compaction_threshold: Option<u32>,
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
            compaction_threshold,
        })
    }
}

/// Per-connection mutable state.
pub(crate) struct LoopState {
    pub history: Mutex<Vec<wire::Content>>,
    pub idle: Arc<AtomicBool>,
    pub idle_notify: Arc<Notify>,
    /// Set by `cancel_turn` (the UI stop button). `run_turn` checks it at
    /// every loop boundary and ends the turn cleanly. Reset at turn start.
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

pub(crate) async fn run_turn(deps: TurnDeps, user: wire::Content, prompt: Content) -> Result<()> {
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
    // The model called `finish` this turn — flags the terminal step as
    // `StepType::Finish` so the in-tab loop stops auto-continuing (and
    // doesn't paint an empty-response bubble on a pure-finish turn).
    let mut finished_turn = false;
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
        // Each call rides with its `thoughtSignature` (Gemini 3.x stamps
        // functionCall parts and 400s if history echoes them back without
        // it — "Function call is missing a thought_signature").
        let mut pending_calls: Vec<(FunctionCall, Option<String>)> = Vec::new();
        let mut finish_reason: Option<FinishReason> = None;
        let mut last_usage: Option<wire::WireUsage> = None;

        // Idle-stall guard: a fresh `idle_ms` timer is armed for EACH chunk
        // (re-armed every time data arrives), so a steadily streaming response
        // never trips it — only `idle_ms` of total silence does. On a stall we
        // end the stream with an Err so the turn returns via the normal error
        // path and the one-turn guard releases (vs. hanging on a dead socket
        // that the cooperative cancel check below can never reach).
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
            // Cooperative stop: drop the rest of this streamed response.
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

            for cand in chunk.candidates {
                if let Some(content) = cand.content {
                    for part in content.parts {
                        match part {
                            Part::Text { text } => {
                                if !text.is_empty() {
                                    accumulated_text.push_str(&text);
                                    deps.state
                                        .emit(Step::text_delta(&trajectory_id, step_index, &text));
                                }
                            }
                            Part::Thought {
                                thought: true,
                                text: Some(t),
                                ..
                            } => {
                                if !t.is_empty() {
                                    accumulated_thought.push_str(&t);
                                    deps.state.emit(Step::thought_delta(
                                        &trajectory_id,
                                        step_index,
                                        &t,
                                    ));
                                }
                            }
                            // Gemini 3.x stamps EVERY part with `thought`, so a
                            // normal visible-text part arrives as
                            // `Thought { thought: false, text: Some(_) }` (see the
                            // CLAUDE.md gotcha + `mod.rs::project_history`, which
                            // already treats this as output text). Without this arm
                            // the text fell through `_ => {}` and was silently
                            // DROPPED from the live stream.
                            Part::Thought {
                                thought: false,
                                text: Some(t),
                                ..
                            } => {
                                if !t.is_empty() {
                                    accumulated_text.push_str(&t);
                                    deps.state
                                        .emit(Step::text_delta(&trajectory_id, step_index, &t));
                                }
                            }
                            Part::FunctionCall {
                                function_call,
                                thought_signature,
                            } => {
                                pending_calls.push((function_call, thought_signature));
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
        for (call, signature) in &pending_calls {
            model_parts.push(Part::FunctionCall {
                function_call: call.clone(),
                thought_signature: signature.clone(),
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
                Some(acc) => acc.merge_round(&usage),
                None => *slot = Some(usage),
            }
        }

        last_text = accumulated_text;
        last_finish = finish_reason;

        // If the model didn't call any tools, the turn is over.
        if pending_calls.is_empty() {
            break;
        }

        // Stop requested while streaming — end now instead of executing the
        // tools the model asked for (the whole point of stop is to NOT run
        // more work / burn more tokens).
        if deps.state.cancel.load(Ordering::Acquire) {
            debug!("turn cancelled before tool dispatch");
            break;
        }

        // Dispatch every function call. The loop continues afterwards
        // unless `finish` was called (or we've hit the cap).
        let mut response_parts: Vec<Part> = Vec::with_capacity(pending_calls.len());
        let mut saw_finish = false;
        for (call, _signature) in pending_calls {
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

            // The shared pipeline: pre-hooks → execute → error-lift →
            // post-hooks. The wire side always gets a JSON value (Gemini
            // needs to see errors as part of the conversation); the typed
            // ToolResult gets `error: Some(msg)` whenever execution didn't
            // produce a real result, so consumers (UI, hooks) branch cleanly.
            let post_result = dispatch_tool_call(
                deps.tool_runner.as_ref(),
                deps.hook_runner.as_ref(),
                &turn_ctx,
                &tool_call,
            )
            .await;
            let result_value = post_result.result.clone().unwrap_or(Value::Null);
            // Surface the result on the stream so UIs can flip the
            // tool block from "running" to ok/err. Until 0.7.1 this
            // emit was missing — the result panel stayed empty.
            deps.state
                .emit_chunk_step(StreamChunk::ToolResult(post_result.clone()));

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
            finished_turn = true;
            break;
        }
        // Otherwise: loop and let the model react to the tool results.
    }

    // Final usage snapshot is already in last_turn_usage.
    let usage = deps.state.last_turn_usage.lock().clone().unwrap_or_default();
    let usage_opt = if usage == UsageMetadata::default() {
        None
    } else {
        Some(usage.clone())
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

    // Post-turn hooks observe the completed turn's final text — fired after
    // the terminal step, never on denied or errored turns.
    dispatch_post_turn(deps.hook_runner.as_ref(), &turn_ctx, &last_text).await;

    // Compaction: if the turn pushed total tokens over the configured
    // threshold, summarize the old prefix of history before the next
    // turn starts. Never errors out — see compaction.rs for fallback.
    let used = usage.prompt_token_count;
    if should_compact(used, deps.config.compaction_threshold) {
        debug!(
            used,
            threshold = ?deps.config.compaction_threshold,
            "compaction triggered"
        );
        compaction::try_compact(&deps.state.history, &deps.client, &deps.config.model).await;
    }

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
    let path_str = args.get("path").and_then(|v| v.as_str())?;
    let path = std::path::Path::new(path_str);
    // Existing files / dirs: canonicalize directly.
    if let Ok(p) = dunce::canonicalize(path) {
        return Some(p.display().to_string());
    }
    // Non-existent target (e.g. create_file): canonicalize the parent
    // and join the file name so workspace_only still has something to
    // check against.
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
        // Wrap a StreamChunk as a Step so it flows through the same
        // broadcast. ToolCall AND ToolResult both surface — dropping results
        // here (the pre-0.34 behavior, despite the call-site comment claiming
        // otherwise) left every live tool block "running" with an EMPTY
        // result panel until a reload replayed it from history.
        match chunk {
            // DONE, not Active. This tool was ALREADY dispatched inline (above),
            // and the Agent's `spawn_tool_dispatcher` RE-EXECUTES any non-Done
            // registered tool-call step it sees on the broadcast — so emitting
            // Active here double-fires every tool (and re-fires it on history
            // replay). Done = "observability only, already handled", exactly the
            // contract the mock backend documents.
            StreamChunk::ToolCall(tc) => self.emit(Step::tool_call(
                self.alloc_step_index(),
                tc,
                StepStatus::Done,
            )),
            StreamChunk::ToolResult(tr) => {
                self.emit(Step::tool_result(self.alloc_step_index(), tr))
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::gemini::api::GeminiClient;
    use crate::hooks::TurnContext;
    use crate::types::{HookResult, StepSource};

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

    /// THE history-pollution invariant: a turn denied by a `PreTurnHook` must
    /// (1) never push the prompt into wire history, (2) never call the model
    /// (this test is OFFLINE — a real request would error differently),
    /// (3) emit a System/Error `turn_error` Step carrying
    /// `"turn denied by hook: {reason}"`, and (4) release the one-turn idle
    /// guard.
    #[tokio::test]
    async fn pre_turn_deny_keeps_prompt_out_of_history() {
        let (tx, mut rx) = broadcast::channel::<Step>(8);
        let state = Arc::new(LoopState::new(tx));
        let hooks = Arc::new(HookRunner::new());
        hooks.register_pre_turn(Arc::new(DenyAllTurns));

        let deps = TurnDeps {
            client: Arc::new(GeminiClient::new("offline-test-key").expect("client builds")),
            config: LoopConfig::from_system(
                "gemini-test".into(),
                None,
                None,
                None,
                Vec::new(),
                None,
            )
            .expect("config builds"),
            state: state.clone(),
            tool_runner: None,
            hook_runner: Some(hooks),
            session_ctx: None,
        };

        let prompt = Content::text("a prompt that must never reach history");
        let user = to_wire_user_content(prompt.clone()).expect("wire content");
        let err = run_turn(deps, user, prompt)
            .await
            .expect_err("a denied turn returns Err");
        assert!(
            err.to_string().contains("turn denied by hook: nope"),
            "deny reason must surface, got: {err}"
        );

        assert!(
            state.history.lock().is_empty(),
            "the denied prompt must NOT enter wire history"
        );
        assert!(
            state.idle.load(Ordering::Acquire),
            "the idle guard must release after a denied turn"
        );

        // The deny surfaced as the System/Error turn_error shape that
        // `subscribe_step_stream` translates into a stream `Err`.
        let step = rx.recv().await.expect("a step was broadcast");
        assert_eq!(step.source, StepSource::System);
        assert_eq!(step.status, StepStatus::Error);
        assert!(step.error.contains("turn denied by hook: nope"));
    }

    /// REGRESSION: the inline-dispatched tool's observability step MUST be Done,
    /// not Active. The Agent's `spawn_tool_dispatcher` RE-EXECUTES any non-Done
    /// registered tool-call step it sees on the broadcast — so an Active step
    /// double-fires every tool (gemini already dispatched it inline) and
    /// re-fires it on history replay. Pins the Done contract the mock documents.
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
        assert!(!step.tool_calls.is_empty(), "it carries the tool call");
        assert_eq!(
            step.status,
            StepStatus::Done,
            "inline-dispatched tool-call step must be Done; Active makes \
             spawn_tool_dispatcher re-execute the tool",
        );
    }
}
