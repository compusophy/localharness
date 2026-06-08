//! Anthropic Messages API request/response/stream-event types.
//!
//! Field naming matches the Anthropic API verbatim (snake_case). Unlike
//! Gemini's untagged `Part` (matched by first-key), Anthropic content is
//! `type`-tagged, so [`Block`] is an internally-tagged enum keyed on
//! `"type"`. The streaming wire is a named-event SSE: `message_start`,
//! `content_block_start`, `content_block_delta`, `content_block_stop`,
//! `message_delta`, `message_stop` (+ `ping` / `error`) — modeled by
//! [`StreamEvent`].
//!
//! From/Into conversions land the load-bearing wire differences below the
//! neutral [`crate::types`] boundary: roles (`assistant` not `model`),
//! tool-call/result matching by `id` (Gemini matches by name), split
//! usage (`input_tokens` in `message_start`, `output_tokens` in
//! `message_delta`), and `cache_read_input_tokens` →
//! `cached_content_token_count` (free prompt-caching surfacing).

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::UsageMetadata;

// =============================================================================
// Model IDs (verified live, June 2026) — live in the backend, not types.rs.
// =============================================================================

/// Default chat model — cheapest, the subsidized default.
pub const DEFAULT_MODEL: &str = "claude-haiku-4-5-20251001";
/// Mid-tier model.
pub const SONNET_MODEL: &str = "claude-sonnet-4-6";
/// 1M-context top tier (the Rust-coding tier).
pub const OPUS_MODEL: &str = "claude-opus-4-8";

/// The `anthropic-version` header value the Messages API requires.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default `max_tokens` when the caller doesn't override. Anthropic
/// REQUIRES `max_tokens`; this is the floor we always send.
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

// =============================================================================
// Request
// =============================================================================

/// A `POST /v1/messages` request body.
#[derive(Debug, Clone, Serialize, Default)]
pub struct MessagesRequest {
    pub model: String,
    /// REQUIRED by the API — never omit.
    pub max_tokens: u32,
    /// Top-level system prompt (Anthropic puts it here, not in `messages`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Always `true` for the streaming path; serialized so the upstream
    /// switches to SSE.
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}

/// One conversation message. Anthropic only knows `user` / `assistant`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
}

/// Message role — `user` / `assistant` (NOT Gemini's `model`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// A `type`-tagged content block. Anthropic discriminates by an explicit
/// `"type"` field, so this is an internally-tagged enum (unlike Gemini's
/// untagged `Part`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    /// Plain text.
    Text { text: String },
    /// Model reasoning (extended thinking).
    Thinking {
        thinking: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// A tool call from the model. `input` is the parsed JSON args.
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: Value,
    },
    /// Our response to a tool call. Matched to the call by `tool_use_id`.
    ToolResult {
        tool_use_id: String,
        content: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
    /// Inline base64 media (image input).
    Image { source: ImageSource },
    /// Any content-block `type` the SDK doesn't model — e.g.
    /// `redacted_thinking` (extended thinking with a redacted segment) or
    /// future server-side blocks (`server_tool_use`, `web_search_tool_result`,
    /// `mcp_tool_use`). Without this fallback an unmodeled block type would
    /// fail to deserialize and abort the whole stream/turn. We decode it into
    /// this benign variant; the loop simply ignores it (it's not text, not a
    /// tool call). `#[serde(other)]` requires a unit variant.
    #[serde(other)]
    Other,
}

/// Base64 image source for an [`Block::Image`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImageSource {
    /// Always `"base64"` for inline data.
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    /// Base64-encoded payload.
    pub data: String,
}

/// A tool declaration. `input_schema` takes the neutral
/// `Tool::input_schema()` JSON verbatim (same as Gemini's `parameters`).
#[derive(Debug, Clone, Serialize, Default)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// `tool_choice` selector.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolChoice {
    /// Model decides whether to call a tool.
    Auto,
    /// Model must call some tool.
    Any,
    /// Model must call this specific tool.
    Tool { name: String },
    /// Model must not call tools.
    None,
}

/// Extended-thinking config. `budget_tokens` must be >= 1024, and the
/// request's `max_tokens` MUST exceed it (the loop clamps both).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ThinkingConfig {
    /// Always `"enabled"` when present.
    #[serde(rename = "type")]
    pub kind: String,
    pub budget_tokens: u32,
}

impl ThinkingConfig {
    /// Build an enabled thinking config with the given (already-clamped)
    /// budget.
    pub fn enabled(budget_tokens: u32) -> Self {
        Self {
            kind: "enabled".to_string(),
            budget_tokens,
        }
    }
}

// =============================================================================
// Non-streaming response
// =============================================================================

/// A complete (non-streaming) `messages` response. Used by the one-shot
/// path (`OneShotComplete` / compaction summary) and the
/// deserialize-parity unit test.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessagesResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub content: Vec<Block>,
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

impl MessagesResponse {
    /// Concatenate every text block into one string (drops tool_use /
    /// thinking). Used by the one-shot completion path.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.content {
            if let Block::Text { text } = block {
                out.push_str(text);
            }
        }
        out
    }
}

/// Why the model stopped.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Refusal,
    /// The model paused mid-turn (e.g. server-tool round-trip); the
    /// caller re-requests to resume.
    PauseTurn,
    #[serde(other)]
    Unknown,
}

// =============================================================================
// Usage
// =============================================================================

/// Anthropic usage. `input_tokens` arrives in `message_start`,
/// `output_tokens` in `message_delta` — the loop accumulates both. Cache
/// fields surface free prompt-caching savings.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct WireUsage {
    #[serde(default)]
    pub input_tokens: Option<i32>,
    #[serde(default)]
    pub output_tokens: Option<i32>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<i32>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<i32>,
}

impl From<WireUsage> for UsageMetadata {
    fn from(w: WireUsage) -> Self {
        // total = prompt + completion (Anthropic has no single total field).
        let total = match (w.input_tokens, w.output_tokens) {
            (Some(i), Some(o)) => Some(i + o),
            (Some(i), None) => Some(i),
            (None, Some(o)) => Some(o),
            (None, None) => None,
        };
        UsageMetadata {
            prompt_token_count: w.input_tokens,
            // Free prompt-caching surfacing onto the existing neutral field.
            cached_content_token_count: w.cache_read_input_tokens,
            candidates_token_count: w.output_tokens,
            thoughts_token_count: None,
            total_token_count: total,
        }
    }
}

// =============================================================================
// Streaming events (named-event SSE)
// =============================================================================

/// One decoded SSE event from the Messages stream. The wire carries the
/// event name twice — as the SSE `event:` line and as the JSON `"type"`
/// field — so we tag on `"type"` and ignore the `event:` line.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: StreamMessage,
    },
    ContentBlockStart {
        index: u32,
        content_block: Block,
    },
    ContentBlockDelta {
        index: u32,
        delta: BlockDelta,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaBody,
        #[serde(default)]
        usage: Option<WireUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiError,
    },
    /// Any future event type we don't model — ignored by the loop.
    #[serde(other)]
    Unknown,
}

/// The `message` envelope inside `message_start`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct StreamMessage {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

/// The incremental delta inside `content_block_delta`. Tool args arrive as
/// `input_json_delta.partial_json` FRAGMENTS to be concatenated per block
/// index and parsed at `content_block_stop`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BlockDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    ThinkingDelta { thinking: String },
    SignatureDelta { signature: String },
    #[serde(other)]
    Unknown,
}

/// The delta body inside `message_delta` — carries the terminal stop reason.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct MessageDeltaBody {
    #[serde(default)]
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
}

/// An `error` SSE event body.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ApiError {
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub message: String,
}

// =============================================================================
// Helpers
// =============================================================================

impl Message {
    /// A single-text user message.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![Block::Text { text: text.into() }],
        }
    }

    /// A single-text assistant message.
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![Block::Text { text: text.into() }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_text_block() {
        let b: Block = serde_json::from_str(r#"{"type":"text","text":"hello"}"#).unwrap();
        assert!(matches!(b, Block::Text { ref text } if text == "hello"));
    }

    #[test]
    fn deserialize_tool_use_block() {
        let json = r#"{"type":"tool_use","id":"toolu_1","name":"view_file","input":{"path":"x.txt"}}"#;
        let b: Block = serde_json::from_str(json).unwrap();
        match b {
            Block::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "view_file");
                assert_eq!(input["path"], "x.txt");
            }
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    /// REGRESSION: a `content_block_start` carrying a content-block type the
    /// SDK doesn't model (`redacted_thinking` when extended thinking is
    /// redacted, or future server-side block types like `server_tool_use` /
    /// `web_search_tool_result`) must NOT fail to deserialize — that would
    /// error the ENTIRE stream and abort the turn. The `Block` enum needs an
    /// `Other` fallback so unmodeled block types decode into a benign,
    /// ignored variant instead of a hard decode error.
    #[test]
    fn deserialize_unknown_block_type_does_not_error() {
        // redacted_thinking is a real Anthropic block type (extended thinking).
        let redacted = r#"{"type":"redacted_thinking","data":"EvwBCkYIB..."}"#;
        let b: Block = serde_json::from_str(redacted)
            .expect("unknown block type must decode into a fallback, not error");
        assert!(matches!(b, Block::Other), "expected Block::Other, got {b:?}");

        // The stream wraps it in a content_block_start — the event the live
        // loop actually sees. This is the path that previously aborted turns.
        let ev: StreamEvent = serde_json::from_str(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"redacted_thinking","data":"EvwBCkYIB..."}}"#,
        )
        .expect("content_block_start with an unknown block must decode, not error");
        match ev {
            StreamEvent::ContentBlockStart { index, content_block } => {
                assert_eq!(index, 1);
                assert!(matches!(content_block, Block::Other));
            }
            other => panic!("expected ContentBlockStart, got {other:?}"),
        }
    }

    /// Deserialize a FULL non-streaming Messages response carrying a text
    /// block AND a tool_use block; assert the neutral conversions.
    #[test]
    fn deserialize_full_response_text_and_tool_use() {
        let json = r#"{
            "id": "msg_01",
            "model": "claude-haiku-4-5-20251001",
            "role": "assistant",
            "content": [
                {"type":"text","text":"Let me read it."},
                {"type":"tool_use","id":"toolu_abc","name":"view_file","input":{"path":"main.rs"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 42, "output_tokens": 17, "cache_read_input_tokens": 8}
        }"#;
        let resp: MessagesResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.role, Some(Role::Assistant));
        assert_eq!(resp.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(resp.text(), "Let me read it.");
        // tool_use block round-trips into a (name, id, input) we can map to ToolCall.
        let tool = resp
            .content
            .iter()
            .find_map(|b| match b {
                Block::ToolUse { id, name, input } => Some((id.clone(), name.clone(), input.clone())),
                _ => None,
            })
            .expect("tool_use block present");
        assert_eq!(tool.0, "toolu_abc");
        assert_eq!(tool.1, "view_file");
        assert_eq!(tool.2["path"], "main.rs");

        // Usage maps onto the neutral metadata, including the cache field.
        let usage: UsageMetadata = resp.usage.unwrap().into();
        assert_eq!(usage.prompt_token_count, Some(42));
        assert_eq!(usage.candidates_token_count, Some(17));
        assert_eq!(usage.cached_content_token_count, Some(8));
        assert_eq!(usage.total_token_count, Some(59));
    }

    #[test]
    fn deserialize_stream_events_tagged_on_type() {
        let start: StreamEvent = serde_json::from_str(
            r#"{"type":"message_start","message":{"id":"msg_1","model":"claude-haiku-4-5-20251001","role":"assistant","usage":{"input_tokens":10,"output_tokens":1}}}"#,
        )
        .unwrap();
        match start {
            StreamEvent::MessageStart { message } => {
                assert_eq!(message.id, "msg_1");
                assert_eq!(message.usage.unwrap().input_tokens, Some(10));
            }
            other => panic!("expected MessageStart, got {other:?}"),
        }

        let delta: StreamEvent = serde_json::from_str(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
        )
        .unwrap();
        match delta {
            StreamEvent::ContentBlockDelta { index, delta } => {
                assert_eq!(index, 0);
                assert_eq!(
                    delta,
                    BlockDelta::InputJsonDelta {
                        partial_json: "{\"pa".to_string()
                    }
                );
            }
            other => panic!("expected ContentBlockDelta, got {other:?}"),
        }

        let mdelta: StreamEvent = serde_json::from_str(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":25}}"#,
        )
        .unwrap();
        match mdelta {
            StreamEvent::MessageDelta { delta, usage } => {
                assert_eq!(delta.stop_reason, Some(StopReason::ToolUse));
                assert_eq!(usage.unwrap().output_tokens, Some(25));
            }
            other => panic!("expected MessageDelta, got {other:?}"),
        }

        let ping: StreamEvent = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(ping, StreamEvent::Ping);
        let stop: StreamEvent = serde_json::from_str(r#"{"type":"message_stop"}"#).unwrap();
        assert_eq!(stop, StreamEvent::MessageStop);
    }

    #[test]
    fn request_serializes_required_fields() {
        let req = MessagesRequest {
            model: DEFAULT_MODEL.to_string(),
            max_tokens: DEFAULT_MAX_TOKENS,
            system: Some("be terse".to_string()),
            messages: vec![Message::user_text("hi")],
            tools: Vec::new(),
            tool_choice: None,
            stream: true,
            temperature: None,
            thinking: None,
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], DEFAULT_MODEL);
        assert_eq!(v["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(v["system"], "be terse");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"][0]["type"], "text");
        // Empty tools omitted; thinking/temperature/tool_choice omitted.
        assert!(v.get("tools").is_none());
        assert!(v.get("thinking").is_none());
        assert!(v.get("temperature").is_none());
        assert!(v.get("tool_choice").is_none());
    }
}
