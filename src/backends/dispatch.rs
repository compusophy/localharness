//! Shared tool-dispatch pipeline: pre-hook → execute → error-lift → post-hook.
//!
//! Every backend funnels its inline tool calls through
//! [`dispatch_tool_call`], so policies, hooks, and the `{"error": ...}`
//! lifting convention behave identically regardless of which model backend
//! requested the call.

use std::sync::Arc;

use serde_json::json;

use crate::hooks::{HookRunner, TurnContext};
use crate::tools::ToolRunner;
use crate::types::{HookResult, ToolCall, ToolResult};

/// Run one tool call through the full dispatch pipeline the backends share:
///
/// 1. pre-tool-call decide hooks (policies ride these) — a deny yields an
///    error result without executing the tool;
/// 2. [`ToolRunner::execute`];
/// 3. error lift — built-in tools encode failures as `{"error": "..."}` in
///    their `Ok` value; lift that into the typed [`ToolResult::error`] so
///    consumers (UI, hooks) can branch cleanly;
/// 4. post-tool-call hooks (observe the typed result).
///
/// The returned [`ToolResult`] always carries `result: Some(value)` — the
/// wire side needs a JSON value either way (the model sees errors as part of
/// the conversation) — with `error: Some(msg)` whenever execution didn't
/// produce a real result. `name`/`id` come from `call` (Anthropic correlates
/// by id; Gemini/mock/local leave it `None`).
pub(crate) async fn dispatch_tool_call(
    tool_runner: Option<&Arc<ToolRunner>>,
    hook_runner: Option<&Arc<HookRunner>>,
    turn_ctx: &TurnContext,
    call: &ToolCall,
) -> ToolResult {
    let (decision, op_ctx) = if let Some(hooks) = hook_runner {
        hooks.dispatch_pre_tool_call(turn_ctx, call).await
    } else {
        (HookResult::allow(), turn_ctx.clone())
    };

    let (result_value, error): (serde_json::Value, Option<String>) = if !decision.allow {
        let msg = decision.message.clone();
        (json!({ "error": msg.clone() }), Some(msg))
    } else if let Some(runner) = tool_runner {
        match runner.execute(&call.name, call.args.clone()).await {
            Ok(v) => {
                let err = v.get("error").and_then(|e| e.as_str()).map(String::from);
                (v, err)
            }
            Err(e) => {
                let s = e.to_string();
                (json!({ "error": s.clone() }), Some(s))
            }
        }
    } else {
        let s = format!("no tool runner registered for '{}'", call.name);
        (json!({ "error": s.clone() }), Some(s))
    };

    let result = ToolResult {
        name: call.name.clone(),
        id: call.id.clone(),
        result: Some(result_value),
        error,
    };
    if let Some(hooks) = hook_runner {
        hooks.dispatch_post_tool_call(&op_ctx, &result).await;
    }
    result
}
