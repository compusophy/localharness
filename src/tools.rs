//! Host-side custom tools.
//!
//! A `Tool` is anything that exposes a JSON-schema-described entry point and
//! produces JSON back. `ToolRunner` registers tools by name and dispatches
//! calls from the harness. The optional `ToolContext` gives tools a handle
//! back to the live connection so they can stream out-of-band messages.

use std::collections::HashMap;
use std::sync::Arc;

use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use parking_lot::RwLock;
use serde_json::Value;

use crate::connections::Connection;
use crate::error::{Error, Result};
use crate::runtime::MaybeSendSync;
use crate::types::{ToolCall, ToolResult};

// =============================================================================
// Tool trait
// =============================================================================

/// A named, schema-described function the model can call.
///
/// Implement this trait to expose custom logic to the agent. Register
/// instances via [`ToolRunner::register`] or [`AgentConfig::with_tool`].
///
/// [`AgentConfig::with_tool`]: crate::AgentConfig::with_tool
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Tool: MaybeSendSync {
    /// Unique wire name the model uses to invoke this tool.
    fn name(&self) -> &str;
    /// Human-readable description shown to the model.
    fn description(&self) -> &str;
    /// JSON Schema describing the expected arguments.
    fn input_schema(&self) -> Value;
    /// Run the tool with the given arguments and return a JSON result.
    async fn execute(&self, args: Value, ctx: Option<Arc<ToolContext>>) -> Result<Value>;
}

// =============================================================================
// Tool context
// =============================================================================

/// Runtime context available to tools during execution.
///
/// Provides access to the live connection (for sending out-of-band messages)
/// and a per-session key-value store for cross-tool state.
pub struct ToolContext {
    connection: Arc<dyn Connection>,
    state: RwLock<HashMap<String, Value>>,
}

impl ToolContext {
    /// Create a new context bound to the given connection.
    pub fn new(connection: Arc<dyn Connection>) -> Self {
        Self {
            connection,
            state: RwLock::new(HashMap::new()),
        }
    }

    /// The backend-assigned conversation identifier.
    pub fn conversation_id(&self) -> &str {
        self.connection.conversation_id()
    }

    /// Whether the agent is currently idle (no turn in flight).
    pub fn is_idle(&self) -> bool {
        self.connection.is_idle()
    }

    /// Send an out-of-band trigger message into the agent.
    pub async fn send(&self, message: impl Into<String>) -> Result<()> {
        self.connection.send_trigger(message.into()).await
    }

    /// Read a value from the per-session state store.
    pub fn get_state(&self, key: &str) -> Option<Value> {
        self.state.read().get(key).cloned()
    }

    /// Write a value into the per-session state store.
    pub fn set_state(&self, key: impl Into<String>, value: Value) {
        self.state.write().insert(key.into(), value);
    }
}

// =============================================================================
// Runner
// =============================================================================

/// Registry that maps tool names to implementations and dispatches calls.
pub struct ToolRunner {
    tools: RwLock<HashMap<String, Arc<dyn Tool>>>,
    context: ArcSwapOption<ToolContext>,
}

impl Default for ToolRunner {
    fn default() -> Self {
        Self {
            tools: RwLock::new(HashMap::new()),
            context: ArcSwapOption::from(None),
        }
    }
}

impl ToolRunner {
    /// Create an empty tool runner with no registered tools.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool by name. Overwrites any existing tool with the same name.
    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.write().insert(name, tool);
    }

    /// Set the shared context passed to tools on each execution.
    pub fn set_context(&self, ctx: Arc<ToolContext>) {
        self.context.store(Some(ctx));
    }

    /// Remove the shared context (tools will receive `None`).
    pub fn clear_context(&self) {
        self.context.store(None);
    }

    /// List the names of all registered tools.
    pub fn names(&self) -> Vec<String> {
        self.tools.read().keys().cloned().collect()
    }

    /// Snapshot every registered tool as `Arc<dyn Tool>`. Cheap clone —
    /// the `Arc`s share their backing data.
    pub fn iter_tools(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.read().values().cloned().collect()
    }

    /// Execute a tool by name with the given JSON arguments.
    pub async fn execute(&self, name: &str, args: Value) -> Result<Value> {
        let tool = self
            .tools
            .read()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::ToolNotFound {
                name: name.to_string(),
            })?;
        let ctx = self.context.load_full();
        tool.execute(args, ctx).await
    }

    /// Execute a batch of tool calls and collect their results.
    pub async fn process_tool_calls(&self, calls: Vec<ToolCall>) -> Vec<ToolResult> {
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            match self.execute(&call.name, call.args.clone()).await {
                Ok(value) => results.push(ToolResult::ok(call.name, call.id, value)),
                Err(e) => results.push(ToolResult::err(call.name, call.id, e.to_string())),
            }
        }
        results
    }
}

// =============================================================================
// Builder helper for ad-hoc closure-based tools
// =============================================================================

// On wasm32 the future doesn't need `Send` (single-threaded executor)
// and the closure doesn't either. Keep the native signature unchanged.
#[cfg(not(target_arch = "wasm32"))]
type ToolFuture = futures_util::future::BoxFuture<'static, Result<Value>>;
#[cfg(target_arch = "wasm32")]
type ToolFuture = futures_util::future::LocalBoxFuture<'static, Result<Value>>;
#[cfg(not(target_arch = "wasm32"))]
type ClosureHandler = Arc<dyn Fn(Value, Option<Arc<ToolContext>>) -> ToolFuture + Send + Sync>;
#[cfg(target_arch = "wasm32")]
type ClosureHandler = Arc<dyn Fn(Value, Option<Arc<ToolContext>>) -> ToolFuture>;

/// A `Tool` whose `execute` is an `Arc<dyn Fn>` closure. Useful for binding
/// a Rust function into the SDK without creating a dedicated type.
pub struct ClosureTool {
    name: String,
    description: String,
    schema: Value,
    handler: ClosureHandler,
}

impl ClosureTool {
    /// Build a closure-based tool from a name, description, JSON schema, and async handler.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use localharness::ClosureTool;
    /// use serde_json::json;
    ///
    /// let tool = ClosureTool::new(
    ///     "greet",
    ///     "Say hello to someone",
    ///     json!({"type": "object", "properties": {"name": {"type": "string"}}}),
    ///     |args, _ctx| async move {
    ///         let name = args["name"].as_str().unwrap_or("world");
    ///         Ok(json!({"greeting": format!("Hello, {name}!")}))
    ///     },
    /// );
    /// ```
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        handler: F,
    ) -> Arc<Self>
    where
        F: Fn(Value, Option<Arc<ToolContext>>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value>> + Send + 'static,
    {
        Arc::new(Self {
            name: name.into(),
            description: description.into(),
            schema,
            handler: Arc::new(move |a, c| Box::pin(handler(a, c))),
        })
    }
    #[cfg(target_arch = "wasm32")]
    pub fn new<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        handler: F,
    ) -> Arc<Self>
    where
        F: Fn(Value, Option<Arc<ToolContext>>) -> Fut + 'static,
        Fut: std::future::Future<Output = Result<Value>> + 'static,
    {
        Arc::new(Self {
            name: name.into(),
            description: description.into(),
            schema,
            handler: Arc::new(move |a, c| Box::pin(handler(a, c))),
        })
    }

    /// Build a closure-based tool whose handler captures shared `state`.
    ///
    /// A `ClosureTool` runs its handler on *every* call, so any state the
    /// handler touches (a counter, an `Arc` resource, an observability sink)
    /// must be cloned *into the handler once* and then *again into each async
    /// body* — the awkward double-move below. `with_state` hoists that clone
    /// into the framework: you pass the shared `state` once, and your closure
    /// receives a fresh clone as its first argument on every call. The handler
    /// stays clean — no manual clone, no double-move.
    ///
    /// # Before (stateless [`new`] — the double-move workaround)
    ///
    /// ```rust,no_run
    /// use std::sync::Arc;
    /// use std::sync::atomic::{AtomicU64, Ordering};
    /// use localharness::ClosureTool;
    /// use serde_json::json;
    ///
    /// let calls = Arc::new(AtomicU64::new(0));
    /// let calls_in_tool = calls.clone();              // move #1: into the closure
    /// let tool = ClosureTool::new(
    ///     "tick",
    ///     "Increment a shared counter.",
    ///     json!({"type": "object", "properties": {}}),
    ///     move |_args, _ctx| {
    ///         let calls = calls_in_tool.clone();       // move #2: into the async body
    ///         async move {
    ///             calls.fetch_add(1, Ordering::SeqCst);
    ///             Ok(json!({"count": calls.load(Ordering::SeqCst)}))
    ///         }
    ///     },
    /// );
    /// ```
    ///
    /// # After (stateful `with_state` — the framework owns the clone)
    ///
    /// ```rust,no_run
    /// use std::sync::Arc;
    /// use std::sync::atomic::{AtomicU64, Ordering};
    /// use localharness::ClosureTool;
    /// use serde_json::json;
    ///
    /// let calls = Arc::new(AtomicU64::new(0));
    /// let tool = ClosureTool::with_state(
    ///     "tick",
    ///     "Increment a shared counter.",
    ///     json!({"type": "object", "properties": {}}),
    ///     calls,                                       // shared state, passed once
    ///     |calls, _args, _ctx| async move {            // a fresh clone per call
    ///         calls.fetch_add(1, Ordering::SeqCst);
    ///         Ok(json!({"count": calls.load(Ordering::SeqCst)}))
    ///     },
    /// );
    /// ```
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_state<S, F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        state: S,
        f: F,
    ) -> Arc<Self>
    where
        S: Clone + Send + Sync + 'static,
        F: Fn(S, Value, Option<Arc<ToolContext>>) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<Value>> + Send + 'static,
    {
        Self::new(name, description, schema, move |args, ctx| {
            f(state.clone(), args, ctx)
        })
    }

    /// Build a closure-based tool whose handler captures shared `state`.
    ///
    /// See the native overload for the before/after example. On wasm32 the
    /// closure, future, and state shed their `Send + Sync` bounds (the browser
    /// executor is single-threaded), matching [`ClosureTool::new`].
    #[cfg(target_arch = "wasm32")]
    pub fn with_state<S, F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        schema: Value,
        state: S,
        f: F,
    ) -> Arc<Self>
    where
        S: Clone + 'static,
        F: Fn(S, Value, Option<Arc<ToolContext>>) -> Fut + 'static,
        Fut: std::future::Future<Output = Result<Value>> + 'static,
    {
        Self::new(name, description, schema, move |args, ctx| {
            f(state.clone(), args, ctx)
        })
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Tool for ClosureTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn input_schema(&self) -> Value {
        self.schema.clone()
    }
    async fn execute(&self, args: Value, ctx: Option<Arc<ToolContext>>) -> Result<Value> {
        (self.handler)(args, ctx).await
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use serde_json::json;

    // `with_state` must thread the SAME shared state through every call, cloned
    // once by the framework. Build a counter tool over an `Arc<AtomicU64>`,
    // invoke it three times via the runner, and assert the mutation accumulated
    // across calls (i.e. the clone is a handle to the one counter, not a copy).
    #[tokio::test]
    async fn with_state_threads_shared_state_across_calls() {
        let counter = Arc::new(AtomicU64::new(0));

        let tool = ClosureTool::with_state(
            "tick",
            "Increment a shared counter and report the new value.",
            json!({ "type": "object", "properties": {} }),
            counter.clone(),
            |counter: Arc<AtomicU64>, _args, _ctx| async move {
                let prev = counter.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "count": prev + 1 }))
            },
        );

        let runner = ToolRunner::new();
        runner.register(tool);

        // Three independent invocations.
        let r1 = runner.execute("tick", json!({})).await.unwrap();
        let r2 = runner.execute("tick", json!({})).await.unwrap();
        let r3 = runner.execute("tick", json!({})).await.unwrap();

        // Each call saw the running total — the same state was threaded through.
        assert_eq!(r1["count"], json!(1));
        assert_eq!(r2["count"], json!(2));
        assert_eq!(r3["count"], json!(3));

        // The handle the test still holds reflects all three mutations: the
        // framework cloned a SHARED handle per call, not an independent copy.
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }
}
