//! Gemini REST request/response types.
//!
//! Field naming matches the Gemini API verbatim (camelCase) — every
//! struct sets `#[serde(rename_all = "camelCase")]`. Untagged enums are
//! used for the `parts` array since Gemini returns one of several
//! shapes per element.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::UsageMetadata;

// =============================================================================
// Request
// =============================================================================

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContentRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_instruction: Option<Content>,
    pub contents: Vec<Content>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools: Vec<ToolDecl>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_config: Option<ToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generation_config: Option<GenerationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Content {
    pub role: ContentRole,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ContentRole {
    User,
    Model,
}

/// A single content part.
///
/// Gemini returns parts as `{"text": "..."}`, `{"thought": true, "text":
/// "..."}`, `{"functionCall": {...}}`, etc. We deserialize by matching
/// the first present field rather than relying on a tag — that's how
/// the Gemini API actually wires it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Part {
    /// A function/tool call from the model.
    FunctionCall {
        #[serde(rename = "functionCall")]
        function_call: FunctionCall,
        // Gemini 3.x stamps every functionCall part with an opaque
        // `thoughtSignature` and REJECTS (HTTP 400 INVALID_ARGUMENT) any
        // replayed history whose functionCall parts lack it. Capture it on
        // decode and echo it back verbatim on encode — see `loop.rs` where
        // the model turn is rebuilt into history.
        #[serde(
            rename = "thoughtSignature",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        thought_signature: Option<String>,
    },
    /// Our response to a function call.
    FunctionResponse {
        #[serde(rename = "functionResponse")]
        function_response: FunctionResponse,
    },
    /// Inline binary data (image, etc.).
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
    /// Model reasoning (when `thinkingConfig` is enabled). Discriminated
    /// by `thought: true`.
    Thought {
        thought: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        // Gemini wires this as `thoughtSignature` (camelCase). The enum is
        // untagged so there's no enum-level `rename_all`; rename explicitly
        // or it deserializes to None (and re-serializes under the wrong key
        // when echoing thinking history back to the model).
        #[serde(
            rename = "thoughtSignature",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        thought_signature: Option<String>,
    },
    /// Plain text. Keep last in the enum so it's only matched after
    /// the more specific shapes above.
    Text { text: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCall {
    pub name: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FunctionResponse {
    pub name: String,
    pub response: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InlineData {
    pub mime_type: String,
    /// Base64-encoded payload.
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolDecl {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub function_declarations: Vec<FunctionDeclaration>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FunctionDeclaration {
    pub name: String,
    pub description: String,
    /// JSON Schema describing the function's args. Free-form Value so the
    /// caller can supply any valid schema.
    pub parameters: Value,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolConfig {
    pub function_calling_config: FunctionCallingConfig,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FunctionCallingConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<FunctionCallingMode>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum FunctionCallingMode {
    /// Model decides whether to call a function.
    Auto,
    /// Model must call a function (or finish).
    Any,
    /// Model must not call functions.
    None,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_config: Option<ThinkingConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mime_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// Token budget for internal reasoning. 0 disables thinking.
    pub thinking_budget: u32,
    /// Whether the model includes thought parts in the response.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub include_thoughts: Option<bool>,
}

// =============================================================================
// Streaming response
// =============================================================================

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct GenerateChunk {
    #[serde(default)]
    pub candidates: Vec<Candidate>,
    #[serde(default)]
    pub usage_metadata: Option<WireUsage>,
    /// Some chunks carry only `modelVersion` or `responseId` metadata — we
    /// ignore those without erroring.
    #[serde(default)]
    pub model_version: Option<String>,
    #[serde(default)]
    pub response_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub content: Option<Content>,
    #[serde(default)]
    pub finish_reason: Option<FinishReason>,
    #[serde(default)]
    pub index: Option<u32>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FinishReason {
    Stop,
    MaxTokens,
    Safety,
    Recitation,
    /// The model wants to call a function; consume queued FunctionCall parts.
    ToolUse,
    Language,
    Other,
    Blocklist,
    ProhibitedContent,
    Spii,
    MalformedFunctionCall,
    FinishReasonUnspecified,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WireUsage {
    #[serde(default)]
    pub prompt_token_count: Option<i32>,
    #[serde(default)]
    pub cached_content_token_count: Option<i32>,
    #[serde(default)]
    pub candidates_token_count: Option<i32>,
    #[serde(default)]
    pub thoughts_token_count: Option<i32>,
    #[serde(default)]
    pub total_token_count: Option<i32>,
}

impl From<WireUsage> for UsageMetadata {
    fn from(w: WireUsage) -> Self {
        UsageMetadata {
            prompt_token_count: w.prompt_token_count,
            cached_content_token_count: w.cached_content_token_count,
            candidates_token_count: w.candidates_token_count,
            thoughts_token_count: w.thoughts_token_count,
            total_token_count: w.total_token_count,
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

impl Content {
    pub fn user_text(text: impl Into<String>) -> Self {
        Self {
            role: ContentRole::User,
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    pub fn model_text(text: impl Into<String>) -> Self {
        Self {
            role: ContentRole::Model,
            parts: vec![Part::Text { text: text.into() }],
        }
    }

    pub fn system_text(text: impl Into<String>) -> Self {
        // Gemini's `systemInstruction` is the same shape as a content,
        // but role is conventionally omitted. We send role=user and
        // it's accepted (the server ignores role on systemInstruction).
        Self {
            role: ContentRole::User,
            parts: vec![Part::Text { text: text.into() }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_text_part() {
        let p: Part = serde_json::from_str(r#"{"text":"hello"}"#).unwrap();
        assert!(matches!(p, Part::Text { ref text } if text == "hello"));
    }

    #[test]
    fn deserialize_thought_part() {
        let p: Part =
            serde_json::from_str(r#"{"thought":true,"text":"reasoning..."}"#).unwrap();
        match p {
            Part::Thought { thought, text, .. } => {
                assert!(thought);
                assert_eq!(text.as_deref(), Some("reasoning..."));
            }
            other => panic!("expected Thought, got {other:?}"),
        }
    }

    #[test]
    fn deserialize_function_call_part() {
        let json = r#"{"functionCall":{"name":"view_file","args":{"path":"x.txt"}}}"#;
        let p: Part = serde_json::from_str(json).unwrap();
        match p {
            Part::FunctionCall { function_call, .. } => {
                assert_eq!(function_call.name, "view_file");
                assert_eq!(function_call.args["path"], "x.txt");
            }
            other => panic!("expected FunctionCall, got {other:?}"),
        }
    }

    /// THE thought-signature 400 regression (Gemini 3.x): a `functionCall`
    /// part arrives stamped with an opaque `thoughtSignature`, and the API
    /// rejects any replayed history whose functionCall parts lack it
    /// ("Function call is missing a thought_signature in functionCall
    /// parts" — bricked every multi-round tool turn). The signature must
    /// survive a decode → re-encode round trip byte-for-byte.
    #[test]
    fn function_call_thought_signature_round_trips() {
        let json =
            r#"{"functionCall":{"name":"f","args":{}},"thoughtSignature":"AbC123="}"#;
        let p: Part = serde_json::from_str(json).unwrap();
        match &p {
            Part::FunctionCall {
                thought_signature, ..
            } => assert_eq!(thought_signature.as_deref(), Some("AbC123=")),
            other => panic!("expected FunctionCall, got {other:?}"),
        }
        let out = serde_json::to_value(&p).unwrap();
        assert_eq!(out["thoughtSignature"], "AbC123=");
        assert_eq!(out["functionCall"]["name"], "f");
    }

    /// A functionCall WITHOUT a signature (pre-3.x histories persisted in
    /// OPFS) must keep decoding, and re-encoding must omit the key entirely
    /// (not emit `"thoughtSignature":null`, which the API also rejects).
    #[test]
    fn function_call_without_signature_omits_key_on_encode() {
        let p: Part =
            serde_json::from_str(r#"{"functionCall":{"name":"f","args":{}}}"#).unwrap();
        let out = serde_json::to_value(&p).unwrap();
        assert!(matches!(p, Part::FunctionCall { ref thought_signature, .. } if thought_signature.is_none()));
        assert!(out.get("thoughtSignature").is_none());
    }

    /// THE documented Gemini 3.x quirk: a *visible-text* part arrives stamped
    /// with `thought: false`. Because the untagged `Part::Thought` variant
    /// precedes `Part::Text` and only requires the `thought` key, such a part
    /// deserializes into `Thought { thought: false, text: Some(_) }`, NOT
    /// `Text`. Any consumer matching only `Text` (or only `thought: true`)
    /// would silently drop the model's output. This test pins the shape so
    /// the streaming loop's `thought: false` arm can't regress unnoticed.
    #[test]
    fn thought_false_text_is_thought_variant_not_text() {
        let p: Part = serde_json::from_str(r#"{"thought":false,"text":"hi"}"#).unwrap();
        match p {
            Part::Thought {
                thought: false,
                text: Some(t),
                ..
            } => assert_eq!(t, "hi"),
            other => panic!(
                "Gemini 3.x stamps text parts with thought:false; expected \
                 Thought{{thought:false,text}}, got {other:?}"
            ),
        }
    }

    /// A *normal* text part with NO `thought` key must still land on `Text`
    /// (the `Thought` variant requires `thought`, which is absent → it falls
    /// through to `Text`). This is the pre-3.x shape and must keep working.
    #[test]
    fn plain_text_without_thought_key_is_text() {
        let p: Part = serde_json::from_str(r#"{"text":"plain"}"#).unwrap();
        assert!(matches!(p, Part::Text { ref text } if text == "plain"));
    }

    /// A part carrying BOTH `text` and `functionCall`. `FunctionCall` is the
    /// first untagged variant and matches on the `functionCall` key alone, so
    /// such a hybrid resolves to `FunctionCall` and the stray `text` is lost.
    /// This documents the precedence so callers know a function-call part is
    /// never *also* surfaced as text.
    #[test]
    fn text_plus_function_call_resolves_to_function_call() {
        let p: Part =
            serde_json::from_str(r#"{"text":"hi","functionCall":{"name":"f","args":{}}}"#)
                .unwrap();
        assert!(matches!(p, Part::FunctionCall { .. }));
    }

    /// A `functionCall` stamped with `thought:false` (Gemini 3.x stamps every
    /// part) still resolves to `FunctionCall`, because that variant precedes
    /// `Thought` in declaration order.
    #[test]
    fn function_call_with_thought_false_stamp_resolves_to_function_call() {
        let p: Part = serde_json::from_str(
            r#"{"thought":false,"functionCall":{"name":"f","args":{}}}"#,
        )
        .unwrap();
        assert!(matches!(p, Part::FunctionCall { .. }));
    }

    /// A thought part with NO `text` (a thought-signature-only part). Must
    /// deserialize without error and leave `text` None — the streaming loop
    /// guards on `text: Some(_)` so a None-text thought is harmlessly ignored.
    #[test]
    fn thought_without_text_deserializes() {
        let p: Part = serde_json::from_str(r#"{"thought":true}"#).unwrap();
        assert!(matches!(p, Part::Thought { thought: true, text: None, .. }));
    }

    /// Unknown / future fields alongside a known one must not break
    /// deserialization (untagged variants ignore extra object keys).
    #[test]
    fn unknown_extra_fields_are_tolerated() {
        let p: Part =
            serde_json::from_str(r#"{"text":"hi","videoMetadata":{"x":1}}"#).unwrap();
        assert!(matches!(p, Part::Text { ref text } if text == "hi"));
    }

    /// `thoughtSignature` rides along on a thought part (Gemini sends an
    /// opaque base64 signature). It must be captured, not rejected.
    #[test]
    fn thought_signature_is_captured() {
        let p: Part = serde_json::from_str(
            r#"{"thought":true,"text":"r","thoughtSignature":"AbC="}"#,
        )
        .unwrap();
        match p {
            Part::Thought {
                thought_signature, ..
            } => assert_eq!(thought_signature.as_deref(), Some("AbC=")),
            other => panic!("expected Thought, got {other:?}"),
        }
    }

    /// An unknown `finishReason` string must map to `Unknown` (via
    /// `#[serde(other)]`) rather than failing the whole chunk decode — a new
    /// server-side reason should never brick streaming.
    #[test]
    fn unknown_finish_reason_maps_to_unknown() {
        let json = r#"{"candidates":[{"finishReason":"SOME_NEW_REASON_2027"}]}"#;
        let chunk: GenerateChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.candidates[0].finish_reason, Some(FinishReason::Unknown));
    }

    /// A chunk that carries ONLY metadata (`modelVersion` / `responseId`, no
    /// candidates) decodes to an empty-candidates chunk, not an error.
    #[test]
    fn metadata_only_chunk_decodes_empty() {
        let json = r#"{"modelVersion":"gemini-3.5-flash","responseId":"abc123"}"#;
        let chunk: GenerateChunk = serde_json::from_str(json).unwrap();
        assert!(chunk.candidates.is_empty());
        assert_eq!(chunk.model_version.as_deref(), Some("gemini-3.5-flash"));
    }

    #[test]
    fn round_trip_chunk() {
        let json = r#"{
            "candidates": [{
                "content": {"role":"model","parts":[{"text":"hi"}]},
                "finishReason": "STOP"
            }],
            "usageMetadata": {"promptTokenCount":3,"candidatesTokenCount":1,"totalTokenCount":4}
        }"#;
        let chunk: GenerateChunk = serde_json::from_str(json).unwrap();
        assert_eq!(chunk.candidates.len(), 1);
        assert_eq!(chunk.candidates[0].finish_reason, Some(FinishReason::Stop));
        let usage: UsageMetadata = chunk.usage_metadata.unwrap().into();
        assert_eq!(usage.total_token_count, Some(4));
    }
}
