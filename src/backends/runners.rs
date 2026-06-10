//! The runner bundle every backend strategy receives from the Agent.

use std::sync::Arc;

use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;

/// Runners the Agent injects into a backend's `ConnectionStrategy` (via its
/// `with_runners`) so tool calls dispatch inline through the same hooks +
/// policies + [`ToolRunner`] on every backend.
///
/// ONE shared struct — the per-backend names (`GeminiRunners`,
/// `AnthropicRunners`, `MockRunners`, `LocalRunners`) are type aliases of
/// this, kept so existing imports and struct literals don't churn.
#[derive(Default, Clone)]
pub struct BackendRunners {
    /// Tool runner for custom + built-in tool execution.
    pub tool_runner: Option<Arc<ToolRunner>>,
    /// Hook runner for pre/post tool-call hooks.
    pub hook_runner: Option<Arc<HookRunner>>,
    /// Session context for hook dispatch.
    pub session_ctx: Option<SessionContext>,
}
