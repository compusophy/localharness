//! Hook traits and the runner that dispatches them.
//!
//! Hooks come in two flavours:
//!
//! * **InspectHook** — observes an event, cannot block it. All registered
//!   hooks of a given kind run; their `Result::Err` is logged but does not
//!   abort the agent loop.
//! * **DecideHook** — gates an event behind a `HookResult`. The first deny
//!   wins; subsequent hooks of the same kind do not run.
//!
//! `HookContext` provides a per-scope JSON KV store that walks up the parent
//! chain. The session context is the root, a turn context is its child, and
//! per-tool-call operation contexts are children of the turn context.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use crate::runtime::MaybeSendSync;
use parking_lot::RwLock;
use tracing::warn;

use crate::content::Content;
use crate::error::Result;
use crate::types::{HookResult, ToolCall, ToolResult};

// =============================================================================
// Context
// =============================================================================

/// Hierarchical KV store. Reads walk up the parent chain; writes land on
/// the current scope only. Cloning is cheap (`Arc` semantics).
#[derive(Clone, Default)]
pub struct HookContext {
    inner: Arc<HookContextInner>,
}

#[derive(Default)]
struct HookContextInner {
    parent: Option<HookContext>,
    store: RwLock<HashMap<String, serde_json::Value>>,
}

impl HookContext {
    /// Create a root context with no parent.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a child scope that inherits reads from this context.
    pub fn child(&self) -> Self {
        Self {
            inner: Arc::new(HookContextInner {
                parent: Some(self.clone()),
                store: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Look up a value by key, walking up the parent chain.
    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        if let Some(v) = self.inner.store.read().get(key).cloned() {
            return Some(v);
        }
        self.inner.parent.as_ref().and_then(|p| p.get(key))
    }

    /// Store a value in this scope (does not affect parent scopes).
    pub fn set(&self, key: impl Into<String>, value: serde_json::Value) {
        self.inner.store.write().insert(key.into(), value);
    }
}

/// Root-level context for the entire session.
pub type SessionContext = HookContext;
/// Per-turn context, child of the session context.
pub type TurnContext = HookContext;
/// Per-tool-call context, child of the turn context.
pub type OperationContext = HookContext;

// =============================================================================
// Hook traits
// =============================================================================

/// Fires once when the agent session starts. Inspect-only (cannot block).
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait OnSessionStartHook: MaybeSendSync {
    /// Hook name for diagnostics.
    fn name(&self) -> &str {
        "on_session_start"
    }
    /// Called once after the connection opens.
    async fn run(&self, ctx: &SessionContext) -> Result<()>;
}

/// Fires once when the agent session ends. Inspect-only (cannot block).
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait OnSessionEndHook: MaybeSendSync {
    /// Hook name for diagnostics.
    fn name(&self) -> &str {
        "on_session_end"
    }
    /// Called once during shutdown.
    async fn run(&self, ctx: &SessionContext) -> Result<()>;
}

/// Gates each user turn. Return `HookResult::deny` to block the prompt.
///
/// Dispatched by every backend's turn loop BEFORE the prompt enters
/// conversation history, so a denied prompt never pollutes context. On deny
/// the model is not called; the turn surfaces as a stream error carrying
/// `"turn denied by hook: {reason}"`, so `chat()`/`text()` return `Err`.
/// First deny wins; subsequent hooks of this kind do not run.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PreTurnHook: MaybeSendSync {
    /// Hook name for diagnostics.
    fn name(&self) -> &str {
        "pre_turn"
    }
    /// Inspect the prompt and decide whether to allow it.
    async fn run(&self, ctx: &TurnContext, prompt: &Content) -> Result<HookResult>;
}

/// Fires after each model turn completes. Inspect-only.
///
/// Dispatched after the turn's terminal step is emitted, with the model's
/// final text for the turn. Does NOT fire for denied (pre-turn) or failed
/// (errored) turns.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PostTurnHook: MaybeSendSync {
    /// Hook name for diagnostics.
    fn name(&self) -> &str {
        "post_turn"
    }
    /// Called with the model's textual response.
    async fn run(&self, ctx: &TurnContext, response: &str) -> Result<()>;
}

/// Gates each tool call. Return `HookResult::deny` to block execution.
/// First deny wins; subsequent hooks are skipped.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PreToolCallDecideHook: MaybeSendSync {
    /// Hook name for diagnostics.
    fn name(&self) -> &str {
        "pre_tool_call_decide"
    }
    /// Inspect the tool call and decide whether to allow it.
    async fn run(&self, ctx: &OperationContext, call: &ToolCall) -> Result<HookResult>;
}

/// Fires after a tool call completes. Inspect-only.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PostToolCallHook: MaybeSendSync {
    /// Hook name for diagnostics.
    fn name(&self) -> &str {
        "post_tool_call"
    }
    /// Called with the tool's result (success or error).
    async fn run(&self, ctx: &OperationContext, result: &ToolResult) -> Result<()>;
}

// =============================================================================
// Runner
// =============================================================================

/// Dispatches registered hooks at the appropriate lifecycle points.
#[derive(Default)]
pub struct HookRunner {
    on_session_start: RwLock<Vec<Arc<dyn OnSessionStartHook>>>,
    on_session_end: RwLock<Vec<Arc<dyn OnSessionEndHook>>>,
    pre_turn: RwLock<Vec<Arc<dyn PreTurnHook>>>,
    post_turn: RwLock<Vec<Arc<dyn PostTurnHook>>>,
    pre_tool_call_decide: RwLock<Vec<Arc<dyn PreToolCallDecideHook>>>,
    post_tool_call: RwLock<Vec<Arc<dyn PostToolCallHook>>>,
}

impl HookRunner {
    /// Create an empty hook runner.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an on-session-start hook.
    pub fn register_on_session_start(&self, hook: Arc<dyn OnSessionStartHook>) {
        self.on_session_start.write().push(hook);
    }
    /// Register an on-session-end hook.
    pub fn register_on_session_end(&self, hook: Arc<dyn OnSessionEndHook>) {
        self.on_session_end.write().push(hook);
    }
    /// Register a pre-turn hook.
    pub fn register_pre_turn(&self, hook: Arc<dyn PreTurnHook>) {
        self.pre_turn.write().push(hook);
    }
    /// Register a post-turn hook.
    pub fn register_post_turn(&self, hook: Arc<dyn PostTurnHook>) {
        self.post_turn.write().push(hook);
    }
    /// Register a pre-tool-call decide hook.
    pub fn register_pre_tool_call_decide(&self, hook: Arc<dyn PreToolCallDecideHook>) {
        self.pre_tool_call_decide.write().push(hook);
    }
    /// Register a post-tool-call hook.
    pub fn register_post_tool_call(&self, hook: Arc<dyn PostToolCallHook>) {
        self.post_tool_call.write().push(hook);
    }

    /// True if at least one pre-tool-call decide hook is registered.
    pub fn has_pre_tool_call_decide(&self) -> bool {
        !self.pre_tool_call_decide.read().is_empty()
    }

    /// Run all on-session-start hooks (errors are logged, not propagated).
    pub async fn dispatch_session_start(&self, ctx: &SessionContext) {
        let hooks = self.on_session_start.read().clone();
        for h in hooks {
            if let Err(e) = h.run(ctx).await {
                warn!(name = h.name(), error = %e, "on_session_start hook failed");
            }
        }
    }

    /// Run all on-session-end hooks (errors are logged, not propagated).
    pub async fn dispatch_session_end(&self, ctx: &SessionContext) {
        let hooks = self.on_session_end.read().clone();
        for h in hooks {
            if let Err(e) = h.run(ctx).await {
                warn!(name = h.name(), error = %e, "on_session_end hook failed");
            }
        }
    }

    /// Run pre-turn hooks; first deny wins.
    pub async fn dispatch_pre_turn(&self, ctx: &TurnContext, prompt: &Content) -> HookResult {
        let hooks = self.pre_turn.read().clone();
        for h in hooks {
            match h.run(ctx, prompt).await {
                Ok(result) if !result.allow => return result,
                Ok(_) => {}
                Err(e) => {
                    warn!(name = h.name(), error = %e, "pre_turn hook errored");
                    return HookResult::deny(format!("hook '{}' error: {e}", h.name()));
                }
            }
        }
        HookResult::allow()
    }

    /// Run all post-turn hooks (errors are logged, not propagated).
    pub async fn dispatch_post_turn(&self, ctx: &TurnContext, response: &str) {
        let hooks = self.post_turn.read().clone();
        for h in hooks {
            if let Err(e) = h.run(ctx, response).await {
                warn!(name = h.name(), error = %e, "post_turn hook failed");
            }
        }
    }

    /// Returns `(HookResult, OperationContext)`. The op context lets the
    /// matching post-tool hook see anything the decide hook stashed.
    pub async fn dispatch_pre_tool_call(
        &self,
        turn_ctx: &TurnContext,
        call: &ToolCall,
    ) -> (HookResult, OperationContext) {
        let op_ctx = turn_ctx.child();
        let hooks = self.pre_tool_call_decide.read().clone();
        for h in hooks {
            match h.run(&op_ctx, call).await {
                Ok(result) if !result.allow => return (result, op_ctx),
                Ok(_) => {}
                Err(e) => {
                    warn!(name = h.name(), error = %e, "pre_tool_call hook errored");
                    return (
                        HookResult::deny(format!("hook '{}' error: {e}", h.name())),
                        op_ctx,
                    );
                }
            }
        }
        (HookResult::allow(), op_ctx)
    }

    /// Run all post-tool-call hooks (errors are logged, not propagated).
    pub async fn dispatch_post_tool_call(&self, op_ctx: &OperationContext, result: &ToolResult) {
        let hooks = self.post_tool_call.read().clone();
        for h in hooks {
            if let Err(e) = h.run(op_ctx, result).await {
                warn!(name = h.name(), error = %e, "post_tool_call hook failed");
            }
        }
    }
}
