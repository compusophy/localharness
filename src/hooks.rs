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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn child(&self) -> Self {
        Self {
            inner: Arc::new(HookContextInner {
                parent: Some(self.clone()),
                store: RwLock::new(HashMap::new()),
            }),
        }
    }

    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        if let Some(v) = self.inner.store.read().get(key).cloned() {
            return Some(v);
        }
        self.inner.parent.as_ref().and_then(|p| p.get(key))
    }

    pub fn set(&self, key: impl Into<String>, value: serde_json::Value) {
        self.inner.store.write().insert(key.into(), value);
    }
}

pub type SessionContext = HookContext;
pub type TurnContext = HookContext;
pub type OperationContext = HookContext;

// =============================================================================
// Hook traits
// =============================================================================

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait OnSessionStartHook: MaybeSendSync {
    fn name(&self) -> &str {
        "on_session_start"
    }
    async fn run(&self, ctx: &SessionContext) -> Result<()>;
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait OnSessionEndHook: MaybeSendSync {
    fn name(&self) -> &str {
        "on_session_end"
    }
    async fn run(&self, ctx: &SessionContext) -> Result<()>;
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PreTurnHook: MaybeSendSync {
    fn name(&self) -> &str {
        "pre_turn"
    }
    async fn run(&self, ctx: &TurnContext, prompt: &Content) -> Result<HookResult>;
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PostTurnHook: MaybeSendSync {
    fn name(&self) -> &str {
        "post_turn"
    }
    async fn run(&self, ctx: &TurnContext, response: &str) -> Result<()>;
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PreToolCallDecideHook: MaybeSendSync {
    fn name(&self) -> &str {
        "pre_tool_call_decide"
    }
    async fn run(&self, ctx: &OperationContext, call: &ToolCall) -> Result<HookResult>;
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait PostToolCallHook: MaybeSendSync {
    fn name(&self) -> &str {
        "post_tool_call"
    }
    async fn run(&self, ctx: &OperationContext, result: &ToolResult) -> Result<()>;
}

// =============================================================================
// Runner
// =============================================================================

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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_on_session_start(&self, hook: Arc<dyn OnSessionStartHook>) {
        self.on_session_start.write().push(hook);
    }
    pub fn register_on_session_end(&self, hook: Arc<dyn OnSessionEndHook>) {
        self.on_session_end.write().push(hook);
    }
    pub fn register_pre_turn(&self, hook: Arc<dyn PreTurnHook>) {
        self.pre_turn.write().push(hook);
    }
    pub fn register_post_turn(&self, hook: Arc<dyn PostTurnHook>) {
        self.post_turn.write().push(hook);
    }
    pub fn register_pre_tool_call_decide(&self, hook: Arc<dyn PreToolCallDecideHook>) {
        self.pre_tool_call_decide.write().push(hook);
    }
    pub fn register_post_tool_call(&self, hook: Arc<dyn PostToolCallHook>) {
        self.post_tool_call.write().push(hook);
    }

    pub fn has_pre_tool_call_decide(&self) -> bool {
        !self.pre_tool_call_decide.read().is_empty()
    }

    pub async fn dispatch_session_start(&self, ctx: &SessionContext) {
        let hooks = self.on_session_start.read().clone();
        for h in hooks {
            if let Err(e) = h.run(ctx).await {
                warn!(name = h.name(), error = %e, "on_session_start hook failed");
            }
        }
    }

    pub async fn dispatch_session_end(&self, ctx: &SessionContext) {
        let hooks = self.on_session_end.read().clone();
        for h in hooks {
            if let Err(e) = h.run(ctx).await {
                warn!(name = h.name(), error = %e, "on_session_end hook failed");
            }
        }
    }

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

    pub async fn dispatch_post_tool_call(&self, op_ctx: &OperationContext, result: &ToolResult) {
        let hooks = self.post_tool_call.read().clone();
        for h in hooks {
            if let Err(e) = h.run(op_ctx, result).await {
                warn!(name = h.name(), error = %e, "post_tool_call hook failed");
            }
        }
    }
}
