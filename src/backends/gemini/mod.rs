//! Rust-native Gemini agent backend.
//!
//! Replaces the 0.1.x `LocalConnection` (which proxied to Google's Go
//! `localharness` binary). The runtime hits the Gemini REST API
//! directly — zero external processes.
//!
//! See `DESIGN.md` for the phased roadmap.
//!
//! Phase 2 (this module today) adds tool calling. The agent loop
//! dispatches function calls inline through the registered hooks +
//! policies + `ToolRunner`, appends the response to history, and
//! continues until the model produces no further function calls.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use futures_util::stream::{BoxStream, StreamExt};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;

use crate::backends::gemini::api::{GeminiClient, SharedClient};
use crate::backends::gemini::r#loop::{
    run_turn, to_wire_user_content, LoopConfig, LoopState, TurnDeps,
};
use crate::backends::gemini::tools::{register_builtins, BuiltinDeps};
use crate::connections::{Connection, ConnectionStrategy};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    CapabilitiesConfig, Step, SystemInstructions, ThinkingLevel, ToolResult,
    DEFAULT_IMAGE_GENERATION_MODEL, DEFAULT_MODEL,
};

pub mod api;
#[path = "loop.rs"]
mod r#loop;
pub mod tools;
pub mod wire;

const STEP_BROADCAST_CAPACITY: usize = 256;

// =============================================================================
// Configuration
// =============================================================================

#[derive(Debug, Clone)]
pub struct GeminiBackendConfig {
    pub api_key: String,
    pub model: String,
    pub image_model: String,
    pub system_instructions: Option<SystemInstructions>,
    pub thinking: Option<ThinkingLevel>,
    /// JSON-string response schema; opt-in to structured output.
    pub response_schema: Option<String>,
    /// Override the Gemini base URL — useful for tests, proxies, or
    /// regional endpoints.
    pub base_url: Option<url::Url>,
    /// Pre-existing conversation id to resume from. When `None`, a
    /// fresh UUID is generated.
    pub conversation_id: Option<String>,
    /// Capability/built-in-tool selection. Defaults to the read-only
    /// safety set.
    pub capabilities: CapabilitiesConfig,
}

impl GeminiBackendConfig {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: DEFAULT_MODEL.to_string(),
            image_model: DEFAULT_IMAGE_GENERATION_MODEL.to_string(),
            system_instructions: None,
            thinking: None,
            response_schema: None,
            base_url: None,
            conversation_id: None,
            capabilities: CapabilitiesConfig::default(),
        }
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    pub fn with_system_instructions(mut self, s: impl Into<SystemInstructions>) -> Self {
        self.system_instructions = Some(s.into());
        self
    }

    pub fn with_thinking(mut self, level: ThinkingLevel) -> Self {
        self.thinking = Some(level);
        self
    }

    pub fn with_response_schema(mut self, schema: impl Into<String>) -> Self {
        self.response_schema = Some(schema.into());
        self
    }

    pub fn with_capabilities(mut self, c: CapabilitiesConfig) -> Self {
        self.capabilities = c;
        self
    }
}

// =============================================================================
// Strategy
// =============================================================================

#[derive(Default)]
pub struct GeminiRunners {
    pub tool_runner: Option<Arc<ToolRunner>>,
    pub hook_runner: Option<Arc<HookRunner>>,
    pub session_ctx: Option<SessionContext>,
}

pub struct GeminiConnectionStrategy {
    config: GeminiBackendConfig,
    runners: GeminiRunners,
}

impl GeminiConnectionStrategy {
    pub fn new(config: GeminiBackendConfig) -> Self {
        Self {
            config,
            runners: GeminiRunners::default(),
        }
    }

    /// Inject the runners the Agent owns. The Gemini backend dispatches
    /// custom + built-in tool calls inline; without runners those calls
    /// fall back to a static error.
    pub fn with_runners(mut self, runners: GeminiRunners) -> Self {
        self.runners = runners;
        self
    }
}

#[async_trait]
impl ConnectionStrategy for GeminiConnectionStrategy {
    async fn connect(&self) -> Result<Arc<dyn Connection>> {
        if self.config.api_key.trim().is_empty() {
            return Err(Error::config("GeminiBackendConfig.api_key is empty"));
        }
        let mut client = GeminiClient::new(self.config.api_key.clone())?;
        if let Some(base) = &self.config.base_url {
            client = client.with_base_url(base.clone());
        }
        let client: SharedClient = Arc::new(client);

        // Auto-register built-in tools per the capabilities config.
        if let Some(runner) = self.runners.tool_runner.as_ref() {
            let deps = BuiltinDeps {
                chat_client: Some(client.clone()),
                chat_model: self.config.model.clone(),
                image_client: Some(client.clone()),
                image_model: self.config.image_model.clone(),
            };
            let registered = register_builtins(runner, &self.config.capabilities, &deps);
            if !registered.is_empty() {
                tracing::debug!(?registered, "registered built-in tools");
            }
        }

        // Build tool declarations from the runner's full set.
        let tool_decls = self
            .runners
            .tool_runner
            .as_ref()
            .map(|r| build_tool_declarations(r))
            .unwrap_or_default();

        let loop_config = LoopConfig::from_system(
            self.config.model.clone(),
            self.config.system_instructions.as_ref(),
            self.config.thinking,
            self.config.response_schema.as_deref(),
            tool_decls,
        )?;

        let (steps_tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = Arc::new(LoopState::new(steps_tx));

        let conv_id = self
            .config
            .conversation_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        Ok(Arc::new(GeminiConnection {
            deps_template: TurnDeps {
                client,
                config: loop_config,
                state: state.clone(),
                tool_runner: self.runners.tool_runner.clone(),
                hook_runner: self.runners.hook_runner.clone(),
                session_ctx: self.runners.session_ctx.clone(),
            },
            state,
            conversation_id: conv_id.into(),
        }))
    }
}

fn build_tool_declarations(runner: &ToolRunner) -> Vec<wire::FunctionDeclaration> {
    runner
        .iter_tools()
        .into_iter()
        .map(|tool| wire::FunctionDeclaration {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            parameters: tool.input_schema(),
        })
        .collect()
}

// =============================================================================
// Connection
// =============================================================================

pub struct GeminiConnection {
    deps_template: TurnDeps,
    state: Arc<LoopState>,
    conversation_id: Arc<str>,
}

#[async_trait]
impl Connection for GeminiConnection {
    fn is_idle(&self) -> bool {
        self.state.idle.load(Ordering::Acquire)
    }

    fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    async fn send(&self, content: Content) -> Result<()> {
        let user = to_wire_user_content(content)?;
        let deps = self.deps_template.clone();
        tokio::spawn(async move {
            if let Err(e) = run_turn(deps, user).await {
                warn!(error = %e, "gemini turn failed");
            }
        });
        Ok(())
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        self.send(Content::text(content)).await
    }

    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        // The Gemini backend dispatches tools inline inside the loop.
        // External callers pushing tool results out-of-band is a no-op.
        Ok(())
    }

    fn subscribe_steps(&self) -> BoxStream<'static, Result<Step>> {
        let rx = self.state.steps.subscribe();
        BroadcastStream::new(rx)
            .map(|r| r.map_err(|e| Error::other(format!("gemini step lag: {e}"))))
            .boxed()
    }

    async fn wait_for_idle(&self) -> Result<()> {
        loop {
            if self.is_idle() {
                return Ok(());
            }
            self.state.idle_notify.notified().await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        self.state.idle.store(true, Ordering::Release);
        self.state.idle_notify.notify_waiters();
        Ok(())
    }
}
