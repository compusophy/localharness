//! Layer-1 `Agent` facade.
//!
//! Mirrors the Python `Agent` class: a single high-level handle that owns
//! the connection, hook runner, tool runner, trigger runner, and a
//! background dispatcher that routes custom-tool calls through the hooks /
//! policies / runner pipeline back to the harness.
//!
//! Lifecycle:
//!
//! ```rust,ignore
//! let cfg = GeminiAgentConfig::new(api_key).with_system_instructions("You are helpful.");
//! let agent = Agent::start_gemini(cfg).await?;
//! let response = agent.chat("hello").await?;
//! println!("{}", response.text().await?);
//! agent.shutdown().await?;
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use futures_util::stream::StreamExt;
#[cfg(not(target_arch = "wasm32"))]
use tokio::task::JoinHandle;
use tracing::{debug, warn};

use crate::backends::gemini::{GeminiBackendConfig, GeminiConnectionStrategy, GeminiRunners};
use crate::connections::{Connection, ConnectionStrategy};
use crate::content::Content;
use crate::conversation::{ChatResponse, Conversation};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::policy::{self, Policy};
use crate::tools::{Tool, ToolContext, ToolRunner};
use crate::triggers::{Trigger, TriggerRunner};
#[cfg(feature = "native")]
use crate::backends::mcp::McpBridge;
#[cfg(feature = "native")]
use crate::types::McpServerConfig;
use crate::types::{
    BuiltinTool, CapabilitiesConfig, GeminiConfig, StepStatus,
    SystemInstructions, ToolCall,
};

// =============================================================================
// Configuration
// =============================================================================

#[derive(Default)]
pub struct AgentConfig {
    pub system_instructions: Option<SystemInstructions>,
    pub capabilities: CapabilitiesConfig,
    pub tools: Vec<Arc<dyn Tool>>,
    pub policies: Vec<Policy>,
    pub triggers: Vec<Arc<dyn Trigger>>,
    pub workspaces: Vec<PathBuf>,
    #[cfg(feature = "native")]
    pub mcp_servers: Vec<McpServerConfig>,
    pub conversation_id: Option<String>,
    pub gemini: GeminiConfig,
    pub response_schema: Option<String>,
}

impl AgentConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        self.system_instructions = Some(instr.into());
        self
    }

    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.capabilities = cap;
        self
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_policies(mut self, policies: Vec<Policy>) -> Self {
        self.policies = policies;
        self
    }

    pub fn with_workspace(mut self, ws: impl Into<PathBuf>) -> Self {
        self.workspaces.push(ws.into());
        self
    }

    pub fn with_trigger(mut self, trigger: Arc<dyn Trigger>) -> Self {
        self.triggers.push(trigger);
        self
    }

    pub fn with_gemini(mut self, gemini: GeminiConfig) -> Self {
        self.gemini = gemini;
        self
    }

    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.gemini.api_key = Some(key.into());
        self
    }

    #[cfg(feature = "native")]
    pub fn with_mcp_server(mut self, server: McpServerConfig) -> Self {
        self.mcp_servers.push(server);
        self
    }
}

/// Configuration for the Rust-native Gemini backend.
///
/// Pairs the generic `AgentConfig` (hooks, tools, policies, triggers)
/// with `GeminiBackendConfig` (model, API key, thinking, etc.).
pub struct GeminiAgentConfig {
    pub agent: AgentConfig,
    pub gemini: GeminiBackendConfig,
}

impl GeminiAgentConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            agent: AgentConfig::default(),
            gemini: GeminiBackendConfig::new(api_key),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.gemini = self.gemini.with_model(model);
        self
    }

    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        let instr = instr.into();
        self.gemini = self.gemini.with_system_instructions(instr.clone());
        self.agent = self.agent.with_system_instructions(instr);
        self
    }

    pub fn with_thinking(mut self, level: crate::types::ThinkingLevel) -> Self {
        self.gemini = self.gemini.with_thinking(level);
        self
    }

    pub fn with_response_schema(mut self, schema: impl Into<String>) -> Self {
        let s = schema.into();
        self.gemini = self.gemini.with_response_schema(s.clone());
        self.agent.response_schema = Some(s);
        self
    }

    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.agent = self.agent.with_capabilities(cap);
        self
    }

    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.agent = self.agent.with_tool(tool);
        self
    }

    pub fn with_policies(mut self, policies: Vec<Policy>) -> Self {
        self.agent = self.agent.with_policies(policies);
        self
    }

    pub fn with_workspace(mut self, ws: impl Into<PathBuf>) -> Self {
        self.agent = self.agent.with_workspace(ws);
        self
    }

    pub fn with_trigger(mut self, trigger: Arc<dyn Trigger>) -> Self {
        self.agent = self.agent.with_trigger(trigger);
        self
    }

    #[cfg(feature = "native")]
    pub fn with_mcp_server(mut self, server: McpServerConfig) -> Self {
        self.agent = self.agent.with_mcp_server(server);
        self
    }

    pub fn resume(mut self, conversation_id: impl Into<String>) -> Self {
        let id = conversation_id.into();
        self.gemini.conversation_id = Some(id.clone());
        self.agent.conversation_id = Some(id);
        self
    }
}

// =============================================================================
// Agent
// =============================================================================

pub struct Agent {
    conversation: Conversation,
    connection: Arc<dyn Connection>,
    hook_runner: Arc<HookRunner>,
    tool_runner: Arc<ToolRunner>,
    trigger_runner: Option<Arc<TriggerRunner>>,
    #[cfg(feature = "native")]
    mcp_bridge: Option<Arc<McpBridge>>,
    session_ctx: SessionContext,
    #[cfg(not(target_arch = "wasm32"))]
    dispatcher: parking_lot::Mutex<Option<JoinHandle<()>>>,
    shutdown_flag: Arc<AtomicBool>,
}

impl Agent {
    /// Start an `Agent` backed by the Rust-native Gemini runtime.
    pub async fn start_gemini(mut config: GeminiAgentConfig) -> Result<Self> {
        config.agent.capabilities.validate()?;
        Self::wire_response_schema(&mut config.agent);
        // The Gemini strategy is bound to the agent's runners so that
        // function-call dispatch can run through hooks + policies +
        // tool_runner without round-tripping through `send_tool_results`.
        let mut gemini_config = config.gemini;
        // Make sure the backend's CapabilitiesConfig matches the agent's
        // (so register_builtins enables the right set).
        gemini_config.capabilities = config.agent.capabilities.clone();
        Self::start_with_factory(config.agent, |hooks, tools, ctx| {
            GeminiConnectionStrategy::new(gemini_config).with_runners(GeminiRunners {
                tool_runner: Some(tools),
                hook_runner: Some(hooks),
                session_ctx: Some(ctx),
            })
        })
        .await
    }

    /// Internal: shared bootstrap. The `factory` closure receives the
    /// fully-wired hook/tool runners and session context so backends
    /// that dispatch tools inline (Gemini) can inject them.
    async fn start_with_factory<S, F>(agent_config: AgentConfig, factory: F) -> Result<Self>
    where
        S: ConnectionStrategy + 'static,
        F: FnOnce(Arc<HookRunner>, Arc<ToolRunner>, SessionContext) -> S,
    {
        let hook_runner = Arc::new(HookRunner::new());
        let tool_runner = Arc::new(ToolRunner::new());

        for t in &agent_config.tools {
            tool_runner.register(t.clone());
        }

        // Build the effective policy list. Mirror Python's safety check:
        // write tools or MCP servers require either a policy list or a
        // user-installed pre-tool-call hook.
        let mut active_policies = agent_config.policies;
        if !agent_config.workspaces.is_empty() {
            let mut ws_policies = policy::workspace_only(agent_config.workspaces.clone());
            ws_policies.extend(active_policies);
            active_policies = ws_policies;
        }
        let effective_tools = agent_config.capabilities.effective_tools();
        let has_write = effective_tools
            .iter()
            .any(|t| !BuiltinTool::READ_ONLY.contains(t));
        if has_write && active_policies.is_empty() && !hook_runner.has_pre_tool_call_decide() {
            return Err(Error::config(
                "write tools are enabled but no safety policies are configured. \
                 Add policy::allow_all() to approve all calls, or \
                 [policy::deny_all(), policy::allow(\"tool_name\")] to scope.",
            ));
        }
        if !active_policies.is_empty() {
            hook_runner.register_pre_tool_call_decide(policy::enforce(active_policies));
        }

        // MCP servers: connect, register their tools BEFORE the
        // strategy spins up so the GeminiConnection captures them in
        // its FunctionDeclarations.
        #[cfg(feature = "native")]
        let mcp_bridge = if agent_config.mcp_servers.is_empty() {
            None
        } else {
            let mut bridge = McpBridge::new();
            for cfg in &agent_config.mcp_servers {
                bridge.connect(cfg).await?;
            }
            let registered = bridge.register_into(&tool_runner);
            if !registered.is_empty() {
                tracing::debug!(?registered, "registered MCP tools");
            }
            Some(Arc::new(bridge))
        };

        let session_ctx = SessionContext::new();
        let strategy = factory(hook_runner.clone(), tool_runner.clone(), session_ctx.clone());
        let connection = strategy.connect().await?;

        hook_runner.dispatch_session_start(&session_ctx).await;

        tool_runner.set_context(Arc::new(ToolContext::new(connection.clone())));

        let conversation = Conversation::new(connection.clone());

        let shutdown_flag = Arc::new(AtomicBool::new(false));
        #[cfg(not(target_arch = "wasm32"))]
        let dispatcher = spawn_tool_dispatcher(
            connection.clone(),
            tool_runner.clone(),
            hook_runner.clone(),
            session_ctx.clone(),
            shutdown_flag.clone(),
        );
        #[cfg(target_arch = "wasm32")]
        spawn_tool_dispatcher(
            connection.clone(),
            tool_runner.clone(),
            hook_runner.clone(),
            session_ctx.clone(),
            shutdown_flag.clone(),
        );

        let trigger_runner = if agent_config.triggers.is_empty() {
            None
        } else {
            let runner = Arc::new(TriggerRunner::new(
                agent_config.triggers,
                connection.clone(),
            ));
            runner.start()?;
            Some(runner)
        };

        Ok(Self {
            conversation,
            connection,
            hook_runner,
            tool_runner,
            trigger_runner,
            #[cfg(feature = "native")]
            mcp_bridge,
            session_ctx,
            #[cfg(not(target_arch = "wasm32"))]
            dispatcher: parking_lot::Mutex::new(Some(dispatcher)),
            shutdown_flag,
        })
    }

    fn wire_response_schema(config: &mut AgentConfig) {
        if let Some(schema) = config.response_schema.take() {
            config.capabilities.finish_tool_schema_json = Some(schema);
        }
    }

    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    pub fn conversation_id(&self) -> String {
        self.connection.conversation_id().to_string()
    }

    pub fn hooks(&self) -> &HookRunner {
        &self.hook_runner
    }

    pub fn tools(&self) -> &ToolRunner {
        &self.tool_runner
    }

    pub async fn chat(&self, content: impl Into<Content>) -> Result<ChatResponse> {
        self.conversation.chat(content).await
    }

    pub async fn shutdown(self) -> Result<()> {
        self.shutdown_flag.store(true, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let handle = self.dispatcher.lock().take();
            if let Some(handle) = handle {
                handle.abort();
                let _ = handle.await;
            }
        }
        if let Some(triggers) = self.trigger_runner.as_ref() {
            triggers.stop().await;
        }
        self.hook_runner.dispatch_session_end(&self.session_ctx).await;
        self.connection.shutdown().await?;
        #[cfg(feature = "native")]
        if let Some(bridge) = self.mcp_bridge.as_ref() {
            bridge.shutdown().await;
        }
        Ok(())
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.shutdown_flag.store(true, Ordering::Release);
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(handle) = self.dispatcher.lock().take() {
            handle.abort();
        }
    }
}

// =============================================================================
// Tool dispatcher
// =============================================================================

#[cfg(not(target_arch = "wasm32"))]
fn spawn_tool_dispatcher(
    connection: Arc<dyn Connection>,
    tool_runner: Arc<ToolRunner>,
    hook_runner: Arc<HookRunner>,
    session_ctx: SessionContext,
    shutdown: Arc<AtomicBool>,
) -> JoinHandle<()> {
    let registered: std::collections::HashSet<String> =
        tool_runner.names().into_iter().collect();
    tokio::spawn(async move {
        let mut stream = connection.subscribe_steps();
        while let Some(step) = stream.next().await {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            let step = match step {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "tool dispatcher stream error");
                    continue;
                }
            };
            if step.tool_calls.is_empty() {
                continue;
            }
            if matches!(step.status, StepStatus::Done) {
                continue;
            }

            let custom_calls: Vec<ToolCall> = step
                .tool_calls
                .into_iter()
                .filter(|tc| registered.contains(&tc.name))
                .collect();
            if custom_calls.is_empty() {
                continue;
            }

            let turn_ctx = session_ctx.child();
            let mut results = Vec::with_capacity(custom_calls.len());
            for call in custom_calls {
                let (decision, op_ctx) =
                    hook_runner.dispatch_pre_tool_call(&turn_ctx, &call).await;
                if !decision.allow {
                    let r = crate::types::ToolResult::err(
                        call.name.clone(),
                        call.id.clone(),
                        decision.message.clone(),
                    );
                    hook_runner.dispatch_post_tool_call(&op_ctx, &r).await;
                    results.push(r);
                    continue;
                }
                let r = match tool_runner.execute(&call.name, call.args.clone()).await {
                    Ok(v) => crate::types::ToolResult::ok(call.name.clone(), call.id.clone(), v),
                    Err(e) => crate::types::ToolResult::err(
                        call.name.clone(),
                        call.id.clone(),
                        e.to_string(),
                    ),
                };
                hook_runner.dispatch_post_tool_call(&op_ctx, &r).await;
                results.push(r);
            }

            if let Err(e) = connection.send_tool_results(results).await {
                warn!(error = %e, "failed to send tool results");
            }
        }
        debug!("tool dispatcher exiting");
    })
}

#[cfg(target_arch = "wasm32")]
fn spawn_tool_dispatcher(
    connection: Arc<dyn Connection>,
    tool_runner: Arc<ToolRunner>,
    hook_runner: Arc<HookRunner>,
    session_ctx: SessionContext,
    shutdown: Arc<AtomicBool>,
) {
    let registered: std::collections::HashSet<String> =
        tool_runner.names().into_iter().collect();
    crate::runtime::spawn(async move {
        let mut stream = connection.subscribe_steps();
        while let Some(step) = stream.next().await {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            let step = match step {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "tool dispatcher stream error");
                    continue;
                }
            };
            if step.tool_calls.is_empty() {
                continue;
            }
            if matches!(step.status, StepStatus::Done) {
                continue;
            }
            let custom_calls: Vec<ToolCall> = step
                .tool_calls
                .into_iter()
                .filter(|tc| registered.contains(&tc.name))
                .collect();
            if custom_calls.is_empty() {
                continue;
            }
            let turn_ctx = session_ctx.child();
            let mut results = Vec::with_capacity(custom_calls.len());
            for call in custom_calls {
                let (decision, op_ctx) =
                    hook_runner.dispatch_pre_tool_call(&turn_ctx, &call).await;
                if !decision.allow {
                    let r = crate::types::ToolResult::err(
                        call.name.clone(),
                        call.id.clone(),
                        decision.message.clone(),
                    );
                    hook_runner.dispatch_post_tool_call(&op_ctx, &r).await;
                    results.push(r);
                    continue;
                }
                let r = match tool_runner.execute(&call.name, call.args.clone()).await {
                    Ok(v) => crate::types::ToolResult::ok(call.name.clone(), call.id.clone(), v),
                    Err(e) => crate::types::ToolResult::err(
                        call.name.clone(),
                        call.id.clone(),
                        e.to_string(),
                    ),
                };
                hook_runner.dispatch_post_tool_call(&op_ctx, &r).await;
                results.push(r);
            }
            if let Err(e) = connection.send_tool_results(results).await {
                warn!(error = %e, "failed to send tool results");
            }
        }
        debug!("tool dispatcher exiting");
    });
}
