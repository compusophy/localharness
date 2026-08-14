//! Agent loop for the Gemini backend — the always-on DEFAULT path, migrated
//! onto the shared [`crate::backends::turn_engine`] (R7 phase 3, last).
//!
//! The turn scaffold (idle/cancel atomics, pre-turn gate, retry-wrapped
//! stream open, idle-stall arm, MAX_TOOL_ROUNDS, the finish-tool special
//! case, usage folding, terminal step, compaction trigger) lives in the
//! engine; this module supplies only the Gemini-specific wire behavior via
//! [`GeminiProvider`]:
//!
//! - Stream fold: Gemini 3.x stamps EVERY part with `thought`, so a normal
//!   visible-text part arrives as `Thought { thought: false, text: Some(_) }`
//!   and MUST be folded as text (without that arm the model's output was
//!   silently dropped from the live stream); `thought: true` parts are
//!   reasoning deltas. Each `functionCall` part rides with its
//!   `thoughtSignature`, captured and echoed back VERBATIM into the persisted
//!   model turn (3.x 400s replayed history missing it — "Function call is
//!   missing a thought_signature").
//! - Tool args arrive PARSED (`FunctionCall.args` is a JSON `Value`, not a
//!   streamed string fragment), so `ResolvedCall.parse_error` is always
//!   `None` — the engine's malformed-args skip path never fires on this wire.
//! - Correlation is by NAME (`id: None`): a `functionResponse` carries no
//!   call id (unlike openai/anthropic).
//! - Tool results: ONE batched `user`-role `Content` of `functionResponse`
//!   parts (like anthropic; unlike openai's one message per call).
//! - Control-flow hooks: engine DEFAULTS — Gemini has no `pause_turn`, and
//!   its wire tolerates a cancelled turn's dangling `functionCall` (no #82
//!   balancing needed).

use std::sync::Arc;

use base64::Engine as _;
use serde_json::Value;

use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::compaction;
use crate::backends::gemini::wire::{
    self, ContentRole, FinishReason, FunctionCall, FunctionResponse, GenerateChunk,
    GenerateContentRequest, GenerationConfig as WireGenConfig, Part, ThinkingConfig,
};
use crate::backends::turn_engine::{
    self, DispatchedResult, EmitCtx, EngineDeps, ResolvedCall, TurnProvider,
};
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{StepStatus, SystemInstructions, ThinkingLevel, UsageMetadata};

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
    /// Behavioral note appended to every compaction summary turn (#80).
    pub compaction_epilogue: Option<String>,
}

impl LoopConfig {
    pub fn from_system(
        model: String,
        system: Option<&SystemInstructions>,
        thinking: Option<ThinkingLevel>,
        response_schema: Option<&str>,
        tool_declarations: Vec<wire::FunctionDeclaration>,
        compaction_threshold: Option<u32>,
        compaction_epilogue: Option<String>,
    ) -> Result<Self> {
        let system_instruction = system
            .map(|s| wire::Content::system_text(crate::backends::render_system(s)));

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
            compaction_epilogue,
        })
    }
}

/// Per-connection mutable state — the shared generic container specialised to
/// Gemini's wire-history shape (`Vec<wire::Content>`).
pub(crate) type LoopState = crate::backends::state::LoopState<wire::Content>;

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

/// One round's stream accumulators (the [`TurnProvider::Accum`]).
#[derive(Default)]
pub(crate) struct RoundAccum {
    /// FunctionCall parts in wire order. Each call rides with its
    /// `thoughtSignature` (Gemini 3.x stamps functionCall parts and 400s if
    /// history echoes them back without it) — kept HERE, not just as
    /// `ResolvedCall`s, so `assemble_assistant_message` can echo every
    /// signature back verbatim.
    pending_calls: Vec<(FunctionCall, Option<String>)>,
    finish_reason: Option<FinishReason>,
    /// A PROMPT-level block (`promptFeedback.blockReason`). It arrives WITHOUT
    /// a candidate, so there is no `finishReason` to map — without this the
    /// round ended with an empty note and read as a blank turn.
    prompt_block: Option<wire::BlockReason>,
    /// A chunk's `usageMetadata` is cumulative for the round —
    /// last-writer-wins (matches the pre-engine loop exactly).
    usage: Option<wire::WireUsage>,
}

/// The Gemini side of the [`TurnProvider`] seam — a zero-sized marker the
/// engine is monomorphized over (static dispatch; see `turn_engine`).
pub(crate) struct GeminiProvider;

impl TurnProvider for GeminiProvider {
    type Message = wire::Content;
    type Config = LoopConfig;
    type Request = GenerateContentRequest;
    type Event = GenerateChunk;
    type Accum = RoundAccum;

    fn build_request(config: &LoopConfig, history: &[wire::Content]) -> GenerateContentRequest {
        build_request(config, history)
    }

    fn compaction_threshold(config: &LoopConfig) -> Option<u32> {
        config.compaction_threshold
    }

    fn fold_event(
        acc: &mut RoundAccum,
        ctx: &mut EmitCtx<'_, wire::Content>,
        chunk: GenerateChunk,
    ) -> Result<()> {
        for cand in chunk.candidates {
            if let Some(content) = cand.content {
                for part in content.parts {
                    match part {
                        Part::Text { text } => ctx.push_text(&text),
                        Part::Thought {
                            thought: true,
                            text: Some(t),
                            ..
                        } => ctx.push_thought(&t),
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
                        } => ctx.push_text(&t),
                        Part::FunctionCall {
                            function_call,
                            thought_signature,
                        } => {
                            acc.pending_calls.push((function_call, thought_signature));
                        }
                        _ => {}
                    }
                }
            }
            if let Some(reason) = cand.finish_reason {
                acc.finish_reason = Some(reason);
            }
        }
        // A PROMPT block: only a `blockReason` means blocked — a
        // `promptFeedback` carrying just ratings (or UNSPECIFIED) is not a
        // block and must stay invisible.
        if let Some(reason) = chunk.prompt_feedback.and_then(|f| f.block_reason) {
            if reason != wire::BlockReason::BlockReasonUnspecified {
                acc.prompt_block = Some(reason);
            }
        }
        if let Some(u) = chunk.usage_metadata {
            acc.usage = Some(u);
        }
        Ok(())
    }

    /// Gemini's tool args arrive PARSED (`FunctionCall.args` is a JSON
    /// `Value`), so `parse_error` is always `None` — the engine's
    /// malformed-args skip path never fires on this wire. `id` is `None`:
    /// Gemini correlates results by NAME (a `functionResponse` carries no
    /// call id). The accumulator is NOT drained — the originals (with their
    /// signatures) are still needed by `assemble_assistant_message`.
    fn resolve_pending_calls(acc: &mut RoundAccum) -> Vec<ResolvedCall> {
        acc.pending_calls
            .iter()
            .map(|(call, _signature)| ResolvedCall {
                id: None,
                name: call.name.clone(),
                args: call.args.clone(),
                parse_error: None,
            })
            .collect()
    }

    fn round_usage(acc: &RoundAccum) -> UsageMetadata {
        acc.usage.clone().map(Into::into).unwrap_or_default()
    }

    /// Every content-block wording here MUST carry a token
    /// `turn_flow::classify_empty` reads (safety / blocklist / prohibited /
    /// recitation / "content filter"), or the turn surfaces as `Blank` and
    /// tells the user to check their balance for what was a content block.
    /// Guard: `every_content_block_reason_classifies_as_blocked`.
    fn map_finish_reason(acc: &RoundAccum) -> (StepStatus, String) {
        // A PROMPT block preempts the candidate finish reason: Google blocks
        // the input and returns NO candidate, so there is no `finishReason` —
        // this is the only note the turn will ever get.
        if let Some(reason) = acc.prompt_block {
            let note = match reason {
                wire::BlockReason::Safety => "prompt blocked by safety policy",
                wire::BlockReason::Blocklist => "prompt blocked by blocklist",
                wire::BlockReason::ProhibitedContent => {
                    "prompt blocked by prohibited-content filter"
                }
                wire::BlockReason::ImageSafety => "prompt blocked by image-safety filter",
                // OTHER / an unknown reason — still a block, named honestly
                // without inventing a cause. (UNSPECIFIED never gets here; the
                // fold treats it as "not blocked".)
                _ => "prompt blocked by content filter",
            };
            return (StepStatus::Error, note.to_string());
        }
        let (status, note) = match acc.finish_reason {
            Some(FinishReason::Safety) => (StepStatus::Error, "stopped by safety policy"),
            Some(FinishReason::Blocklist) => (StepStatus::Error, "stopped by blocklist"),
            Some(FinishReason::ProhibitedContent) => {
                (StepStatus::Error, "stopped by prohibited-content filter")
            }
            Some(FinishReason::Recitation) => (StepStatus::Done, "stopped to avoid recitation"),
            // SPII = sensitive personally identifiable information. Both of
            // these are content blocks like the arms above; without an arm they
            // fell through `_` to an EMPTY note and the turn read as blank.
            Some(FinishReason::Spii) => (
                StepStatus::Error,
                "stopped by sensitive-personal-info content filter",
            ),
            Some(FinishReason::Language) => (
                StepStatus::Error,
                "stopped by unsupported-language content filter",
            ),
            Some(FinishReason::ImageSafety) => {
                (StepStatus::Error, "stopped by image-safety content filter")
            }
            Some(FinishReason::MaxTokens) => (StepStatus::Done, "stopped at max tokens"),
            Some(FinishReason::MalformedFunctionCall) => {
                (StepStatus::Error, "malformed function call")
            }
            _ => (StepStatus::Done, ""),
        };
        (status, note.to_string())
    }

    /// Build the model-turn content (text + functionCalls). Every
    /// functionCall part echoes its captured `thoughtSignature` VERBATIM —
    /// 3.x rejects replayed history missing it.
    fn assemble_assistant_message(
        acc: RoundAccum,
        text: &str,
        _calls: &[ResolvedCall],
    ) -> Option<wire::Content> {
        let mut parts: Vec<Part> = Vec::new();
        if !text.is_empty() {
            parts.push(Part::Text {
                text: text.to_string(),
            });
        }
        for (call, signature) in acc.pending_calls {
            parts.push(Part::FunctionCall {
                function_call: call,
                thought_signature: signature,
            });
        }
        (!parts.is_empty()).then_some(wire::Content {
            role: ContentRole::Model,
            parts,
        })
    }

    /// ONE batched `user`-role content of `functionResponse` parts, matched
    /// by NAME (like anthropic's batched turn; unlike openai's one message
    /// per call).
    fn tool_result_messages(results: Vec<DispatchedResult>) -> Vec<wire::Content> {
        if results.is_empty() {
            return Vec::new();
        }
        let parts: Vec<Part> = results
            .into_iter()
            .map(|r| Part::FunctionResponse {
                function_response: FunctionResponse {
                    name: r.call.name,
                    response: r.value,
                },
            })
            .collect();
        vec![wire::Content {
            role: ContentRole::User,
            parts,
        }]
    }
}

/// Drive one turn through the shared engine, plugging in the Gemini client
/// for the stream open and the compaction fold (the engine stays
/// client-agnostic — the async edges ride in as closures, exactly like the
/// compaction engine's `summarize`).
pub(crate) async fn run_turn(deps: TurnDeps, user: wire::Content, prompt: Content) -> Result<()> {
    let TurnDeps {
        client,
        config,
        state,
        tool_runner,
        hook_runner,
        session_ctx,
    } = deps;
    // The per-turn model (send() may have applied the difficulty-router
    // override to this clone) — Gemini's model rides the URL path, not the
    // request body, so the open closure needs it alongside the request.
    let model = config.model.clone();
    let compact_epilogue = config.compaction_epilogue.clone();
    let engine_deps = EngineDeps::<GeminiProvider> {
        config,
        state: state.clone(),
        tool_runner,
        hook_runner,
        session_ctx,
    };
    let open_client = client.clone();
    let open_model = model.clone();
    turn_engine::run_turn::<GeminiProvider, _, _, _, _, _>(
        engine_deps,
        user,
        prompt,
        move |req: GenerateContentRequest| {
            let client = open_client.clone();
            let model = open_model.clone();
            async move { client.stream_generate(&model, &req).await }
        },
        move || async move {
            compaction::try_compact(&state.history, &client, &model, compact_epilogue.as_deref())
                .await;
        },
    )
    .await
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::gemini::api::GeminiClient;
    use crate::backends::gemini::wire::Candidate;
    use crate::hooks::TurnContext;
    use crate::types::{HookResult, Step, StepSource, StreamChunk};
    use serde_json::json;
    use std::sync::atomic::Ordering;
    use tokio::sync::broadcast;

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

    /// THE Gemini-specific fold contract, exercised through the provider's
    /// real seam (the path the engine drives): `thought:false` text parts
    /// fold as VISIBLE text (the silently-dropped-output fix),
    /// `thought:true` parts as reasoning; each functionCall's
    /// `thoughtSignature` is captured; args arrive parsed (`parse_error`
    /// always None, id None — Gemini correlates by name); usage is
    /// last-writer-wins per round.
    #[test]
    fn provider_fold_handles_thought_stamped_parts_and_signature_capture() {
        let chunk = |parts: Vec<Part>| GenerateChunk {
            candidates: vec![Candidate {
                content: Some(wire::Content {
                    role: ContentRole::Model,
                    parts,
                }),
                finish_reason: None,
                index: None,
            }],
            ..Default::default()
        };

        let (tx, mut rx) = broadcast::channel::<Step>(16);
        let state = LoopState::new(tx);
        let mut acc = RoundAccum::default();
        turn_engine::test_fold_events::<GeminiProvider>(
            &state,
            &mut acc,
            vec![
                // Reasoning (thought: true) — a thought delta, NOT visible text.
                chunk(vec![Part::Thought {
                    thought: true,
                    text: Some("reasoning".into()),
                    thought_signature: None,
                }]),
                // The 3.x stamp: visible text arrives as thought:false.
                chunk(vec![Part::Thought {
                    thought: false,
                    text: Some("visible".into()),
                    thought_signature: None,
                }]),
                // A functionCall stamped with its thoughtSignature.
                chunk(vec![Part::FunctionCall {
                    function_call: FunctionCall {
                        name: "view_file".into(),
                        args: json!({"path": "a.rs"}),
                    },
                    thought_signature: Some("AbC123=".into()),
                }]),
                // Terminal usage + finish reason.
                GenerateChunk {
                    candidates: vec![Candidate {
                        content: None,
                        finish_reason: Some(FinishReason::ToolUse),
                        index: None,
                    }],
                    usage_metadata: Some(wire::WireUsage {
                        prompt_token_count: Some(10),
                        candidates_token_count: Some(5),
                        total_token_count: Some(15),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            ],
        );

        // thought:false text streamed as a TEXT delta; thought:true as thought.
        let mut text_deltas = String::new();
        let mut thought_deltas = String::new();
        while let Ok(s) = rx.try_recv() {
            text_deltas.push_str(&s.content_delta);
            thought_deltas.push_str(&s.thinking_delta);
        }
        assert_eq!(text_deltas, "visible", "thought:false parts are VISIBLE text");
        assert_eq!(thought_deltas, "reasoning");

        let calls = GeminiProvider::resolve_pending_calls(&mut acc);
        assert_eq!(calls.len(), 1);
        assert!(calls[0].id.is_none(), "Gemini correlates by name, not id");
        assert_eq!(calls[0].name, "view_file");
        assert_eq!(calls[0].args, json!({"path": "a.rs"}), "args arrive parsed");
        assert!(calls[0].parse_error.is_none(), "parse_error is always None");

        assert_eq!(acc.finish_reason, Some(FinishReason::ToolUse));
        let usage = GeminiProvider::round_usage(&acc);
        assert_eq!(usage.total_token_count, Some(15));

        // The persisted model turn echoes the signature VERBATIM (text first).
        let msg = GeminiProvider::assemble_assistant_message(acc, "visible", &calls)
            .expect("model turn assembled");
        assert_eq!(msg.role, ContentRole::Model);
        assert!(matches!(&msg.parts[0], Part::Text { text } if text == "visible"));
        match &msg.parts[1] {
            Part::FunctionCall {
                function_call,
                thought_signature,
            } => {
                assert_eq!(function_call.name, "view_file");
                assert_eq!(
                    thought_signature.as_deref(),
                    Some("AbC123="),
                    "thoughtSignature must be echoed verbatim or 3.x 400s the replay"
                );
            }
            other => panic!("expected FunctionCall part, got {other:?}"),
        }
    }

    /// The OUTCOME half of the blocked-frame fix: the REAL captured frame
    /// (TB full-set 2026-08-01, `dna-assembly`) folds through the provider
    /// seam into a NAMED blocked stop — terminal step status `Error` with
    /// "stopped by prohibited-content filter", which `turn_flow::classify_empty`
    /// reads as `Blocked` (terminal; rephrase, don't retry). Nothing streamed,
    /// so no model turn is persisted. Before the wire fix this frame never got
    /// this far — the chunk decode failed and the turn read as an infra crash.
    #[test]
    fn blocked_frame_folds_into_a_named_blocked_stop() {
        let chunk: GenerateChunk =
            serde_json::from_str(wire::BLOCKED_FRAME_JSON).expect("frame decodes");
        let (tx, _rx) = broadcast::channel::<Step>(8);
        let state = LoopState::new(tx);
        let mut acc = RoundAccum::default();
        turn_engine::test_fold_events::<GeminiProvider>(&state, &mut acc, vec![chunk]);

        assert_eq!(acc.finish_reason, Some(FinishReason::ProhibitedContent));
        let (status, note) = GeminiProvider::map_finish_reason(&acc);
        assert_eq!(status, StepStatus::Error);
        assert_eq!(note, "stopped by prohibited-content filter");
        assert_eq!(
            crate::turn_flow::classify_empty(Some(note.as_str()), false),
            crate::turn_flow::EmptyKind::Blocked,
            "the terminal note must classify as a content block, not a blank turn"
        );
        assert!(
            GeminiProvider::assemble_assistant_message(acc, "", &[]).is_none(),
            "a blocked round streamed nothing — persist no model turn"
        );
    }

    /// Fold a frame through the provider seam and return its terminal
    /// `(status, note)` — the shared body of the block-classification tests.
    fn fold_frame(json: &str) -> (RoundAccum, (StepStatus, String)) {
        let chunk: GenerateChunk = serde_json::from_str(json).expect("frame decodes");
        let (tx, _rx) = broadcast::channel::<Step>(8);
        let state = LoopState::new(tx);
        let mut acc = RoundAccum::default();
        turn_engine::test_fold_events::<GeminiProvider>(&state, &mut acc, vec![chunk]);
        let status = GeminiProvider::map_finish_reason(&acc);
        (acc, status)
    }

    /// A PROMPT-level block: Google blocks the INPUT, so the frame carries
    /// `promptFeedback.blockReason` and NO candidate — there is no
    /// `finishReason` to map. Unmodelled, the frame decoded, folded to
    /// nothing, and the turn ended with an EMPTY note, which
    /// `turn_flow::classify_empty` reads as `Blank`: "check your
    /// session/balance" for what was a content block (our failure, their bill
    /// questioned). It must name the block instead.
    #[test]
    fn prompt_blocked_frame_folds_into_a_named_blocked_stop() {
        let (acc, (status, note)) = fold_frame(wire::PROMPT_BLOCKED_FRAME_JSON);

        assert_eq!(acc.finish_reason, None, "a prompt block has no candidate");
        assert_eq!(status, StepStatus::Error);
        assert_eq!(note, "prompt blocked by safety policy");
        assert_eq!(
            crate::turn_flow::classify_empty(Some(note.as_str()), false),
            crate::turn_flow::EmptyKind::Blocked,
            "a prompt block must NOT read as a blank turn (credits/session)"
        );
        assert!(
            GeminiProvider::assemble_assistant_message(acc, "", &[]).is_none(),
            "a blocked prompt streamed nothing — persist no model turn"
        );
    }

    /// `promptFeedback` WITHOUT a `blockReason` (ratings only) is not a block —
    /// it must stay invisible, exactly as before the field was modelled.
    #[test]
    fn prompt_feedback_without_a_block_reason_is_not_a_block() {
        let (_, (status, note)) = fold_frame(
            r#"{"candidates":[{"content":{"parts":[{"text":"hi"}],"role":"model"},"finishReason":"STOP"}],"promptFeedback":{"safetyRatings":[{"category":"HARM_CATEGORY_HARASSMENT","probability":"NEGLIGIBLE"}]}}"#,
        );
        assert_eq!(status, StepStatus::Done);
        assert_eq!(note, "", "ratings alone are not a block");
    }

    /// SPII (sensitive personally identifiable information) — defined in the
    /// wire enum but with NO arm in `map_finish_reason`, so it fell through the
    /// catch-all to an empty note and the turn read as `Blank`.
    #[test]
    fn spii_finish_reason_folds_into_a_named_blocked_stop() {
        let (acc, (status, note)) = fold_frame(
            r#"{"candidates": [{"content": {},"finishReason": "SPII","index": 0}],"usageMetadata": {"promptTokenCount": 512,"totalTokenCount": 512},"modelVersion": "gemini-3.7-flash"}"#,
        );

        assert_eq!(acc.finish_reason, Some(FinishReason::Spii));
        assert_eq!(status, StepStatus::Error);
        assert_eq!(note, "stopped by sensitive-personal-info content filter");
        assert_eq!(
            crate::turn_flow::classify_empty(Some(note.as_str()), false),
            crate::turn_flow::EmptyKind::Blocked,
            "an SPII stop is a content block, not a blank turn"
        );
    }

    /// LANGUAGE — same gap as SPII: in the enum, missing from the map.
    #[test]
    fn language_finish_reason_folds_into_a_named_blocked_stop() {
        let (acc, (status, note)) = fold_frame(
            r#"{"candidates": [{"content": {},"finishReason": "LANGUAGE","index": 0}],"usageMetadata": {"promptTokenCount": 88,"totalTokenCount": 88},"modelVersion": "gemini-3.7-flash"}"#,
        );

        assert_eq!(acc.finish_reason, Some(FinishReason::Language));
        assert_eq!(status, StepStatus::Error);
        assert_eq!(note, "stopped by unsupported-language content filter");
        assert_eq!(
            crate::turn_flow::classify_empty(Some(note.as_str()), false),
            crate::turn_flow::EmptyKind::Blocked,
            "an unsupported-language stop is a content block, not a blank turn"
        );
    }

    /// DRIFT GUARD — this class of bug has now landed three times (gemini
    /// RECITATION, openai content_filter, gemini SPII/LANGUAGE): a terminal
    /// content-block reason with no arm silently becomes an empty note and the
    /// user is told to check their balance. Every block reason on this wire —
    /// candidate-level AND prompt-level — must produce a note that
    /// `classify_empty` reads as `Blocked`.
    ///
    /// ⛔ The tripwire is the two `match`es below, NOT the arrays: they have no
    /// `_` arm, so adding a variant to `wire::FinishReason` or
    /// `wire::BlockReason` is a COMPILE ERROR here until someone declares
    /// whether it blocks. (An earlier version iterated hand-written arrays
    /// only, which left a new variant silently uncovered while this very
    /// comment claimed otherwise — the review caught it, and IMAGE_SAFETY was
    /// exactly the variant slipping through.) When the compiler stops you,
    /// declare the variant in the match AND add it to the array below it so it
    /// is actually exercised.
    #[test]
    fn every_content_block_reason_classifies_as_blocked() {
        /// Does this candidate reason mean "the model's output was blocked"?
        /// Exhaustive on purpose — see the tripwire note above.
        fn is_content_block(r: FinishReason) -> bool {
            match r {
                FinishReason::Safety
                | FinishReason::Blocklist
                | FinishReason::ProhibitedContent
                | FinishReason::Recitation
                | FinishReason::Spii
                | FinishReason::Language
                | FinishReason::ImageSafety => true,
                FinishReason::Stop
                | FinishReason::MaxTokens
                | FinishReason::ToolUse
                | FinishReason::Other
                | FinishReason::MalformedFunctionCall
                | FinishReason::FinishReasonUnspecified
                | FinishReason::Unknown => false,
            }
        }
        /// Every `BlockReason` except UNSPECIFIED is a block (the fold never
        /// records UNSPECIFIED as one). Exhaustive for the same reason.
        fn prompt_reason_blocks(r: wire::BlockReason) -> bool {
            match r {
                wire::BlockReason::Safety
                | wire::BlockReason::Blocklist
                | wire::BlockReason::ProhibitedContent
                | wire::BlockReason::ImageSafety
                | wire::BlockReason::Other
                | wire::BlockReason::Unknown => true,
                wire::BlockReason::BlockReasonUnspecified => false,
            }
        }

        for reason in [
            FinishReason::Safety,
            FinishReason::Blocklist,
            FinishReason::ProhibitedContent,
            FinishReason::Recitation,
            FinishReason::Spii,
            FinishReason::Language,
            FinishReason::ImageSafety,
        ] {
            assert!(is_content_block(reason), "{reason:?} is listed as a block");
            let acc = RoundAccum {
                finish_reason: Some(reason),
                ..Default::default()
            };
            let (_, note) = GeminiProvider::map_finish_reason(&acc);
            assert!(!note.is_empty(), "{reason:?} must carry a note");
            assert_eq!(
                crate::turn_flow::classify_empty(Some(note.as_str()), false),
                crate::turn_flow::EmptyKind::Blocked,
                "{reason:?} → {note:?} must classify as Blocked, not Blank"
            );
        }
        for reason in [
            wire::BlockReason::Safety,
            wire::BlockReason::Blocklist,
            wire::BlockReason::ProhibitedContent,
            wire::BlockReason::ImageSafety,
            wire::BlockReason::Other,
            wire::BlockReason::Unknown,
        ] {
            assert!(prompt_reason_blocks(reason), "{reason:?} is listed as a block");
            let acc = RoundAccum {
                prompt_block: Some(reason),
                ..Default::default()
            };
            let (_, note) = GeminiProvider::map_finish_reason(&acc);
            assert!(!note.is_empty(), "{reason:?} must carry a note");
            assert_eq!(
                crate::turn_flow::classify_empty(Some(note.as_str()), false),
                crate::turn_flow::EmptyKind::Blocked,
                "{reason:?} → {note:?} must classify as Blocked, not Blank"
            );
        }
    }

    /// The engine hands every dispatched result back for wire-shaping: ONE
    /// batched user turn of functionResponse parts, correlated by NAME (a
    /// functionResponse carries no call id). A thought-only round (no text,
    /// no calls) persists NO model turn.
    #[test]
    fn tool_results_batch_into_one_user_turn_and_empty_round_persists_nothing() {
        let mk = |name: &str, value: Value| DispatchedResult {
            call: ResolvedCall {
                id: None,
                name: name.into(),
                args: json!({}),
                parse_error: None,
            },
            value,
            is_error: false,
        };
        let msgs = GeminiProvider::tool_result_messages(vec![
            mk("view_file", json!({"contents": "fn main() {}"})),
            mk("finish", json!({"ok": true})),
        ]);
        assert_eq!(msgs.len(), 1, "one batched user turn");
        assert_eq!(msgs[0].role, ContentRole::User);
        assert_eq!(msgs[0].parts.len(), 2);
        match &msgs[0].parts[0] {
            Part::FunctionResponse { function_response } => {
                assert_eq!(function_response.name, "view_file");
                assert_eq!(function_response.response["contents"], "fn main() {}");
            }
            other => panic!("expected FunctionResponse, got {other:?}"),
        }

        // Nothing streamed → no model turn pushed (the old loop's
        // `!model_parts.is_empty()` guard).
        let msg =
            GeminiProvider::assemble_assistant_message(RoundAccum::default(), "", &[]);
        assert!(msg.is_none());
    }
}
