//! Rust-native Gemini agent backend.
//!
//! Replaces the 0.1.x `LocalConnection` (which proxied to Google's Go
//! `localharness` binary). The runtime hits the Gemini REST API
//! directly — zero external processes.
//!
//! See `DESIGN.md` for the phased roadmap. **Phase 1 (this module
//! today) is text-only**: no tool dispatch, no thinking, no structured
//! output. Send a prompt, stream text deltas back, end the turn.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use futures_util::stream::{BoxStream, StreamExt};
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;

use crate::backends::gemini::api::{GeminiClient, SharedClient};
use crate::backends::gemini::r#loop::{
    run_turn, to_wire_user_content, LoopConfig, LoopState,
};
use crate::connections::{Connection, ConnectionStrategy};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::types::{
    Step, SystemInstructions, ThinkingLevel, ToolResult, DEFAULT_IMAGE_GENERATION_MODEL,
    DEFAULT_MODEL,
};

pub mod api;
pub mod wire;
#[path = "loop.rs"]
mod r#loop;

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
}

// =============================================================================
// Strategy
// =============================================================================

pub struct GeminiConnectionStrategy {
    config: GeminiBackendConfig,
}

impl GeminiConnectionStrategy {
    pub fn new(config: GeminiBackendConfig) -> Self {
        Self { config }
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

        let loop_config = LoopConfig::from_system(
            self.config.model.clone(),
            self.config.system_instructions.as_ref(),
            self.config.thinking,
            self.config.response_schema.as_deref(),
        )?;

        let (steps_tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = Arc::new(LoopState::new(steps_tx));

        let conv_id = self
            .config
            .conversation_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        Ok(Arc::new(GeminiConnection {
            client,
            loop_config,
            state,
            conversation_id: conv_id.into(),
        }))
    }
}

// =============================================================================
// Connection
// =============================================================================

pub struct GeminiConnection {
    client: SharedClient,
    loop_config: LoopConfig,
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
        let client = self.client.clone();
        let config = self.loop_config.clone();
        let state = self.state.clone();

        // Spawn the turn so `send` returns immediately and the caller
        // can `subscribe_steps()` to observe progress.
        tokio::spawn(async move {
            if let Err(e) = run_turn(client, config, state, user).await {
                warn!(error = %e, "gemini turn failed");
            }
        });
        Ok(())
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        // Triggers are just user messages.
        self.send(Content::text(content)).await
    }

    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        // The Gemini backend dispatches tools inline inside the loop.
        // External callers pushing tool results out-of-band is a no-op
        // here — kept on the trait so the LocalConnection still works.
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
        // No background tasks to kill (turns are self-contained tokio
        // spawns); just mark idle so `wait_for_idle` returns.
        self.state.idle.store(true, Ordering::Release);
        self.state.idle_notify.notify_waiters();
        Ok(())
    }
}
