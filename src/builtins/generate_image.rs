//! `generate_image` — produce an image from a text prompt.
//!
//! Calls the Gemini image-generation model (non-streaming
//! `generateContent`). The response carries the bytes inline as base64;
//! we decode and surface them as a structured `{ mime_type, data }`
//! pair plus a convenience `bytes_len` field.
//!
//! The model name and a shared `GeminiClient` are bound to the tool at
//! construction time — that's why the strategy hands them in via
//! `register_builtins_with_image_client`.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};

use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::wire::{
    Content, ContentRole, GenerateContentRequest, Part,
};
use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

pub struct GenerateImage {
    client: SharedClient,
    model: String,
}

impl GenerateImage {
    pub fn new(client: SharedClient, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    struct Args: serde {
        prompt: req_str = "Description of the image to generate.",
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for GenerateImage {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        "Generate an image from a text prompt. Returns { mime_type, data_base64, bytes_len } \
         where data_base64 is the standard base64-encoded image bytes."
    }

    fn input_schema(&self) -> Value {
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("generate_image args: {e}")))?;
        let req = GenerateContentRequest {
            contents: vec![Content {
                role: ContentRole::User,
                parts: vec![Part::Text { text: args.prompt }],
            }],
            ..Default::default()
        };
        let chunk = self.client.generate(&self.model, &req).await?;
        let Some(candidate) = chunk.candidates.into_iter().next() else {
            return Err(Error::other("image model returned no candidates"));
        };
        let Some(content) = candidate.content else {
            return Err(Error::other("image candidate has no content"));
        };
        for part in content.parts {
            if let Part::InlineData { inline_data } = part {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(&inline_data.data)
                    .map_err(|e| Error::other(format!("image base64 decode: {e}")))?;
                return Ok(json!({
                    "mime_type": inline_data.mime_type,
                    "data_base64": inline_data.data,
                    "bytes_len": bytes.len(),
                }));
            }
        }
        Err(Error::other(
            "image model response carried no inlineData part",
        ))
    }
}

#[cfg(test)]
mod schema_tests {
    use super::Args;
    use serde_json::json;

    /// BYTE-IDENTITY: the macro-generated schema must serialize byte-for-byte
    /// equal to the hand-written literal it replaced (frozen verbatim here) —
    /// the wire shape is model-behavior-load-bearing.
    #[test]
    fn schema_is_byte_identical_to_the_frozen_original() {
        let frozen = json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "Description of the image to generate." }
            },
            "required": ["prompt"]
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
    }
}
