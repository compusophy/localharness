//! `finish` — special tool the model calls to terminate the turn.
//!
//! When `response_schema` is configured, the model is asked to call
//! `finish` with the structured payload as the `output` arg; the loop
//! pulls that out and stashes it as `structured_output` on the
//! conversation state.
//!
//! The loop recognises `finish` by name (see [`FINISH_TOOL_NAME`]) and
//! exits the outer dispatch loop after handling it — `execute` here is
//! just a no-op that echoes the args so the function response is well
//! formed for the model's history.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub const FINISH_TOOL_NAME: &str = "finish";

pub struct Finish;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for Finish {
    fn name(&self) -> &str {
        FINISH_TOOL_NAME
    }

    fn description(&self) -> &str {
        "Signal that the turn is complete. Pass `summary` with a short final message \
         to the user (one or two sentences) when your prior turns only showed tool \
         activity — it is rendered as your closing reply so the user isn't left with \
         a silent completion. Pass `output` when the agent is configured with a \
         response schema; it will be returned to the caller as the structured output \
         of this turn."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "Optional short closing message to show the user when the turn would otherwise end silently after tool calls." },
                "output": { "description": "Optional structured output. Must conform to the agent's response_schema when one is set." }
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        // The runtime intercepts this call to extract `output` before we run.
        // Returning the args back lets the model see a well-formed response
        // even if the runtime didn't intercept (defensive).
        Ok(json!({ "ok": true, "args": args }))
    }
}
