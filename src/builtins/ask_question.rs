//! `ask_question` — interactive dialog with the user.
//!
//! The model calls `ask_question` to surface a multi-choice or freeform
//! question. The host renders the UI; in the Rust crate that means the
//! caller must register their own ask_question implementation if they
//! want anything more than the default "skipped" response.
//!
//! The default tool below is a no-op that always returns "skipped" —
//! safe to ship, lets the model keep going without crashing, and is
//! documented as overridable.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub struct AskQuestion;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for AskQuestion {
    fn name(&self) -> &str {
        "ask_question"
    }

    fn description(&self) -> &str {
        "Ask the user a clarifying question. Each question entry supports multiple choice \
         and/or freeform responses. The default host implementation skips all questions \
         — register a custom `ask_question` tool to wire interactive UI."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id":   { "type": "string" },
                                        "text": { "type": "string" }
                                    },
                                    "required": ["id", "text"]
                                }
                            },
                            "is_multi_select": { "type": "boolean" }
                        },
                        "required": ["question"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    async fn execute(&self, _args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        Ok(json!({
            "skipped": true,
            "responses": [],
            "note": "default ask_question tool — register a custom implementation to enable interactive UI"
        }))
    }
}
