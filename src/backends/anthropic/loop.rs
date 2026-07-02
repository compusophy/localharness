//! Agent loop for the Anthropic (Claude Messages API) backend — on the shared
//! [`crate::backends::turn_engine`] (R7 phase 2, after openai in phase 1).
//!
//! The turn scaffold (idle/cancel atomics, pre-turn gate, retry-wrapped
//! stream open, idle-stall arm, MAX_TOOL_ROUNDS, the finish-tool special
//! case, usage folding, terminal step, compaction trigger) lives in the
//! engine; this module supplies only the Anthropic-specific wire behavior
//! via [`AnthropicProvider`]:
//!
//! - Stream fold: INDEX-KEYED `thinking`/`signature`/`input_json` deltas —
//!   tool args arrive as `input_json_delta.partial_json` FRAGMENTS
//!   concatenated per block index; a thinking block's `signature` arrives on
//!   a trailing `signature_delta` for the same index.
//! - Assistant-message assembly: SIGNED thinking blocks lead the turn
//!   (thinking → text → tool_use); unsigned thinking is dropped.
//! - Tool results: ONE batched `user`-role message of `tool_result` blocks
//!   matched by `tool_use_id` (unlike openai's one message per call).
//! - `on_stream_end`: the `pause_turn` resume loop (re-request against
//!   identical history, accumulators retained) under the anthropic-owned
//!   [`MAX_PAUSE_RESUMES`] cap.
//! - `on_cancel_with_pending_calls`: the #82 tool_result balancing — a
//!   dangling `tool_use` 400s the NEXT request.

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};

use crate::backends::loop_util::resolve_tool_args;
use crate::backends::anthropic::api::SharedClient;
use crate::backends::anthropic::wire::{
    Block, BlockDelta, ImageSource, Message, MessagesRequest, Role, StopReason, StreamEvent,
    ThinkingConfig, ToolDef, WireUsage, DEFAULT_MAX_TOKENS,
};
use crate::backends::turn_engine::{
    self, DispatchedResult, EmitCtx, EngineDeps, ResolvedCall, StreamEnd, TurnProvider,
};
use crate::content::{Content, Part as ApiPart};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{StepStatus, SystemInstructions, ThinkingLevel, UsageMetadata};

/// Hard cap on `pause_turn` resumes within a single round, so a backend
/// stuck emitting `pause_turn` can't spin forever. Anthropic-owned (the
/// engine only threads the resume COUNT through `on_stream_end`).
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

/// Per-connection mutable state — the shared generic container specialised to
/// Anthropic's wire-history shape (`Vec<Message>`), analogous to Gemini's
/// `Vec<wire::Content>`.
pub(crate) type LoopState = crate::backends::state::LoopState<Message>;

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

/// One round's stream accumulators (the [`TurnProvider::Accum`]).
#[derive(Default)]
pub(crate) struct RoundAccum {
    /// Per-index thinking accumulators. With extended thinking enabled,
    /// Anthropic REQUIRES the assistant turn's thinking block(s) — WITH
    /// their `signature` — to be echoed back verbatim in the next request
    /// whenever that turn also contains `tool_use`; otherwise the follow-up
    /// round 400s (`messages.N: ... thinking blocks must be preserved`).
    /// So we accumulate each thinking block's text + signature by block
    /// index and re-emit them (in index order, ahead of text/tool_use) into
    /// the persisted assistant message. (`signature` arrives on a separate
    /// `signature_delta`, after the `thinking_delta`s for the same index.)
    thinking_blocks: BTreeMap<u32, ThinkingAccum>,
    /// Per-index block accumulators — text blocks need no state (the engine's
    /// `EmitCtx` owns visible text), tool_use blocks accumulate id/name/args
    /// across deltas.
    tool_blocks: BTreeMap<u32, ToolUseAccum>,
    stop_reason: Option<StopReason>,
    usage: WireUsage,
}

/// The Anthropic side of the [`TurnProvider`] seam — a zero-sized marker the
/// engine is monomorphized over (static dispatch; see `turn_engine`).
pub(crate) struct AnthropicProvider;

impl TurnProvider for AnthropicProvider {
    type Message = Message;
    type Config = LoopConfig;
    type Request = MessagesRequest;
    type Event = StreamEvent;
    type Accum = RoundAccum;

    fn build_request(config: &LoopConfig, history: &[Message]) -> MessagesRequest {
        build_request(config, history)
    }

    fn compaction_threshold(config: &LoopConfig) -> Option<u32> {
        config.compaction_threshold
    }

    fn fold_event(
        acc: &mut RoundAccum,
        ctx: &mut EmitCtx<'_, Message>,
        ev: StreamEvent,
    ) -> Result<()> {
        match ev {
            StreamEvent::MessageStart { message } => {
                if let Some(u) = message.usage {
                    accumulate_wire_usage(&mut acc.usage, &u);
                }
            }
            StreamEvent::ContentBlockStart {
                index,
                content_block,
            } => match content_block {
                Block::ToolUse { id, name, .. } => {
                    acc.tool_blocks.insert(
                        index,
                        ToolUseAccum {
                            id,
                            name,
                            args_json: String::new(),
                        },
                    );
                }
                // content_block_start for a text block may carry a non-empty
                // seed (rare); `push_text` no-ops on an empty one.
                Block::Text { text } => ctx.push_text(&text),
                _ => {}
            },
            StreamEvent::ContentBlockDelta { index, delta } => match delta {
                BlockDelta::TextDelta { text } => ctx.push_text(&text),
                BlockDelta::ThinkingDelta { thinking } => {
                    if !thinking.is_empty() {
                        acc.thinking_blocks
                            .entry(index)
                            .or_default()
                            .thinking
                            .push_str(&thinking);
                        ctx.push_thought(&thinking);
                    }
                }
                BlockDelta::SignatureDelta { signature } => {
                    // The cryptographic signature for the thinking block at
                    // this index — required when echoing the block back
                    // alongside a tool_use.
                    acc.thinking_blocks.entry(index).or_default().signature = signature;
                }
                BlockDelta::InputJsonDelta { partial_json } => {
                    if let Some(a) = acc.tool_blocks.get_mut(&index) {
                        a.args_json.push_str(&partial_json);
                    }
                }
                _ => {}
            },
            StreamEvent::MessageDelta { delta, usage } => {
                if let Some(r) = delta.stop_reason {
                    acc.stop_reason = Some(r);
                }
                if let Some(u) = usage {
                    accumulate_wire_usage(&mut acc.usage, &u);
                }
            }
            // An in-band `error` event fails the turn through the engine's
            // shared stream-error path.
            StreamEvent::Error { error } => {
                return Err(Error::other(format!(
                    "anthropic stream error [{}]: {}",
                    error.kind, error.message
                )));
            }
            StreamEvent::ContentBlockStop { .. }
            | StreamEvent::MessageStop
            | StreamEvent::Ping
            | StreamEvent::Unknown => {}
        }
        Ok(())
    }

    /// Resolve tool_use accumulators into ordered calls (parse the
    /// concatenated args JSON). An EMPTY/absent fragment is a valid no-arg
    /// call → `{}`. A NON-EMPTY fragment that FAILS to parse (truncated
    /// stream) must NOT silently run with `{}` — carry the parse error so
    /// dispatch surfaces it as a tool error to the model instead.
    fn resolve_pending_calls(acc: &mut RoundAccum) -> Vec<ResolvedCall> {
        std::mem::take(&mut acc.tool_blocks)
            .into_values()
            .map(|a| {
                let (args, parse_error) = resolve_tool_args(&a.name, &a.args_json);
                ResolvedCall {
                    // Anthropic correlates by id (Gemini leaves None).
                    id: Some(a.id),
                    name: a.name,
                    args,
                    parse_error,
                }
            })
            .collect()
    }

    fn round_usage(acc: &RoundAccum) -> UsageMetadata {
        acc.usage.clone().into()
    }

    fn map_finish_reason(acc: &RoundAccum) -> (StepStatus, &'static str) {
        match acc.stop_reason {
            Some(StopReason::Refusal) => (StepStatus::Error, "stopped by refusal"),
            Some(StopReason::MaxTokens) => (StepStatus::Done, "stopped at max tokens"),
            Some(StopReason::PauseTurn) => (StepStatus::Done, "paused (resume cap reached)"),
            _ => (StepStatus::Done, ""),
        }
    }

    /// Build the assistant-turn message. Block order matters: Anthropic
    /// requires thinking block(s) to lead the turn (ahead of text/tool_use),
    /// each carrying its `signature`. We only persist a thinking block once
    /// its signature has arrived — an unsigned thinking block echoed back
    /// would itself 400. (A thinking block with no signature can only occur
    /// on a truncated/cancelled stream, in which case we drop it; the turn
    /// won't be replayed correctly anyway.)
    fn assemble_assistant_message(
        acc: RoundAccum,
        text: &str,
        calls: &[ResolvedCall],
    ) -> Option<Message> {
        let mut blocks: Vec<Block> = Vec::new();
        for (_idx, t) in acc.thinking_blocks {
            if !t.thinking.is_empty() && !t.signature.is_empty() {
                blocks.push(Block::Thinking {
                    thinking: t.thinking,
                    signature: Some(t.signature),
                });
            }
        }
        if !text.is_empty() {
            blocks.push(Block::Text {
                text: text.to_string(),
            });
        }
        for c in calls {
            blocks.push(Block::ToolUse {
                id: c.id.clone().unwrap_or_default(),
                name: c.name.clone(),
                input: c.args.clone(),
            });
        }
        (!blocks.is_empty()).then_some(Message {
            role: Role::Assistant,
            content: blocks,
        })
    }

    /// ONE batched `user`-role message of `tool_result` blocks matched by
    /// `tool_use_id` (unlike openai's one `tool` message per call).
    fn tool_result_messages(results: Vec<DispatchedResult>) -> Vec<Message> {
        if results.is_empty() {
            return Vec::new();
        }
        let blocks: Vec<Block> = results
            .into_iter()
            .map(|r| Block::ToolResult {
                tool_use_id: r.call.id.unwrap_or_default(),
                content: tool_result_content(&r.value),
                is_error: r.is_error.then_some(true),
            })
            .collect();
        vec![Message {
            role: Role::User,
            content: blocks,
        }]
    }

    /// THE `pause_turn` hook (this backend is why the engine has it): the
    /// model paused mid-turn (e.g. a server-side tool) — `Resume` re-requests
    /// against IDENTICAL history with the round's accumulators retained,
    /// bounded by the anthropic-owned [`MAX_PAUSE_RESUMES`] cap
    /// (`ProceedAndEndTurn`: persist what streamed, end the turn without
    /// dispatching). `stop_reason` is NOT cleared on `Resume`: a resumed
    /// stream's `message_delta` overwrites it, and when the ENGINE ignores
    /// the `Resume` (cancelled pause) the retained `PauseTurn` still maps the
    /// terminal step to the "paused" finish reason — the old loop's exact
    /// cancelled-pause semantics.
    fn on_stream_end(acc: &mut RoundAccum, pause_resumes: u32) -> StreamEnd {
        if !matches!(acc.stop_reason, Some(StopReason::PauseTurn)) {
            return StreamEnd::Proceed;
        }
        if pause_resumes < MAX_PAUSE_RESUMES {
            StreamEnd::Resume
        } else {
            StreamEnd::ProceedAndEndTurn
        }
    }

    /// REGRESSION guard (#82): the assistant turn carrying `tool_use` blocks
    /// is pushed to history BEFORE tools dispatch. Anthropic 400s the NEXT
    /// turn if a tool_use isn't answered by a matching tool_result
    /// (`tool_use ids were found without tool_result blocks`), so balance
    /// every pending call with a cancelled tool_result — otherwise the
    /// dangling tool_use bricks the conversation.
    fn on_cancel_with_pending_calls(calls: &[ResolvedCall]) -> Vec<Message> {
        let blocks: Vec<Block> = calls
            .iter()
            .map(|c| Block::ToolResult {
                tool_use_id: c.id.clone().unwrap_or_default(),
                content: tool_result_content(&json!({ "error": "cancelled" })),
                is_error: Some(true),
            })
            .collect();
        vec![Message {
            role: Role::User,
            content: blocks,
        }]
    }
}

/// Drive one turn through the shared engine, plugging in the Anthropic
/// client for the stream open and the compaction fold (the engine stays
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
    let engine_deps = EngineDeps::<AnthropicProvider> {
        config,
        state: state.clone(),
        tool_runner,
        hook_runner,
        session_ctx,
    };
    let open_client = client.clone();
    turn_engine::run_turn::<AnthropicProvider, _, _, _, _, _>(
        engine_deps,
        user,
        prompt,
        move |req: MessagesRequest| {
            let client = open_client.clone();
            async move { client.stream_messages(&req).await }
        },
        move || async move {
            crate::backends::anthropic::compaction::try_compact(
                &state.history,
                &client,
                &model,
            )
            .await;
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
        // System → content-block array with a cache_control breakpoint (the
        // ~15KB system prompt caches across turns). `system_from` handles the
        // None/empty case (omitted).
        system: MessagesRequest::system_from(config.system.clone()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CustomSystemInstructions, Step, StepStatus, StreamChunk, SystemInstructions};
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
        // System renders as the cache-controlled content-block array.
        assert_eq!(req.system.len(), 1);
        assert_eq!(req.system[0].text, "sys");
        assert!(req.system[0].cache_control.is_some());
    }

    /// REGRESSION: Anthropic splits usage across two events — `message_start`
    /// carries `input_tokens` + a small PLACEHOLDER `output_tokens` (typically
    /// 1-4, the envelope), and `message_delta` carries the CUMULATIVE final
    /// `output_tokens` for the whole message. The provider's `fold_event`
    /// folds both into the round accumulator exactly as this test does.
    /// `output_tokens` must end up equal to the `message_delta` value (the
    /// cumulative total), NOT `message_start.output_tokens +
    /// message_delta.output_tokens` — adding them double-counts the
    /// placeholder and over-reports billed output tokens every turn (and
    /// compounds per round in a multi-round tool turn).
    #[test]
    fn usage_does_not_double_count_message_start_output_placeholder() {
        // Replicates the exact accumulation the provider performs in one round.
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

    /// A thinking-enabled turn that also makes a tool call: Anthropic
    /// REQUIRES the signed thinking block(s) to be echoed back (and lead the
    /// assistant turn) whenever the turn contains a tool_use — omitting them
    /// 400s the follow-up round. The block order must be: thinking (with
    /// signature) → text → tool_use. (R7 phase 2: setup retargeted from the
    /// deleted inline scaffold to the provider's real
    /// `assemble_assistant_message` — assertions unchanged.)
    #[test]
    fn assistant_turn_preserves_signed_thinking_block_before_tool_use() {
        // Mirror the per-index accumulators fold_event fills from the stream.
        let mut acc = RoundAccum::default();
        // Block 0: a thinking block with text + a trailing signature.
        acc.thinking_blocks.insert(
            0,
            ThinkingAccum {
                thinking: "Let me reason about this.".into(),
                signature: "sig_abc".into(),
            },
        );
        let pending_calls = vec![ResolvedCall {
            id: Some("toolu_1".into()),
            name: "view_file".into(),
            args: json!({"path": "a.rs"}),
            parse_error: None,
        }];

        let msg = AnthropicProvider::assemble_assistant_message(acc, "I'll read it.", &pending_calls)
            .expect("assistant message assembled");
        assert_eq!(msg.role, Role::Assistant);
        let assistant_blocks = msg.content;

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
    /// unsigned thinking block sent back is itself a 400. (Setup retargeted
    /// to the provider's `assemble_assistant_message`; assertion unchanged.)
    #[test]
    fn unsigned_thinking_block_is_dropped() {
        let mut acc = RoundAccum::default();
        acc.thinking_blocks.insert(
            0,
            ThinkingAccum {
                thinking: "partial reasoning".into(),
                signature: String::new(), // never signed
            },
        );
        let msg = AnthropicProvider::assemble_assistant_message(acc, "", &[]);
        assert!(
            msg.is_none(),
            "unsigned thinking must not be persisted"
        );
    }

    /// REGRESSION (#82): the assistant turn carrying `tool_use` blocks is pushed
    /// to history BEFORE tools dispatch. If the turn is cancelled in that window
    /// the loop breaks WITHOUT appending the matching `tool_result` user message
    /// — a dangling tool_use that 400s the NEXT Anthropic turn
    /// (`tool_use ids were found without tool_result blocks`). The cancel branch
    /// must balance every pending call with a cancelled tool_result so history
    /// stays valid. Exercises the provider's `on_cancel_with_pending_calls`
    /// (the engine hook this phase exists to prove); assertions unchanged from
    /// the pre-engine pinning test.
    #[test]
    fn cancelled_turn_balances_pending_tool_use_with_tool_results() {
        let pending_calls = vec![
            ResolvedCall {
                id: Some("toolu_1".into()),
                name: "view_file".into(),
                args: json!({"path": "a.rs"}),
                parse_error: None,
            },
            ResolvedCall {
                id: Some("toolu_2".into()),
                name: "list_directory".into(),
                args: json!({}),
                parse_error: None,
            },
        ];

        let balance = AnthropicProvider::on_cancel_with_pending_calls(&pending_calls);
        // ONE batched user message of tool_result blocks (the wire shape).
        assert_eq!(balance.len(), 1, "anthropic balances with one user turn");
        assert_eq!(balance[0].role, Role::User);
        let cancelled_blocks = &balance[0].content;

        // One tool_result per tool_use, ids preserved, marked errored.
        assert_eq!(cancelled_blocks.len(), 2);
        let ids: Vec<&str> = cancelled_blocks
            .iter()
            .map(|b| match b {
                Block::ToolResult {
                    tool_use_id,
                    is_error,
                    content,
                } => {
                    assert_eq!(*is_error, Some(true), "cancelled result must be is_error");
                    // Content is a wire-valid STRING (Anthropic 400s a bare object).
                    assert!(content.is_string(), "tool_result.content must be a string");
                    assert!(
                        content.as_str().unwrap().contains("cancelled"),
                        "content should mark the call cancelled"
                    );
                    tool_use_id.as_str()
                }
                other => panic!("expected ToolResult block, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, vec!["toolu_1", "toolu_2"], "every tool_use id answered");
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

    /// THE index-keyed stream-fold contract, exercised through the provider's
    /// real seam (the path the engine drives): `thinking`/`signature`/
    /// `input_json` deltas accumulate per block index, args fragments
    /// concatenate, and `resolve_pending_calls` parses them in index order.
    #[test]
    fn provider_fold_accumulates_index_keyed_deltas() {
        let (tx, _rx) = broadcast::channel::<Step>(8);
        let state = LoopState::new(tx);
        let mut acc = RoundAccum::default();
        turn_engine::test_fold_events::<AnthropicProvider>(
            &state,
            &mut acc,
            vec![
                // Block 0: thinking text then its trailing signature.
                StreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: BlockDelta::ThinkingDelta { thinking: "reason ".into() },
                },
                StreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: BlockDelta::ThinkingDelta { thinking: "more".into() },
                },
                StreamEvent::ContentBlockDelta {
                    index: 0,
                    delta: BlockDelta::SignatureDelta { signature: "sig_x".into() },
                },
                // Block 1: a tool_use whose args stream as fragments.
                StreamEvent::ContentBlockStart {
                    index: 1,
                    content_block: Block::ToolUse {
                        id: "toolu_9".into(),
                        name: "view_file".into(),
                        input: Value::Null,
                    },
                },
                StreamEvent::ContentBlockDelta {
                    index: 1,
                    delta: BlockDelta::InputJsonDelta { partial_json: "{\"path\":".into() },
                },
                StreamEvent::ContentBlockDelta {
                    index: 1,
                    delta: BlockDelta::InputJsonDelta { partial_json: "\"a.rs\"}".into() },
                },
                // Terminal stop reason.
                StreamEvent::MessageDelta {
                    delta: crate::backends::anthropic::wire::MessageDeltaBody {
                        stop_reason: Some(StopReason::ToolUse),
                        stop_sequence: None,
                    },
                    usage: None,
                },
            ],
        );

        let t = &acc.thinking_blocks[&0];
        assert_eq!(t.thinking, "reason more", "thinking deltas concatenate per index");
        assert_eq!(t.signature, "sig_x", "trailing signature lands on the same index");

        let calls = AnthropicProvider::resolve_pending_calls(&mut acc);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id.as_deref(), Some("toolu_9"));
        assert_eq!(calls[0].name, "view_file");
        assert_eq!(calls[0].args, json!({"path": "a.rs"}), "args fragments reassemble");
        assert!(calls[0].parse_error.is_none());
        assert_eq!(acc.stop_reason, Some(StopReason::ToolUse));
    }

    /// THE `pause_turn` hook (the other hook this phase proves): under the
    /// anthropic-owned MAX_PAUSE_RESUMES cap the provider asks the engine to
    /// Resume (identical history, accumulators retained); AT the cap it ends
    /// the turn after persisting (ProceedAndEndTurn); a non-pause stop
    /// proceeds normally. `stop_reason` is retained across Resume so a
    /// CANCELLED pause (the engine ignores Resume while cancelled) still maps
    /// the terminal step to the "paused" finish reason — and a resume cap hit
    /// reports it too.
    #[test]
    fn pause_turn_resumes_until_cap_then_ends_turn() {
        let mut acc = RoundAccum {
            stop_reason: Some(StopReason::EndTurn),
            ..Default::default()
        };

        // Non-pause stop → Proceed.
        assert!(matches!(
            AnthropicProvider::on_stream_end(&mut acc, 0),
            StreamEnd::Proceed
        ));

        // pause_turn under the cap → Resume, stop_reason retained.
        acc.stop_reason = Some(StopReason::PauseTurn);
        assert!(matches!(
            AnthropicProvider::on_stream_end(&mut acc, 0),
            StreamEnd::Resume
        ));
        assert_eq!(
            acc.stop_reason,
            Some(StopReason::PauseTurn),
            "retained so a cancelled pause still reports the paused finish reason"
        );
        assert!(matches!(
            AnthropicProvider::on_stream_end(&mut acc, MAX_PAUSE_RESUMES - 1),
            StreamEnd::Resume
        ));

        // AT the cap → persist what streamed, end the turn (no dispatch).
        assert!(matches!(
            AnthropicProvider::on_stream_end(&mut acc, MAX_PAUSE_RESUMES),
            StreamEnd::ProceedAndEndTurn
        ));

        // The retained PauseTurn maps to the "paused" terminal message.
        let (status, msg) = AnthropicProvider::map_finish_reason(&acc);
        assert_eq!(status, StepStatus::Done);
        assert_eq!(msg, "paused (resume cap reached)");
    }

    /// The engine hands every dispatched result back for wire-shaping: one
    /// BATCHED user message; `is_error` maps onto the typed
    /// `tool_result.is_error` (`false` → omitted, matching the old loop's
    /// finish/success shape); content is stringified.
    #[test]
    fn tool_result_messages_batch_into_one_user_turn() {
        let mk = |id: &str, value: Value, is_error: bool| DispatchedResult {
            call: ResolvedCall {
                id: Some(id.into()),
                name: "t".into(),
                args: json!({}),
                parse_error: None,
            },
            value,
            is_error,
        };
        let msgs = AnthropicProvider::tool_result_messages(vec![
            mk("toolu_1", json!({"ok": true}), false),
            mk("toolu_2", json!({"error": "boom"}), true),
        ]);
        assert_eq!(msgs.len(), 1, "one batched user turn");
        assert_eq!(msgs[0].role, Role::User);
        match &msgs[0].content[0] {
            Block::ToolResult { tool_use_id, is_error, content } => {
                assert_eq!(tool_use_id, "toolu_1");
                assert_eq!(*is_error, None, "success omits is_error");
                assert!(content.is_string());
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
        match &msgs[0].content[1] {
            Block::ToolResult { tool_use_id, is_error, .. } => {
                assert_eq!(tool_use_id, "toolu_2");
                assert_eq!(*is_error, Some(true), "failure marks is_error");
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }
}
