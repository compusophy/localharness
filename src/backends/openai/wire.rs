//! OpenAI Chat Completions API request/response/stream-chunk types.
//!
//! Field naming matches the OpenAI API verbatim (snake_case). Unlike
//! Anthropic's `type`-tagged content blocks, OpenAI messages are
//! role-keyed objects: a `system`/`user`/`assistant`/`tool` `role` plus a
//! `content` string and (for assistant turns) a `tool_calls` array. A tool
//! result is its OWN message (`role:"tool"`, `tool_call_id`, `content`).
//!
//! The streaming wire is OpenAI's SSE: each frame is `data: <json>` (NO
//! `event:` line, terminated by a literal `data: [DONE]`), decoding to a
//! [`ChatChunk`] carrying `choices[].delta`. Tool calls arrive as INDEX-KEYED
//! `delta.tool_calls` FRAGMENTS — the `id`/`function.name` land on the first
//! fragment for a given `index`, and `function.arguments` arrives as STRING
//! fragments to concatenate across chunks (the #1 OpenAI-specific gotcha;
//! see `loop.rs::run_turn`). The turn ends on a `choices[].finish_reason`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::UsageMetadata;

// =============================================================================
// Model IDs — live in the backend, not types.rs. OpenAI flips model ids; do
// NOT hardcode-validate. These are documented defaults only.
// =============================================================================

/// Default chat model — the cheapest, the subsidized default.
pub const DEFAULT_MODEL: &str = "gpt-5-nano";
/// Mid-tier model.
pub const MINI_MODEL: &str = "gpt-5-mini";
/// Top-tier reasoning model.
pub const PRO_MODEL: &str = "gpt-5-pro";

// =============================================================================
// Request
// =============================================================================

/// A `POST /v1/chat/completions` request body.
#[derive(Debug, Clone, Serialize, Default)]
pub struct ChatRequest {
    pub model: String,
    /// Full conversation history (system message included, unlike Anthropic's
    /// top-level `system`).
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<ToolChoice>,
    /// Always `true` for the streaming path; serialized so the upstream
    /// switches to SSE.
    pub stream: bool,
    /// Ask the upstream to include a final `usage` chunk in the stream.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_completion_tokens: Option<u32>,
}

/// `stream_options` toggle — `{"include_usage": true}` makes the upstream emit
/// a terminal chunk carrying the full `usage` block (otherwise streaming
/// responses omit usage entirely).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StreamOptions {
    pub include_usage: bool,
}

/// Message role — `system` / `user` / `assistant` / `tool`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// One conversation message. The shape is role-dependent: a `tool` message
/// carries a `tool_call_id`; an `assistant` message that calls tools carries
/// a `tool_calls` array (and may have `content: null`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    /// Text content. `None` (serialized `null`) for an assistant turn that
    /// only calls tools, which OpenAI accepts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub content: Option<String>,
    /// Tool calls the assistant requested (assistant turns only). NULLABLE on
    /// the wire (the official SDK types it `Optional[List[...]]`) — see
    /// [`null_to_default`].
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        default,
        deserialize_with = "null_to_default"
    )]
    pub tool_calls: Vec<ToolCall>,
    /// The id of the tool call this message answers (`tool` role only).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}

/// A tool call on an assistant message. `function.arguments` is a STRING of
/// JSON (OpenAI's shape), not a parsed object — the loop parses it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    /// Always `"function"` for the only tool kind OpenAI supports here.
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

/// The `function` payload of a [`ToolCall`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionCall {
    pub name: String,
    /// Arguments as a JSON STRING (e.g. `"{\"path\":\"a.rs\"}"`) — NOT a
    /// parsed object. The loop parses it once the stream completes.
    pub arguments: String,
}

/// A tool declaration. The neutral `Tool::input_schema()` JSON passes through
/// verbatim as `function.parameters` (same as Anthropic's `input_schema`).
#[derive(Debug, Clone, Serialize, Default)]
pub struct ToolDef {
    /// Always `"function"`.
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionDef,
}

/// The `function` payload of a [`ToolDef`].
#[derive(Debug, Clone, Serialize, Default)]
pub struct FunctionDef {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub description: String,
    /// The JSON Schema for the arguments — the neutral tool input schema.
    pub parameters: Value,
}

/// `tool_choice` selector.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    /// Model decides whether to call a tool.
    Auto,
    /// Model must call some tool.
    Required,
    /// Model must not call tools.
    None,
}

// =============================================================================
// Non-streaming response
// =============================================================================

/// A complete (non-streaming) chat-completions response. Used by the one-shot
/// path (compaction summary) and a deserialize-parity unit test.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ChatResponse {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

impl ChatResponse {
    /// The first choice's message text (empty if absent). Used by the one-shot
    /// completion path.
    pub fn text(&self) -> String {
        self.choices
            .first()
            .and_then(|c| c.message.as_ref())
            .and_then(|m| m.content.clone())
            .unwrap_or_default()
    }
}

/// One choice in a non-streaming response.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Choice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub message: Option<Message>,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
}

/// Why the model stopped — OpenAI's `finish_reason`.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    /// Any future / function-only reason we don't model.
    #[serde(other)]
    Unknown,
}

// =============================================================================
// Usage
// =============================================================================

/// OpenAI usage block. `prompt_tokens` / `completion_tokens` / `total_tokens`,
/// with optional cached-prompt detail under `prompt_tokens_details`.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct WireUsage {
    #[serde(default)]
    pub prompt_tokens: Option<i32>,
    #[serde(default)]
    pub completion_tokens: Option<i32>,
    #[serde(default)]
    pub total_tokens: Option<i32>,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
}

/// Cached-prompt detail inside [`WireUsage`] — surfaces OpenAI's free
/// prompt-caching savings onto the neutral `cached_content_token_count`.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: Option<i32>,
}

impl From<WireUsage> for UsageMetadata {
    fn from(w: WireUsage) -> Self {
        // OpenAI reports a single total; fall back to prompt+completion.
        let total = w.total_tokens.or(match (w.prompt_tokens, w.completion_tokens) {
            (Some(p), Some(c)) => Some(p + c),
            (Some(p), None) => Some(p),
            (None, Some(c)) => Some(c),
            (None, None) => None,
        });
        UsageMetadata {
            prompt_token_count: w.prompt_tokens,
            cached_content_token_count: w
                .prompt_tokens_details
                .and_then(|d| d.cached_tokens),
            candidates_token_count: w.completion_tokens,
            thoughts_token_count: None,
            total_token_count: total,
        }
    }
}

// =============================================================================
// Streaming chunk (`data: <json>` SSE; sentinel `data: [DONE]`)
// =============================================================================

/// One decoded SSE chunk from the chat-completions stream. The wire frame is
/// `data: <json>` (no `event:` line); the terminal `data: [DONE]` is handled
/// by the SSE decoder, not as a JSON chunk.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct ChatChunk {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    /// Present only on the terminal chunk when `stream_options.include_usage`
    /// is set.
    #[serde(default)]
    pub usage: Option<WireUsage>,
}

/// One choice inside a streaming [`ChatChunk`].
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct ChunkChoice {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
}

/// The incremental `delta` inside a streaming choice. `content` arrives as
/// text fragments; `tool_calls` arrive as INDEX-KEYED fragments — the `id` and
/// `function.name` come on the first fragment for an index, and
/// `function.arguments` arrives as string fragments to concatenate.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct Delta {
    #[serde(default)]
    pub role: Option<Role>,
    #[serde(default)]
    pub content: Option<String>,
    /// The refusal message (SDK `ChoiceDelta.refusal`): a structured-outputs
    /// refusal streams its text HERE (with `content` null) and finishes with a
    /// plain `"stop"` — the loop accumulates it into the classified refusal
    /// finish; dropping it ended the turn as a silent empty Done.
    #[serde(default)]
    pub refusal: Option<String>,
    /// NULLABLE on the wire (SDK type `Optional[List[...]]`) — see
    /// [`null_to_default`].
    #[serde(default, deserialize_with = "null_to_default")]
    pub tool_calls: Vec<ToolCallDelta>,
}

/// Deserialize a nullable field as its default. OpenAI's official SDK types
/// mark `tool_calls` `Optional[List[...]]` — the wire may carry an explicit
/// `null`, which plain `#[serde(default)]` does NOT cover (default applies
/// only to ABSENT fields; an explicit `null` fails `Vec` decode). A rejected
/// frame kills the WHOLE stream decode and fails the turn as an infra-looking
/// "openai sse decode" error — the same class as the Gemini blocked-candidate
/// bug (ad931b47) — so widen the accepted input at the wire boundary.
fn null_to_default<'de, D, T>(d: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// A streamed fragment of an assistant tool call. Keyed by `index`: the first
/// fragment for an index carries `id` + `function.name`; subsequent fragments
/// for the SAME index carry `function.arguments` string pieces to concatenate.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct ToolCallDelta {
    /// Position of this tool call in the assistant turn — the join key for
    /// reassembling fragments. (NOT the `id` — `id` is itself fragmentary.)
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub function: Option<FunctionDelta>,
}

/// The `function` payload of a [`ToolCallDelta`]. `name` lands once;
/// `arguments` is a string FRAGMENT to concatenate per tool-call index.
#[derive(Debug, Clone, Deserialize, Default, PartialEq)]
pub struct FunctionDelta {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

// =============================================================================
// Helpers
// =============================================================================

impl Message {
    /// A single-text system message.
    pub fn system(text: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// A single-text user message.
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// A single-text assistant message.
    pub fn assistant_text(text: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: Some(text.into()),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// A `tool`-role result message answering the call `id`.
    pub fn tool_result(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: Some(content.into()),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn request_serializes_messages_and_tool_schema() {
        let req = ChatRequest {
            model: DEFAULT_MODEL.to_string(),
            messages: vec![
                Message::system("be terse"),
                Message::user_text("hi"),
            ],
            tools: vec![ToolDef {
                kind: "function".into(),
                function: FunctionDef {
                    name: "view_file".into(),
                    description: "read a file".into(),
                    parameters: json!({
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"]
                    }),
                },
            }],
            tool_choice: Some(ToolChoice::Auto),
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
            temperature: Some(0.2),
            max_completion_tokens: Some(256),
        };
        let v = serde_json::to_value(&req).unwrap();
        assert_eq!(v["model"], DEFAULT_MODEL);
        assert_eq!(v["stream"], true);
        assert_eq!(v["stream_options"]["include_usage"], true);
        assert_eq!(v["messages"][0]["role"], "system");
        assert_eq!(v["messages"][0]["content"], "be terse");
        assert_eq!(v["messages"][1]["role"], "user");
        // The tool schema passes through verbatim under function.parameters.
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["function"]["name"], "view_file");
        assert_eq!(
            v["tools"][0]["function"]["parameters"]["properties"]["path"]["type"],
            "string"
        );
        assert_eq!(v["tool_choice"], "auto");
        // f32 → JSON widens to f64 with rounding; check the approximate value.
        assert!((v["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6);
        assert_eq!(v["max_completion_tokens"], 256);
    }

    /// Empty `tools` are omitted (not `[]`); a tool message serializes role
    /// `tool` + `tool_call_id`; an assistant tool-call turn keeps `tool_calls`.
    #[test]
    fn message_shapes_serialize_per_role() {
        let req = ChatRequest {
            model: "m".into(),
            messages: vec![
                Message {
                    role: Role::Assistant,
                    content: None,
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: "view_file".into(),
                            arguments: r#"{"path":"a.rs"}"#.into(),
                        },
                    }],
                    tool_call_id: None,
                },
                Message::tool_result("call_1", r#"{"contents":"fn main(){}"}"#),
            ],
            ..Default::default()
        };
        let v = serde_json::to_value(&req).unwrap();
        assert!(v.get("tools").is_none(), "empty tools omitted");
        // assistant tool-call turn: content omitted (None), tool_calls present.
        assert!(v["messages"][0].get("content").is_none());
        assert_eq!(v["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(v["messages"][0]["tool_calls"][0]["type"], "function");
        assert_eq!(
            v["messages"][0]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"a.rs"}"#
        );
        // tool result message.
        assert_eq!(v["messages"][1]["role"], "tool");
        assert_eq!(v["messages"][1]["tool_call_id"], "call_1");
    }

    #[test]
    fn deserialize_full_response_text_and_tool_call() {
        let json = r#"{
            "id": "chatcmpl-1",
            "model": "gpt-5-nano",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": "Let me read it.",
                    "tool_calls": [{
                        "id": "call_abc",
                        "type": "function",
                        "function": {"name": "view_file", "arguments": "{\"path\":\"main.rs\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 42, "completion_tokens": 17, "total_tokens": 59,
                      "prompt_tokens_details": {"cached_tokens": 8}}
        }"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text(), "Let me read it.");
        let choice = &resp.choices[0];
        assert_eq!(choice.finish_reason, Some(FinishReason::ToolCalls));
        let call = &choice.message.as_ref().unwrap().tool_calls[0];
        assert_eq!(call.id, "call_abc");
        assert_eq!(call.function.name, "view_file");
        // arguments is a STRING of JSON, parsed by the loop.
        let parsed: Value = serde_json::from_str(&call.function.arguments).unwrap();
        assert_eq!(parsed["path"], "main.rs");

        // Usage maps onto the neutral metadata, including the cache field.
        let usage: UsageMetadata = resp.usage.unwrap().into();
        assert_eq!(usage.prompt_token_count, Some(42));
        assert_eq!(usage.candidates_token_count, Some(17));
        assert_eq!(usage.cached_content_token_count, Some(8));
        assert_eq!(usage.total_token_count, Some(59));
    }

    /// FIXTURE (OpenAI SDK `chat_completion_chunk.py`, `Choice.finish_reason`
    /// docstring: "`content_filter` if content was omitted due to a flag from
    /// our content filters"): the terminal filter chunk carries an EMPTY
    /// `delta`, a null `logprobs`, and `finish_reason: "content_filter"`.
    /// It must decode into `FinishReason::ContentFilter` — a rejected frame
    /// would kill the whole stream as a decode error instead of the
    /// classified filter stop (the Gemini blocked-candidate class, ad931b47).
    /// The deprecated `"function_call"` finish reason (also documented in the
    /// same Literal union) must fold to the Unknown catch-all, not error.
    #[test]
    fn deserialize_content_filter_terminal_chunk() {
        let c: ChatChunk = serde_json::from_str(
            r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":"gpt-5-nano","choices":[{"index":0,"delta":{},"logprobs":null,"finish_reason":"content_filter"}]}"#,
        )
        .expect("documented content_filter chunk must decode");
        assert_eq!(c.choices[0].finish_reason, Some(FinishReason::ContentFilter));
        assert!(c.choices[0].delta.content.is_none());
        assert!(c.choices[0].delta.tool_calls.is_empty());

        // Deprecated finish reason we don't model → Unknown, not a decode error.
        let f: ChatChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"function_call"}]}"#,
        )
        .unwrap();
        assert_eq!(f.choices[0].finish_reason, Some(FinishReason::Unknown));
    }

    /// FIXTURE (OpenAI SDK `chat_completion_chunk.py`, `ChoiceDelta.refusal`:
    /// "The refusal message generated by the model."): a refusal streams its
    /// text in `delta.refusal` with `content` null. The frame must DECODE
    /// (rejecting it would abort the stream mid-refusal) AND surface the
    /// refusal text — dropping it ended the turn as a silent empty Done.
    #[test]
    fn deserialize_refusal_delta_chunk() {
        let c: ChatChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":null,"refusal":"I'm sorry, I can't help with that."},"finish_reason":null}]}"#,
        )
        .expect("documented refusal delta must decode");
        assert!(c.choices[0].delta.content.is_none());
        assert_eq!(
            c.choices[0].delta.refusal.as_deref(),
            Some("I'm sorry, I can't help with that."),
            "the refusal text must be decoded, not dropped"
        );
        assert_eq!(c.choices[0].finish_reason, None);
    }

    /// FIXTURE (OpenAI SDK `chat_completion_chunk.py`, `usage` docstring:
    /// with `stream_options.include_usage` every chunk carries `"usage":
    /// null` EXCEPT the last, which carries the stats — and that final usage
    /// chunk's `choices` array "can also be empty"): both shapes must decode.
    #[test]
    fn deserialize_usage_null_then_final_usage_chunk_with_empty_choices() {
        let mid: ChatChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}],"usage":null}"#,
        )
        .expect("explicit usage:null must decode");
        assert!(mid.usage.is_none());

        let last: ChatChunk = serde_json::from_str(
            r#"{"choices":[],"usage":{"prompt_tokens":5,"completion_tokens":7,"total_tokens":12}}"#,
        )
        .expect("final usage chunk with empty choices must decode");
        assert!(last.choices.is_empty());
        assert_eq!(last.usage.unwrap().completion_tokens, Some(7));
    }

    /// REGRESSION (the fix this audit landed): `tool_calls` is
    /// `Optional[List[...]]` in the official SDK types — an explicit
    /// `"tool_calls": null` (streaming delta OR non-streaming message) must
    /// decode to an empty list. Plain `#[serde(default)]` only covers the
    /// ABSENT case; before `null_to_default` the null killed the whole chunk
    /// decode and failed the turn.
    #[test]
    fn deserialize_null_tool_calls_as_empty() {
        let c: ChatChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":null},"finish_reason":null}]}"#,
        )
        .expect("delta with tool_calls:null must decode");
        assert!(c.choices[0].delta.tool_calls.is_empty());

        let m: Message = serde_json::from_str(
            r#"{"role":"assistant","content":"hi","tool_calls":null}"#,
        )
        .expect("message with tool_calls:null must decode");
        assert!(m.tool_calls.is_empty());
    }

    #[test]
    fn deserialize_stream_chunk_tool_call_fragment() {
        // First fragment: id + name, partial args.
        let c: ChatChunk = serde_json::from_str(
            r#"{"id":"x","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"view_file","arguments":"{\"pa"}}]}}]}"#,
        )
        .unwrap();
        let d = &c.choices[0].delta.tool_calls[0];
        assert_eq!(d.index, 0);
        assert_eq!(d.id.as_deref(), Some("call_1"));
        assert_eq!(d.function.as_ref().unwrap().name.as_deref(), Some("view_file"));
        assert_eq!(d.function.as_ref().unwrap().arguments.as_deref(), Some("{\"pa"));

        // A text delta chunk.
        let t: ChatChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{"content":"hello"}}]}"#,
        )
        .unwrap();
        assert_eq!(t.choices[0].delta.content.as_deref(), Some("hello"));

        // A terminal chunk: finish_reason + usage (include_usage).
        let f: ChatChunk = serde_json::from_str(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":7,"total_tokens":12}}"#,
        )
        .unwrap();
        assert_eq!(f.choices[0].finish_reason, Some(FinishReason::Stop));
        assert_eq!(f.usage.unwrap().completion_tokens, Some(7));
    }
}
