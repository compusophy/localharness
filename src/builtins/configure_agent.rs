//! `configure_agent` — let the chat agent read or edit its OWN config
//! (the `agent.json` manifest): its custom system prompt and its tool
//! allowlist, or reset both to defaults.
//!
//! This is the agent-facing mirror of the admin UI's config tab — both
//! write the same manifest via `crate::app::agent_config`. Changes to the
//! system prompt / allowlist take effect on the NEXT session, since the
//! prompt + capabilities are baked when the session starts.

use std::sync::Arc;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolContext};

pub struct ConfigureAgent;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for ConfigureAgent {
    fn name(&self) -> &str {
        "configure_agent"
    }

    fn description(&self) -> &str {
        "Read or change THIS agent's own configuration (stored in the \
         agent.json manifest): its custom system prompt and its tool \
         allowlist. Call with no arguments to read the current config. \
         Pass `system_prompt` (a string) to set a custom prompt, or \
         `system_prompt: null` to clear it (revert to default). Pass \
         `tools` (an array of tool wire-names, e.g. [\"view_file\", \
         \"create_file\"]) to restrict which tools are enabled, or \
         `tools: null` to allow all. Pass `reset: true` to clear BOTH back \
         to factory defaults. Note: the tools finish, ask_question, and \
         configure_agent are always enabled and cannot be removed. Config \
         changes apply on the agent's NEXT session, not mid-conversation."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                // NOTE: Gemini's function-declaration schema rejects
                // array-valued (union) `type` like ["string","null"] with a
                // 400 — it must be a single type. Nullability is conveyed in
                // the description + the `reset` flag; the model can still
                // omit the field or pass null at the value level.
                "system_prompt": {
                    "type": "string",
                    "description": "New custom system prompt; null/empty clears it."
                },
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Allowlisted tool wire-names; omit or null allows all."
                },
                "reset": {
                    "type": "boolean",
                    "description": "If true, clear both prompt and allowlist to defaults."
                }
            }
        })
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let reset = args.get("reset").and_then(|v| v.as_bool()).unwrap_or(false);
        let has_prompt = args.get("system_prompt").is_some();
        let has_tools = args.get("tools").is_some();

        #[cfg(all(target_arch = "wasm32", feature = "browser-app"))]
        {
            use crate::app::agent_config;
            if reset {
                agent_config::set_system_prompt(None).await.map_err(crate::error::Error::other)?;
                agent_config::set_tools(None).await.map_err(crate::error::Error::other)?;
                return Ok(json!({ "status": "reset to defaults", "applies": "next session" }));
            }

            if has_prompt {
                let prompt = args.get("system_prompt").and_then(|v| v.as_str());
                agent_config::set_system_prompt(prompt).await.map_err(crate::error::Error::other)?;
            }
            if has_tools {
                match args.get("tools").and_then(|v| v.as_array()) {
                    Some(arr) => {
                        let names: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect();
                        let tools: Vec<crate::types::BuiltinTool> = names
                            .iter()
                            .filter_map(|n| {
                                crate::types::BuiltinTool::ALL
                                    .iter()
                                    .find(|t| t.wire_name() == n)
                                    .copied()
                            })
                            .collect();
                        agent_config::set_tools(Some(tools.as_slice())).await.map_err(crate::error::Error::other)?;
                    }
                    None => {
                        // tools: null -> unrestricted
                        agent_config::set_tools(None).await.map_err(crate::error::Error::other)?;
                    }
                }
            }

            // Always report the current resolved config back.
            let manifest = agent_config::load().await;
            return Ok(json!({
                "status": if has_prompt || has_tools { "saved" } else { "current config" },
                "system_prompt": manifest.system_prompt,
                "tools": manifest.tools,
                "applies": "next session"
            }));
        }

        #[cfg(not(all(target_arch = "wasm32", feature = "browser-app")))]
        {
            let _ = (reset, has_prompt, has_tools);
            Ok(json!({
                "configured": false,
                "note": "agent config requires the browser app"
            }))
        }
    }
}
