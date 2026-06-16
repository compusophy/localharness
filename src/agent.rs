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
use crate::backends::mock::{MockConnectionStrategy, MockRunners};
#[cfg(feature = "anthropic")]
use crate::backends::anthropic::{
    AnthropicBackendConfig, AnthropicConnection, AnthropicConnectionStrategy, AnthropicRunners,
};
#[cfg(feature = "openai")]
use crate::backends::openai::{
    OpenAiBackendConfig, OpenAiConnection, OpenAiConnectionStrategy, OpenAiRunners,
};
#[cfg(feature = "local")]
use crate::backends::local::connection::{
    LocalBackendConfig, LocalConnection, LocalConnectionStrategy, LocalRunners,
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
    BuiltinTool, CapabilitiesConfig, StepStatus, SystemInstructions, ToolCall,
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
    /// JSON schema string for structured output via the `finish` tool.
    pub response_schema: Option<String>,
    /// Custom pre-tool-call decide hooks, registered alongside the policy
    /// enforcer. First deny wins — use for cross-cutting guards a static
    /// `Policy` can't express (e.g. duplicate-action suppression).
    pub pre_tool_hooks: Vec<Arc<dyn crate::hooks::PreToolCallDecideHook>>,
    /// Custom post-tool-call inspect hooks, run after each call's result is
    /// known. Inspect-only (cannot block) — use to observe outcomes or undo a
    /// pre-hook's optimistic bookkeeping on failure (e.g. the dedup cleanup).
    pub post_tool_hooks: Vec<Arc<dyn crate::hooks::PostToolCallHook>>,
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

    /// Register a custom pre-tool-call decide hook (runs alongside the
    /// policy enforcer; first deny wins). For cross-cutting guards a static
    /// `Policy` can't express — e.g. duplicate-action suppression.
    pub fn with_pre_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PreToolCallDecideHook>,
    ) -> Self {
        self.pre_tool_hooks.push(hook);
        self
    }

    /// Register a custom post-tool-call inspect hook (runs after each call's
    /// result is known; inspect-only). Pairs with a pre-tool hook to undo
    /// optimistic bookkeeping on failure — e.g. reverting a dedup hash insert.
    pub fn with_post_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PostToolCallHook>,
    ) -> Self {
        self.post_tool_hooks.push(hook);
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
    /// use localharness::{GeminiAgentConfig, policy};
    ///
    /// // Builders chain; each returns `Self`. A deny-by-default policy turns the
    /// // agent into an allowlist (only the named tools run), and `with_workspace`
    /// // sandboxes the filesystem builtins to that directory.
    /// let cfg = GeminiAgentConfig::new("my-api-key")
    ///     .with_model("gemini-3.5-flash")
    ///     .with_system_instructions("You are a careful coding assistant.")
    ///     .with_workspace("/path/to/project")
    ///     .with_policies(vec![
    ///         policy::deny_all(),
    ///         policy::Policy::allow("view_file"),
    ///     ]);
    /// # let _ = cfg;
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

    /// Cap output tokens (`maxOutputTokens`) per model call. Set this high
    /// enough that a hard task can both reason and emit a final answer in one
    /// call; an unset/low cap lets dynamic thinking starve the text on a 3.x
    /// model, ending the turn `MAX_TOKENS` with no output.
    pub fn with_max_output_tokens(mut self, max: u32) -> Self {
        self.gemini = self.gemini.with_max_output_tokens(max);
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

    /// Mint a fresh auth credential for EVERY request instead of reusing
    /// the static api key (which becomes a fallback). Long-lived sessions
    /// against the credit proxy need this — its signed tokens expire after
    /// 5 minutes, so a session-baked token goes stale mid-conversation.
    pub fn with_auth_provider(mut self, provider: crate::backends::KeyProvider) -> Self {
        self.gemini.api_key_provider = Some(crate::backends::AuthTokenProvider(provider));
        self
    }

    /// Attach an extra header to every outbound request (e.g. an `X-PAYMENT`
    /// x402 authorization carried alongside the proxy auth token). No-op when
    /// unset.
    pub fn with_extra_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.gemini = self.gemini.with_extra_header(name, value);
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

    /// Register a custom pre-tool-call decide hook (see
    /// [`AgentConfig::with_pre_tool_hook`]).
    pub fn with_pre_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PreToolCallDecideHook>,
    ) -> Self {
        self.agent = self.agent.with_pre_tool_hook(hook);
        self
    }

    /// Register a custom post-tool-call inspect hook (see
    /// [`AgentConfig::with_post_tool_hook`]).
    pub fn with_post_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PostToolCallHook>,
    ) -> Self {
        self.agent = self.agent.with_post_tool_hook(hook);
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
// Mock agent config (always available — offline testing)
// =============================================================================

/// Configuration for the deterministic, offline mock backend.
///
/// Pairs the generic [`AgentConfig`] (hooks, tools, policies, triggers) with a
/// scripted [`MockConnectionStrategy`] so an [`Agent`] can be driven entirely
/// offline — no network, no API key, no LLM. The parallel of
/// [`GeminiAgentConfig`], always available (no feature flag). Build the
/// strategy with [`MockConnection::builder`].
///
/// [`MockConnectionStrategy`]: crate::backends::mock::MockConnectionStrategy
/// [`MockConnection::builder`]: crate::backends::mock::MockConnection::builder
///
/// # Examples
///
/// ```rust,no_run
/// use localharness::{Agent, policy};
/// use localharness::backends::mock::{MockAgentConfig, MockConnection};
///
/// # async fn run() -> localharness::Result<()> {
/// let backend = MockConnection::builder()
///     .turn(|t| t.text("the scripted answer"))
///     .build();
/// let agent = Agent::start_mock(MockAgentConfig::new(backend)).await?;
/// assert_eq!(agent.chat("anything").await?.text().await?, "the scripted answer");
/// agent.shutdown().await?;
/// # Ok(())
/// # }
/// ```
pub struct MockAgentConfig {
    /// Backend-agnostic settings (tools, policies, triggers).
    pub agent: AgentConfig,
    /// The scripted mock backend strategy.
    pub mock: MockConnectionStrategy,
}

impl MockAgentConfig {
    /// Create a mock agent configuration from a scripted backend strategy
    /// (built via [`MockConnection::builder`]).
    ///
    /// [`MockConnection::builder`]: crate::backends::mock::MockConnection::builder
    pub fn new(mock: MockConnectionStrategy) -> Self {
        Self {
            agent: AgentConfig::default(),
            mock,
        }
    }

    /// Set the system instructions (recorded on the agent config; the mock
    /// ignores them — its turns are scripted, not generated).
    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        self.agent = self.agent.with_system_instructions(instr);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.agent = self.agent.with_capabilities(cap);
        self
    }

    /// Register a custom tool. Scripted `tool_call`s dispatch through it.
    pub fn with_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.agent = self.agent.with_tool(tool);
        self
    }

    /// Set the safety policies for tool execution. Scripted tool calls run
    /// through these exactly as the live backends do.
    pub fn with_policies(mut self, policies: Vec<Policy>) -> Self {
        self.agent = self.agent.with_policies(policies);
        self
    }

    /// Register a custom pre-tool-call decide hook (see
    /// [`AgentConfig::with_pre_tool_hook`]).
    pub fn with_pre_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PreToolCallDecideHook>,
    ) -> Self {
        self.agent = self.agent.with_pre_tool_hook(hook);
        self
    }

    /// Register a custom post-tool-call inspect hook (see
    /// [`AgentConfig::with_post_tool_hook`]).
    pub fn with_post_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PostToolCallHook>,
    ) -> Self {
        self.agent = self.agent.with_post_tool_hook(hook);
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
}

// =============================================================================
// Anthropic agent config (feature = "anthropic")
// =============================================================================

/// Configuration for the Rust-native Anthropic (Claude Messages) backend.
///
/// Pairs the generic `AgentConfig` (hooks, tools, policies, triggers) with
/// `AnthropicBackendConfig` (model, API key, thinking, max_tokens). The
/// parallel of [`GeminiAgentConfig`]; additive — `start_gemini` and the
/// neutral `AgentConfig` are untouched.
#[cfg(feature = "anthropic")]
pub struct AnthropicAgentConfig {
    /// Backend-agnostic settings (tools, policies, triggers).
    pub agent: AgentConfig,
    /// Anthropic-specific settings (model, API key, thinking, max_tokens).
    pub anthropic: AnthropicBackendConfig,
    /// Opaque history bytes from a previous session
    /// (`Agent::history_bytes()`), applied immediately after `connect()`.
    pub initial_history: Option<Vec<u8>>,
}

#[cfg(feature = "anthropic")]
impl AnthropicAgentConfig {
    /// Create a new Anthropic agent configuration with the given API key
    /// (BYOK — talks directly to `api.anthropic.com`).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            agent: AgentConfig::default(),
            anthropic: AnthropicBackendConfig::new(api_key),
            initial_history: None,
        }
    }

    /// Seed the new connection with previously-saved history bytes.
    pub fn with_history_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.initial_history = Some(bytes);
        self
    }

    /// Override the Anthropic model ID (e.g. sonnet / opus).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.anthropic = self.anthropic.with_model(model);
        self
    }

    /// Set the system instructions for the model.
    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        let instr = instr.into();
        self.anthropic = self.anthropic.with_system_instructions(instr.clone());
        self.agent = self.agent.with_system_instructions(instr);
        self
    }

    /// Enable extended thinking at the given level.
    pub fn with_thinking(mut self, level: crate::types::ThinkingLevel) -> Self {
        self.anthropic = self.anthropic.with_thinking(level);
        self
    }

    /// Set the sampling temperature.
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.anthropic = self.anthropic.with_temperature(t);
        self
    }

    /// Set `max_tokens` for the response (Anthropic requires it; defaults
    /// to 8192 otherwise).
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.anthropic = self.anthropic.with_max_tokens(n);
        self
    }

    /// Route requests through an alternate base URL (future credit proxy).
    pub fn with_base_url(mut self, url: url::Url) -> Self {
        self.anthropic = self.anthropic.with_base_url(url);
        self
    }

    /// Mint a fresh auth credential for EVERY request instead of reusing
    /// the static api key (which becomes a fallback) — see
    /// [`GeminiAgentConfig::with_auth_provider`].
    pub fn with_auth_provider(mut self, provider: crate::backends::KeyProvider) -> Self {
        self.anthropic.api_key_provider = Some(crate::backends::AuthTokenProvider(provider));
        self
    }

    /// Attach an extra header to every outbound request (e.g. an `X-PAYMENT`
    /// x402 authorization carried alongside the proxy auth token). No-op when
    /// unset.
    pub fn with_extra_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.anthropic = self.anthropic.with_extra_header(name, value);
        self
    }

    /// Plug in a custom [`Filesystem`] impl for the fs built-ins.
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.anthropic = self.anthropic.with_filesystem(fs);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.agent = self.agent.with_capabilities(cap);
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

    /// Register a custom pre-tool-call decide hook (see
    /// [`AgentConfig::with_pre_tool_hook`]).
    pub fn with_pre_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PreToolCallDecideHook>,
    ) -> Self {
        self.agent = self.agent.with_pre_tool_hook(hook);
        self
    }

    /// Register a custom post-tool-call inspect hook (see
    /// [`AgentConfig::with_post_tool_hook`]).
    pub fn with_post_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PostToolCallHook>,
    ) -> Self {
        self.agent = self.agent.with_post_tool_hook(hook);
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
        self.anthropic.conversation_id = Some(id.clone());
        self.agent.conversation_id = Some(id);
        self
    }
}

// =============================================================================
// OpenAI agent config (feature = "openai")
// =============================================================================

/// Configuration for the Rust-native OpenAI (Chat Completions) backend.
///
/// Pairs the generic `AgentConfig` (hooks, tools, policies, triggers) with
/// `OpenAiBackendConfig` (model, API key, temperature, max_tokens). The
/// parallel of [`GeminiAgentConfig`] / [`AnthropicAgentConfig`]; additive —
/// `start_gemini` and the neutral `AgentConfig` are untouched.
#[cfg(feature = "openai")]
pub struct OpenAiAgentConfig {
    /// Backend-agnostic settings (tools, policies, triggers).
    pub agent: AgentConfig,
    /// OpenAI-specific settings (model, API key, temperature, max_tokens).
    pub openai: OpenAiBackendConfig,
    /// Opaque history bytes from a previous session
    /// (`Agent::history_bytes()`), applied immediately after `connect()`.
    pub initial_history: Option<Vec<u8>>,
}

#[cfg(feature = "openai")]
impl OpenAiAgentConfig {
    /// Create a new OpenAI agent configuration with the given API key
    /// (BYOK — talks directly to `api.openai.com`).
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            agent: AgentConfig::default(),
            openai: OpenAiBackendConfig::new(api_key),
            initial_history: None,
        }
    }

    /// Seed the new connection with previously-saved history bytes.
    pub fn with_history_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.initial_history = Some(bytes);
        self
    }

    /// Override the OpenAI model ID (e.g. `gpt-5-mini` / `gpt-5-pro` / any
    /// other string — model ids are NOT validated).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.openai = self.openai.with_model(model);
        self
    }

    /// Set the system instructions for the model.
    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        let instr = instr.into();
        self.openai = self.openai.with_system_instructions(instr.clone());
        self.agent = self.agent.with_system_instructions(instr);
        self
    }

    /// Set the sampling temperature.
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.openai = self.openai.with_temperature(t);
        self
    }

    /// Set `max_completion_tokens` for the response.
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.openai = self.openai.with_max_tokens(n);
        self
    }

    /// Route requests through an alternate base URL (the credit proxy, which
    /// already forwards `/v1/chat/completions`).
    pub fn with_base_url(mut self, url: url::Url) -> Self {
        self.openai = self.openai.with_base_url(url);
        self
    }

    /// Mint a fresh auth credential for EVERY request instead of reusing the
    /// static api key (which becomes a fallback) — see
    /// [`GeminiAgentConfig::with_auth_provider`].
    pub fn with_auth_provider(mut self, provider: crate::backends::KeyProvider) -> Self {
        self.openai.api_key_provider = Some(crate::backends::AuthTokenProvider(provider));
        self
    }

    /// Plug in a custom [`Filesystem`] impl for the fs built-ins.
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.openai = self.openai.with_filesystem(fs);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.agent = self.agent.with_capabilities(cap);
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

    /// Register a custom pre-tool-call decide hook (see
    /// [`AgentConfig::with_pre_tool_hook`]).
    pub fn with_pre_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PreToolCallDecideHook>,
    ) -> Self {
        self.agent = self.agent.with_pre_tool_hook(hook);
        self
    }

    /// Register a custom post-tool-call inspect hook (see
    /// [`AgentConfig::with_post_tool_hook`]).
    pub fn with_post_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PostToolCallHook>,
    ) -> Self {
        self.agent = self.agent.with_post_tool_hook(hook);
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
        self.openai.conversation_id = Some(id.clone());
        self.agent.conversation_id = Some(id);
        self
    }
}

// =============================================================================
// Local agent config (feature = "local")
// =============================================================================

/// Configuration for the in-browser local (Gemma 3 270M / Burn-wgpu) backend.
///
/// Pairs the generic `AgentConfig` with [`LocalBackendConfig`]. The parallel of
/// [`GeminiAgentConfig`] / [`AnthropicAgentConfig`]; additive and feature-gated.
/// There is no API key — the model runs fully on-device; weights are read from
/// the supplied [`Filesystem`] (OPFS in the browser).
///
/// [`Filesystem`]: crate::filesystem::Filesystem
#[cfg(feature = "local")]
pub struct LocalAgentConfig {
    /// Backend-agnostic settings (tools, policies, triggers).
    pub agent: AgentConfig,
    /// Local-backend settings (model label, OPFS paths, filesystem).
    pub local: LocalBackendConfig,
    /// Opaque history bytes from a previous session, applied after `connect()`.
    pub initial_history: Option<Vec<u8>>,
}

#[cfg(feature = "local")]
impl LocalAgentConfig {
    /// Create a new local agent configuration for the given model label.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            agent: AgentConfig::default(),
            local: LocalBackendConfig::new(model),
            initial_history: None,
        }
    }

    /// Seed the new connection with previously-saved history bytes.
    pub fn with_history_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.initial_history = Some(bytes);
        self
    }

    /// Set the model id label.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.local = self.local.with_model(model);
        self
    }

    /// Set the system instructions for the model.
    pub fn with_system_instructions(mut self, instr: impl Into<SystemInstructions>) -> Self {
        let instr = instr.into();
        self.local = self.local.with_system_instructions(instr.clone());
        self.agent = self.agent.with_system_instructions(instr);
        self
    }

    /// Plug in the [`Filesystem`] the weights/tokenizer are read from.
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.local = self.local.with_filesystem(fs);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, cap: CapabilitiesConfig) -> Self {
        self.agent = self.agent.with_capabilities(cap);
        self.local = self.local.with_capabilities(self.agent.capabilities.clone());
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

    /// Register a custom pre-tool-call decide hook (see
    /// [`AgentConfig::with_pre_tool_hook`]).
    pub fn with_pre_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PreToolCallDecideHook>,
    ) -> Self {
        self.agent = self.agent.with_pre_tool_hook(hook);
        self
    }

    /// Register a custom post-tool-call inspect hook (see
    /// [`AgentConfig::with_post_tool_hook`]).
    pub fn with_post_tool_hook(
        mut self,
        hook: Arc<dyn crate::hooks::PostToolCallHook>,
    ) -> Self {
        self.agent = self.agent.with_post_tool_hook(hook);
        self
    }

    /// Resume an existing conversation by its ID.
    pub fn resume(mut self, conversation_id: impl Into<String>) -> Self {
        let id = conversation_id.into();
        self.local.conversation_id = Some(id.clone());
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
    /// Typed handle to the Anthropic connection when `start_anthropic` was
    /// used. Parallels `gemini_connection` so `history_bytes()` / `compact()`
    /// / `transcript()` work for either backend. Additive (feature-gated).
    #[cfg(feature = "anthropic")]
    anthropic_connection: Option<Arc<AnthropicConnection>>,
    /// Typed handle to the OpenAI connection when `start_openai` was used.
    /// Parallels `anthropic_connection`. Additive (feature-gated).
    #[cfg(feature = "openai")]
    openai_connection: Option<Arc<OpenAiConnection>>,
    /// Typed handle to the local (in-browser Gemma) connection when
    /// `start_local` was used. Parallels `anthropic_connection`. Additive
    /// (feature-gated).
    #[cfg(feature = "local")]
    local_connection: Option<Arc<LocalConnection>>,
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

    /// Start an `Agent` backed by the deterministic, offline [mock backend].
    ///
    /// The agent runs entirely offline — no network, no API key, no LLM. The
    /// model's turns are scripted via [`MockConnection::builder`]; scripted
    /// tool calls dispatch inline through the SAME hooks + policies + tool
    /// runner the live backends use, so this exercises real agent logic (the
    /// tool loop) against a deterministic model. Always available (no feature
    /// flag) — built for unit-testing agents.
    ///
    /// [mock backend]: crate::backends::mock
    /// [`MockConnection::builder`]: crate::backends::mock::MockConnection::builder
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use localharness::{Agent, policy};
    /// use localharness::backends::mock::{MockAgentConfig, MockConnection};
    ///
    /// # async fn run() -> localharness::Result<()> {
    /// let backend = MockConnection::builder()
    ///     .turn(|t| t.text("hello from the mock"))
    ///     .build();
    /// let agent = Agent::start_mock(MockAgentConfig::new(backend)).await?;
    /// assert_eq!(agent.chat("hi").await?.text().await?, "hello from the mock");
    /// agent.shutdown().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn start_mock(mut config: MockAgentConfig) -> Result<Self> {
        config.agent.capabilities.validate()?;
        Self::wire_response_schema(&mut config.agent);
        let mock = config.mock;
        Self::start_with_factory(config.agent, move |hooks, tools, ctx| {
            mock.with_runners(MockRunners {
                tool_runner: Some(tools),
                hook_runner: Some(hooks),
                session_ctx: Some(ctx),
            })
        })
        .await
    }

    /// Start an `Agent` backed by the Rust-native Anthropic (Claude Messages)
    /// runtime. Parallels [`Agent::start_gemini`]; additive and
    /// non-breaking. BYOK — `AnthropicAgentConfig::new(key)` talks directly
    /// to `api.anthropic.com`.
    #[cfg(feature = "anthropic")]
    pub async fn start_anthropic(mut config: AnthropicAgentConfig) -> Result<Self> {
        config.agent.capabilities.validate()?;
        Self::wire_response_schema(&mut config.agent);
        let mut anthropic_config = config.anthropic;
        // Keep the backend's CapabilitiesConfig in sync with the agent's so
        // register_builtins enables the right set.
        anthropic_config.capabilities = config.agent.capabilities.clone();
        let initial_history = config.initial_history.take();
        let capture: Arc<parking_lot::Mutex<Option<Arc<AnthropicConnection>>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let capture_for_factory = capture.clone();
        let mut agent = Self::start_with_factory(config.agent, move |hooks, tools, ctx| {
            AnthropicConnectionStrategy::new(anthropic_config)
                .with_runners(AnthropicRunners {
                    tool_runner: Some(tools),
                    hook_runner: Some(hooks),
                    session_ctx: Some(ctx),
                })
                .with_typed_capture(capture_for_factory)
        })
        .await?;
        agent.anthropic_connection = capture.lock().take();
        if let (Some(bytes), Some(ac)) = (initial_history, agent.anthropic_connection.as_ref()) {
            ac.set_history_bytes(&bytes)?;
        }
        Ok(agent)
    }

    /// Start an `Agent` backed by the Rust-native OpenAI (Chat Completions)
    /// runtime. Parallels [`Agent::start_anthropic`]; additive and
    /// non-breaking. BYOK — `OpenAiAgentConfig::new(key)` talks directly to
    /// `api.openai.com`; `with_base_url` routes through the credit proxy.
    #[cfg(feature = "openai")]
    pub async fn start_openai(mut config: OpenAiAgentConfig) -> Result<Self> {
        config.agent.capabilities.validate()?;
        Self::wire_response_schema(&mut config.agent);
        let mut openai_config = config.openai;
        // Keep the backend's CapabilitiesConfig in sync with the agent's so
        // register_builtins enables the right set.
        openai_config.capabilities = config.agent.capabilities.clone();
        let initial_history = config.initial_history.take();
        let capture: Arc<parking_lot::Mutex<Option<Arc<OpenAiConnection>>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let capture_for_factory = capture.clone();
        let mut agent = Self::start_with_factory(config.agent, move |hooks, tools, ctx| {
            OpenAiConnectionStrategy::new(openai_config)
                .with_runners(OpenAiRunners {
                    tool_runner: Some(tools),
                    hook_runner: Some(hooks),
                    session_ctx: Some(ctx),
                })
                .with_typed_capture(capture_for_factory)
        })
        .await?;
        agent.openai_connection = capture.lock().take();
        if let (Some(bytes), Some(oc)) = (initial_history, agent.openai_connection.as_ref()) {
            oc.set_history_bytes(&bytes)?;
        }
        Ok(agent)
    }

    /// Start an `Agent` backed by the in-browser local (Gemma 3 270M / Burn-wgpu)
    /// runtime. Parallels [`Agent::start_gemini`]; additive and non-breaking. No
    /// API key — the model runs fully on-device, reading weights from the
    /// supplied filesystem (OPFS in the browser).
    #[cfg(feature = "local")]
    pub async fn start_local(mut config: LocalAgentConfig) -> Result<Self> {
        config.agent.capabilities.validate()?;
        Self::wire_response_schema(&mut config.agent);
        let mut local_config = config.local;
        // Keep the backend's CapabilitiesConfig in sync with the agent's.
        local_config.capabilities = config.agent.capabilities.clone();
        let initial_history = config.initial_history.take();
        let capture: Arc<parking_lot::Mutex<Option<Arc<LocalConnection>>>> =
            Arc::new(parking_lot::Mutex::new(None));
        let capture_for_factory = capture.clone();
        let mut agent = Self::start_with_factory(config.agent, move |hooks, tools, ctx| {
            LocalConnectionStrategy::new(local_config)
                .with_runners(LocalRunners {
                    tool_runner: Some(tools),
                    hook_runner: Some(hooks),
                    session_ctx: Some(ctx),
                })
                .with_typed_capture(capture_for_factory)
        })
        .await?;
        agent.local_connection = capture.lock().take();
        if let (Some(bytes), Some(lc)) = (initial_history, agent.local_connection.as_ref()) {
            lc.set_history_bytes(&bytes)?;
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

    /// Opaque snapshot of the current conversation history (Gemini or, with
    /// the `anthropic` feature, the Anthropic backend). Returns `None` for
    /// backends without a typed session handle. Round-trips through the
    /// matching `with_history_bytes` for session resume.
    pub fn history_bytes(&self) -> Result<Option<Vec<u8>>> {
        if let Some(gc) = self.gemini_connection.as_ref() {
            return gc.history_bytes().map(Some);
        }
        #[cfg(feature = "anthropic")]
        if let Some(ac) = self.anthropic_connection.as_ref() {
            return ac.history_bytes().map(Some);
        }
        #[cfg(feature = "openai")]
        if let Some(oc) = self.openai_connection.as_ref() {
            return oc.history_bytes().map(Some);
        }
        #[cfg(feature = "local")]
        if let Some(lc) = self.local_connection.as_ref() {
            return lc.history_bytes().map(Some);
        }
        Ok(None)
    }

    /// Manually trigger context compaction. Summarises older history
    /// entries and replaces them with a single synthetic turn, freeing
    /// context-window budget. Returns `true` if compaction changed the
    /// history, `false` if it was too short or not applicable.
    /// Returns `false` for backends without a typed session handle.
    pub async fn compact(&self) -> bool {
        if let Some(gc) = self.gemini_connection.as_ref() {
            return gc.compact().await;
        }
        #[cfg(feature = "anthropic")]
        if let Some(ac) = self.anthropic_connection.as_ref() {
            return ac.compact().await;
        }
        #[cfg(feature = "openai")]
        if let Some(oc) = self.openai_connection.as_ref() {
            return oc.compact().await;
        }
        #[cfg(feature = "local")]
        if let Some(lc) = self.local_connection.as_ref() {
            return lc.compact().await;
        }
        false
    }

    /// Wipe the conversation history, returning the agent to a fresh, empty
    /// context — the in-tab `clear_context` tool / a "clear the chat"
    /// request. Synchronous (clearing a `Vec` needs no network). No-op for
    /// backends without a typed session handle.
    pub fn clear_history(&self) {
        // Exactly one backend connection is ever set, so each arm's `if let`
        // fires only for the active backend — no early `return` needed (and a
        // `return` is `needless_return` once the other arms are cfg'd out).
        if let Some(gc) = self.gemini_connection.as_ref() {
            gc.clear_history();
        }
        #[cfg(feature = "anthropic")]
        if let Some(ac) = self.anthropic_connection.as_ref() {
            ac.clear_history();
        }
        #[cfg(feature = "openai")]
        if let Some(oc) = self.openai_connection.as_ref() {
            oc.clear_history();
        }
        #[cfg(feature = "local")]
        if let Some(lc) = self.local_connection.as_ref() {
            lc.clear_history();
        }
    }

    /// Human-readable transcript of the current session, including tool-call
    /// activity — see [`TranscriptEntry`] for the shape. Returns an empty
    /// vec for backends without a typed session handle.
    ///
    /// [`TranscriptEntry`]: crate::types::TranscriptEntry
    pub fn transcript(&self) -> Vec<crate::types::TranscriptEntry> {
        if let Some(gc) = self.gemini_connection.as_ref() {
            return gc.transcript();
        }
        #[cfg(feature = "anthropic")]
        if let Some(ac) = self.anthropic_connection.as_ref() {
            return ac.transcript();
        }
        #[cfg(feature = "openai")]
        if let Some(oc) = self.openai_connection.as_ref() {
            return oc.transcript();
        }
        #[cfg(feature = "local")]
        if let Some(lc) = self.local_connection.as_ref() {
            return lc.transcript();
        }
        Vec::new()
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
        for hook in &agent_config.pre_tool_hooks {
            hook_runner.register_pre_tool_call_decide(hook.clone());
        }
        for hook in &agent_config.post_tool_hooks {
            hook_runner.register_post_tool_call(hook.clone());
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
            #[cfg(feature = "anthropic")]
            anthropic_connection: None,
            #[cfg(feature = "openai")]
            openai_connection: None,
            #[cfg(feature = "local")]
            local_connection: None,
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
