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

crate::tool_params! {
    /// ONE table generates both this struct and `input_schema` (see
    /// `crate::tool_params`); the schema byte-identity test is below.
    /// Nullability is conveyed in the descriptions + the `reset` flag — the
    /// grammar (like Gemini) can't express a `["string","null"]` union. The
    /// body ALSO reads the raw args: absent-vs-`null` (`tools: null` clears,
    /// absent leaves) is a distinction the parsed struct can't carry.
    struct Args: lenient {
        system_prompt: opt_str = "New custom system prompt; null/empty clears it.",
        tools: opt_str_array = "Allowlisted tool wire-names; omit or null allows all.",
        reset: opt_bool = "If true, clear both prompt and allowlist to defaults.",
    }
}

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
        Args::schema()
    }

    async fn execute(&self, args: Value, _ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        let p = Args::lenient(&args);
        let reset = p.reset.unwrap_or(false);
        // Presence checks stay on the RAW args: `tools: null` / `system_prompt:
        // null` (present) clear, while an absent field leaves things alone —
        // the parsed struct maps both to `None`.
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
                agent_config::set_system_prompt(p.system_prompt.as_deref())
                    .await
                    .map_err(crate::error::Error::other)?;
            }
            if has_tools {
                match &p.tools {
                    Some(names) => {
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
            let _ = (reset, has_prompt, has_tools, p.system_prompt, p.tools);
            Ok(json!({
                "configured": false,
                "note": "agent config requires the browser app"
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BYTE-IDENTITY: the generated schema serializes byte-for-byte equal to
    /// the original hand-written literal it replaced (frozen verbatim below —
    /// all-optional, so there is NO `required` key).
    #[test]
    fn schema_is_byte_identical_to_the_frozen_original() {
        let frozen = json!({
            "type": "object",
            "properties": {
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
        });
        assert_eq!(Args::schema().to_string(), frozen.to_string());
    }

    /// Lenient parity with the historical inline chains — including the
    /// null-vs-absent cases the body still distinguishes off the raw args.
    #[test]
    fn lenient_matches_the_old_inline_extraction() {
        let p = Args::lenient(&serde_json::json!({}));
        assert_eq!((p.system_prompt, p.tools, p.reset), (None, None, None));
        // `tools: null` parses to None (the body's has_tools raw check routes
        // it to "unrestricted"); an array filter_maps its string entries.
        let raw = serde_json::json!({"system_prompt": null, "tools": null, "reset": true});
        let p = Args::lenient(&raw);
        assert_eq!(p.system_prompt, None);
        assert_eq!(p.tools, None);
        assert!(p.reset.unwrap_or(false));
        assert!(raw.get("tools").is_some()); // the raw presence check survives
        let p = Args::lenient(&serde_json::json!({"tools": ["view_file", 3, "finish"]}));
        assert_eq!(p.tools, Some(vec!["view_file".to_string(), "finish".to_string()]));
    }
}
