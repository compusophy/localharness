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

use crate::backends::gemini::{
    GeminiBackendConfig, GeminiConnection, GeminiConnectionStrategy, GeminiRunners,
};
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

/// Backend-agnostic agent configuration (tools, policies, triggers, workspaces).
#[derive(Default)]
pub struct AgentConfig {
    /// Optional system-level instructions for the model.
    pub system_instructions: Option<SystemInstructions>,
    /// Which built-in tools are enabled or disabled.
    pub capabilities: CapabilitiesConfig,
    /// Custom tools registered into the agent.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Safety policies governing tool execution.
    pub policies: Vec<Policy>,
    /// Background triggers that fire messages into the agent.
    pub triggers: Vec<Arc<dyn Trigger>>,
    /// Filesystem workspace roots for path-containment policies.
    pub workspaces: Vec<PathBuf>,
    /// MCP server configurations (native only).
    #[cfg(feature = "native")]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Resume an existing conversation by ID.
    pub conversation_id: Option<String>,
    /// Gemini-specific model and API key settings.
    pub gemini: GeminiConfig,
    /// JSON schema string for structured output via the `finish` tool.
    pub response_schema: Option<String>,
}

impl AgentConfig {
    /// Create an empty agent configuration with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the system instructions for the model.
    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        self.system_instructions = Some(instr.into());
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.capabilities = cap;
        self
    }

    /// Register a custom tool.
    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Set the safety policies for tool execution.
    pub fn with_policies(mut self, policies: Vec<Policy>) -> Self {
        self.policies = policies;
        self
    }

    /// Add a workspace root for path-containment enforcement.
    pub fn with_workspace(mut self, ws: impl Into<PathBuf>) -> Self {
        self.workspaces.push(ws.into());
        self
    }

    /// Register a background trigger.
    pub fn with_trigger(mut self, trigger: Arc<dyn Trigger>) -> Self {
        self.triggers.push(trigger);
        self
    }

    /// Set Gemini-specific configuration (model, API key).
    pub fn with_gemini(mut self, gemini: GeminiConfig) -> Self {
        self.gemini = gemini;
        self
    }

    /// Set the Gemini API key.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.gemini.api_key = Some(key.into());
        self
    }

    /// Add an MCP server to connect at startup (native only).
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
    /// Backend-agnostic settings (tools, policies, triggers).
    pub agent: AgentConfig,
    /// Gemini-specific settings (model, API key, thinking).
    pub gemini: GeminiBackendConfig,
    /// Opaque history bytes from a previous session, as returned by
    /// `Agent::history_bytes()`. Applied to the new connection
    /// immediately after `connect()`. Empty / missing means "start fresh."
    pub initial_history: Option<Vec<u8>>,
}

impl GeminiAgentConfig {
    /// Create a new Gemini agent configuration with the given API key.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use localharness::GeminiAgentConfig;
    ///
    /// let cfg = GeminiAgentConfig::new("my-api-key")
    ///     .with_system_instructions("You are helpful.");
    /// ```
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            agent: AgentConfig::default(),
            gemini: GeminiBackendConfig::new(api_key),
            initial_history: None,
        }
    }

    /// Seed the new connection with previously-saved history bytes
    /// (obtained from `Agent::history_bytes()`). If the bytes fail to
    /// parse at start time, `Agent::start_gemini` returns an error.
    pub fn with_history_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.initial_history = Some(bytes);
        self
    }

    /// Override the default Gemini model ID.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.gemini = self.gemini.with_model(model);
        self
    }

    /// Set the system instructions for the model.
    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        let instr = instr.into();
        self.gemini = self.gemini.with_system_instructions(instr.clone());
        self.agent = self.agent.with_system_instructions(instr);
        self
    }

    /// Enable extended thinking at the given level.
    pub fn with_thinking(mut self, level: crate::types::ThinkingLevel) -> Self {
        self.gemini = self.gemini.with_thinking(level);
        self
    }

    /// Set a JSON schema for structured output via the `finish` tool.
    pub fn with_response_schema(mut self, schema: impl Into<String>) -> Self {
        let s = schema.into();
        self.gemini = self.gemini.with_response_schema(s.clone());
        self.agent.response_schema = Some(s);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.agent = self.agent.with_capabilities(cap);
        self
    }

    /// Route requests through an alternate base URL (e.g. the
    /// localharness credit proxy) instead of Google's endpoint. In
    /// credits mode the api key carries the proxy auth token.
    pub fn with_base_url(mut self, url: url::Url) -> Self {
        self.gemini = self.gemini.with_base_url(url);
        self
    }

    /// Plug in a custom [`Filesystem`] impl for the 6 fs built-ins.
    /// Without this, native builds use `NativeFilesystem`; wasm builds
    /// have no filesystem and the fs builtins skip registration.
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.gemini = self.gemini.with_filesystem(fs);
        self
    }

    /// Register a custom tool.
    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.agent = self.agent.with_tool(tool);
        self
    }

    /// Set the safety policies for tool execution.
    pub fn with_policies(mut self, policies: Vec<Policy>) -> Self {
        self.agent = self.agent.with_policies(policies);
        self
    }

    /// Add a workspace root for path-containment enforcement.
    pub fn with_workspace(mut self, ws: impl Into<PathBuf>) -> Self {
        self.agent = self.agent.with_workspace(ws);
        self
    }

    /// Register a background trigger.
    pub fn with_trigger(mut self, trigger: Arc<dyn Trigger>) -> Self {
        self.agent = self.agent.with_trigger(trigger);
        self
    }

    /// Add an MCP server to connect at startup (native only).
    #[cfg(feature = "native")]
    pub fn with_mcp_server(mut self, server: McpServerConfig) -> Self {
        self.agent = self.agent.with_mcp_server(server);
        self
    }

    /// Resume an existing conversation by its ID.
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

/// High-level agent handle: connect, chat, shutdown.
///
/// Owns the connection, runners, and background dispatcher. Drop aborts
/// background tasks; call [`Agent::shutdown`] for a clean teardown.
///
/// # Examples
///
/// ```rust,no_run
/// use localharness::{Agent, GeminiAgentConfig};
///
/// # async fn run() -> localharness::Result<()> {
/// let agent = Agent::start_gemini(
///     GeminiAgentConfig::new("key")
///         .with_system_instructions("Be concise."),
/// ).await?;
/// let resp = agent.chat("Hello").await?;
/// println!("{}", resp.text().await?);
/// agent.shutdown().await?;
/// # Ok(())
/// # }
/// ```
pub struct Agent {
    conversation: Conversation,
    connection: Arc<dyn Connection>,
    /// Typed handle to the Gemini connection when `start_gemini` was
    /// used. Lets backend-specific APIs like `history_bytes()` work
    /// without forcing the Connection trait to carry every backend's
    /// per-protocol surface.
    gemini_connection: Option<Arc<GeminiConnection>>,
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
        let initial_history = config.initial_history.take();
        // Capture the typed Arc<GeminiConnection> through a shared slot
        // the strategy fills during connect(). Lets us call
        // backend-specific methods (history snapshot, etc.) without
        // bloating the Connection trait.
        let capture: Arc<parking_lot::Mutex<Option<Arc<GeminiConnection>>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let capture_for_factory = capture.clone();
        let mut agent = Self::start_with_factory(config.agent, move |hooks, tools, ctx| {
            GeminiConnectionStrategy::new(gemini_config)
                .with_runners(GeminiRunners {
                    tool_runner: Some(tools),
                    hook_runner: Some(hooks),
                    session_ctx: Some(ctx),
                })
                .with_typed_capture(capture_for_factory)
        })
        .await?;
        agent.gemini_connection = capture.lock().take();
        if let (Some(bytes), Some(gc)) = (initial_history, agent.gemini_connection.as_ref()) {
            gc.set_history_bytes(&bytes)?;
        }
        Ok(agent)
    }

    /// Token usage accumulated across every turn in this agent's
    /// conversation. Surfaced in the browser app's Usage tab.
    pub fn cumulative_usage(&self) -> crate::types::UsageMetadata {
        self.conversation.cumulative_usage()
    }

    /// Cooperatively cancel the in-flight turn (e.g. a UI stop button).
    /// The backend stops at its next safe boundary — between streamed
    /// chunks or before the next model call / tool dispatch — so no more
    /// tokens are spent and no further tools run. No-op when idle.
    pub fn cancel_turn(&self) {
        self.conversation.cancel_turn();
    }

    /// Opaque snapshot of the current Gemini conversation history.
    /// Returns `None` for non-Gemini backends. Round-trips through
    /// [`GeminiAgentConfig::with_history_bytes`] for session resume.
    pub fn history_bytes(&self) -> Result<Option<Vec<u8>>> {
        match self.gemini_connection.as_ref() {
            Some(gc) => gc.history_bytes().map(Some),
            None => Ok(None),
        }
    }

    /// Manually trigger context compaction. Summarises older history
    /// entries and replaces them with a single synthetic turn, freeing
    /// context-window budget. Returns `true` if compaction changed the
    /// history, `false` if it was too short or not applicable.
    /// Returns `false` for non-Gemini backends (no-op).
    pub async fn compact(&self) -> bool {
        match self.gemini_connection.as_ref() {
            Some(gc) => gc.compact().await,
            None => false,
        }
    }

    /// Human-readable transcript of the current session. Drops
    /// tool-call activity — see [`TranscriptEntry`] for the shape.
    /// Returns an empty vec for non-Gemini backends.
    ///
    /// [`TranscriptEntry`]: crate::types::TranscriptEntry
    pub fn transcript(&self) -> Vec<crate::types::TranscriptEntry> {
        self.gemini_connection
            .as_ref()
            .map(|gc| gc.transcript())
            .unwrap_or_default()
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
        // Roadmap Phase 0b — close the custom-tool safety bypass. The old guard
        // only inspected `effective_tools()` (the BuiltinTool set), so a config
        // with custom `ClosureTool`s (e.g. the autonomous loop's `qa_*` tools)
        // and no policy passed with ZERO enforcement — the safety story would be
        // a prompt-level honor system. Now ANY custom tool also requires an
        // explicit policy or a pre-tool-call hook, enforced at the ToolRunner.
        let has_custom_tools = !agent_config.tools.is_empty();
        if requires_safety_policy(&agent_config.capabilities, has_custom_tools)
            && active_policies.is_empty()
            && !hook_runner.has_pre_tool_call_decide()
        {
            return Err(Error::config(
                "write or custom tools are enabled but no safety policies are \
                 configured. Add policy::allow_all() to approve all calls, or \
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
            gemini_connection: None,
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

    /// The underlying conversation session.
    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    /// The backend-assigned conversation identifier.
    pub fn conversation_id(&self) -> String {
        self.connection.conversation_id().to_string()
    }

    /// The hook runner; use to register additional hooks after start.
    pub fn hooks(&self) -> &HookRunner {
        &self.hook_runner
    }

    /// The tool runner; use to register additional tools after start.
    pub fn tools(&self) -> &ToolRunner {
        &self.tool_runner
    }

    /// Send a prompt and return a streaming [`ChatResponse`].
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// # use localharness::Agent;
    /// # async fn run(agent: &Agent) -> localharness::Result<()> {
    /// let response = agent.chat("What is Rust?").await?;
    /// println!("{}", response.text().await?);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn chat(&self, content: impl Into<Content>) -> Result<ChatResponse> {
        self.conversation.chat(content).await
    }

    /// Cleanly shut down the agent, aborting triggers and dispatchers.
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

/// Whether the agent's safety guard must require an explicit policy or a
/// pre-tool-call hook before start: true if any WRITE builtin OR any custom
/// tool is enabled. Custom tools (`ClosureTool`s) bypassed the old
/// `effective_tools()`-only check — closing that is roadmap Phase 0b, so the
/// autonomous loop can't register `qa_*` tools and run them with zero policy.
fn requires_safety_policy(capabilities: &CapabilitiesConfig, has_custom_tools: bool) -> bool {
    has_custom_tools
        || capabilities
            .effective_tools()
            .iter()
            .any(|t| !BuiltinTool::READ_ONLY.contains(t))
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

#[cfg(test)]
mod safety_guard_tests {
    use super::*;

    fn caps(enabled: Vec<BuiltinTool>) -> CapabilitiesConfig {
        CapabilitiesConfig {
            enabled_tools: Some(enabled),
            ..Default::default()
        }
    }

    #[test]
    fn custom_tools_require_a_safety_policy() {
        // No builtins, no custom tools → no policy required.
        assert!(!requires_safety_policy(&caps(vec![]), false));
        // A custom tool ALONE now requires a policy (the closed Phase-0b bypass).
        assert!(requires_safety_policy(&caps(vec![]), true));
        // A write builtin requires a policy (unchanged behavior).
        assert!(requires_safety_policy(&caps(vec![BuiltinTool::CreateFile]), false));
        // Read-only builtins alone do not.
        assert!(!requires_safety_policy(
            &caps(BuiltinTool::READ_ONLY.to_vec()),
            false
        ));
    }
}
