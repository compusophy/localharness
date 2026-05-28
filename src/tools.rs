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
