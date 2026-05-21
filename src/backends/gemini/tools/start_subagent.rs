//! `start_subagent` — spawn a one-shot subagent.
//!
//! The parent agent calls `start_subagent` to delegate a self-contained
//! task: an isolated context (no shared history), a single user prompt,
//! its own system instructions. The subagent runs against the same
//! Gemini client + model and returns its final text response.
//!
//! No tool dispatch in v1 — the subagent can only produce text. Tool
//! delegation, recursion limits, and parallel fan-out are 0.4.x work.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::wire::{
    Content, ContentRole, FinishReason, GenerateContentRequest, Part,
};
use crate::error::{Error, Result};
use crate::tools::{Tool, ToolContext};

pub struct StartSubagent {
    client: SharedClient,
    model: String,
}

impl StartSubagent {
    pub fn new(client: SharedClient, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }
}

#[derive(Deserialize)]
struct Args {
    system_instructions: String,
    prompt: String,
}

#[async_trait]
impl Tool for StartSubagent {
    fn name(&self) -> &str {
        "start_subagent"
    }

    fn description(&self) -> &str {
        "Spawn a one-shot subagent with isolated context. The subagent receives the \
         given `system_instructions` and `prompt`, runs against the same model as the \
         parent, and returns its final text response. The subagent has no access to \
         tools (text-only)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "system_instructions": { "type": "string", "description": "System instructions for the subagent's persona / role." },
                "prompt": { "type": "string", "description": "The user message to send to the subagent." }
            },
            "required": ["system_instructions", "prompt"]
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let args: Args = serde_json::from_value(args)
            .map_err(|e| Error::other(format!("start_subagent args: {e}")))?;
        let req = GenerateContentRequest {
            system_instruction: Some(Content {
                role: ContentRole::User,
                parts: vec![Part::Text {
                    text: args.system_instructions,
                }],
            }),
            contents: vec![Content {
                role: ContentRole::User,
                parts: vec![Part::Text { text: args.prompt }],
            }],
            ..Default::default()
        };

        let mut stream = self.client.stream_generate(&self.model, &req).await?;
        let mut text = String::new();
        let mut finish_reason: Option<FinishReason> = None;
        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res?;
            for cand in chunk.candidates {
                if let Some(content) = cand.content {
                    for part in content.parts {
                        if let Part::Text { text: t } = part {
                            text.push_str(&t);
                        }
                    }
                }
                if let Some(r) = cand.finish_reason {
                    finish_reason = Some(r);
                }
            }
        }

        Ok(json!({
            "final_response": text,
            "finish_reason": format!("{:?}", finish_reason.unwrap_or(FinishReason::Stop)),
        }))
    }
}
