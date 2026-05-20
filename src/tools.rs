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
use crate::types::{ToolCall, ToolResult};

// =============================================================================
// Tool trait
// =============================================================================

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    async fn execute(&self, args: Value, ctx: Option<Arc<ToolContext>>) -> Result<Value>;
}

// =============================================================================
// Tool context
// =============================================================================

pub struct ToolContext {
    connection: Arc<dyn Connection>,
    state: RwLock<HashMap<String, Value>>,
}

impl ToolContext {
    pub fn new(connection: Arc<dyn Connection>) -> Self {
        Self {
            connection,
            state: RwLock::new(HashMap::new()),
        }
    }

    pub fn conversation_id(&self) -> &str {
        self.connection.conversation_id()
    }

    pub fn is_idle(&self) -> bool {
        self.connection.is_idle()
    }

    pub async fn send(&self, message: impl Into<String>) -> Result<()> {
        self.connection.send_trigger(message.into()).await
    }

    pub fn get_state(&self, key: &str) -> Option<Value> {
        self.state.read().get(key).cloned()
    }

    pub fn set_state(&self, key: impl Into<String>, value: Value) {
        self.state.write().insert(key.into(), value);
    }
}

// =============================================================================
// Runner
// =============================================================================

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
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.write().insert(name, tool);
    }

    pub fn set_context(&self, ctx: Arc<ToolContext>) {
        self.context.store(Some(ctx));
    }

    pub fn clear_context(&self) {
        self.context.store(None);
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.read().keys().cloned().collect()
    }

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

type ToolFuture = futures_util::future::BoxFuture<'static, Result<Value>>;
type ClosureHandler = Arc<dyn Fn(Value, Option<Arc<ToolContext>>) -> ToolFuture + Send + Sync>;

/// A `Tool` whose `execute` is an `Arc<dyn Fn>` closure. Useful for binding
/// a Rust function into the SDK without creating a dedicated type.
pub struct ClosureTool {
    name: String,
    description: String,
    schema: Value,
    handler: ClosureHandler,
}

impl ClosureTool {
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
}

#[async_trait]
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
