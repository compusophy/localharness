//! Rust-native Gemini agent backend.
//!
//! Hits the Gemini REST API directly — zero external processes. The
//! agent loop dispatches function calls inline through the registered
//! hooks + policies + `ToolRunner`, appends the response to history,
//! and continues until the model produces no further function calls.

pub mod api;
pub mod compaction;
pub mod wire;
#[path = "loop.rs"]
mod r#loop;
pub mod tools;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use futures_util::stream::StreamExt;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::warn;

use crate::backends::gemini::api::{GeminiClient, SharedClient};
use crate::backends::gemini::r#loop::{
    run_turn, to_wire_user_content, LoopConfig, LoopState, TurnDeps,
};
use crate::backends::gemini::tools::{register_builtins, BuiltinDeps};
use crate::connections::{Connection, ConnectionStrategy, StepStream};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    CapabilitiesConfig, Step, SystemInstructions, ThinkingLevel, ToolResult,
    DEFAULT_IMAGE_GENERATION_MODEL, DEFAULT_MODEL,
};

const STEP_BROADCAST_CAPACITY: usize = 256;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the Gemini REST backend.
#[derive(Debug, Clone)]
pub struct GeminiBackendConfig {
    /// Gemini API key.
    pub api_key: String,
    /// Chat model ID.
    pub model: String,
    /// Image generation model ID.
    pub image_model: String,
    /// Optional system instructions for the model.
    pub system_instructions: Option<SystemInstructions>,
    /// Optional thinking (chain-of-thought) level.
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
    /// Filesystem implementation the 6 fs built-ins call into. When
    /// `None`, `connect` falls back to `NativeFilesystem::new()` on
    /// native (and to `None` on wasm — so fs builtins simply don't
    /// register until a custom impl is supplied).
    pub filesystem: Option<crate::filesystem::SharedFilesystem>,
}

impl GeminiBackendConfig {
    /// Create a new config with the given API key and default model.
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
            filesystem: None,
        }
    }

    /// Plug in a custom [`Filesystem`] implementation that the 6 fs
    /// built-ins will call into. Without this, `connect` falls back to
    /// `NativeFilesystem::new()` on native (or to no filesystem at all
    /// on wasm, in which case the fs builtins skip registration).
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.filesystem = Some(fs);
        self
    }

    /// Override the default chat model ID.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set system instructions.
    pub fn with_system_instructions(mut self, s: impl Into<SystemInstructions>) -> Self {
        self.system_instructions = Some(s.into());
        self
    }

    /// Enable extended thinking at the given level.
    pub fn with_thinking(mut self, level: ThinkingLevel) -> Self {
        self.thinking = Some(level);
        self
    }

    /// Set a JSON schema for structured output.
    pub fn with_response_schema(mut self, schema: impl Into<String>) -> Self {
        self.response_schema = Some(schema.into());
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, c: CapabilitiesConfig) -> Self {
        self.capabilities = c;
        self
    }
}

// =============================================================================
// Strategy
// =============================================================================


/// Injected runners for inline tool dispatch in the Gemini backend.
#[derive(Default)]
pub struct GeminiRunners {
    /// Tool runner for custom + built-in tool execution.
    pub tool_runner: Option<Arc<ToolRunner>>,
    /// Hook runner for pre/post tool-call hooks.
    pub hook_runner: Option<Arc<HookRunner>>,
    /// Session context for hook dispatch.
    pub session_ctx: Option<SessionContext>,
}

/// Factory that opens a [`GeminiConnection`].
pub struct GeminiConnectionStrategy {
    config: GeminiBackendConfig,
    runners: GeminiRunners,
    /// Optional out-slot: if set, `connect()` stashes a clone of the
    /// typed `Arc<GeminiConnection>` here before upcasting to the
    /// trait object. `Agent::start_gemini` uses this to keep a typed
    /// handle for backend-specific APIs (e.g. history snapshot).
    typed_capture: Option<Arc<parking_lot::Mutex<Option<Arc<GeminiConnection>>>>>,
}


impl GeminiConnectionStrategy {
    /// Create a strategy from a backend config.
    pub fn new(config: GeminiBackendConfig) -> Self {
        Self {
            config,
            runners: GeminiRunners::default(),
            typed_capture: None,
        }
    }

    /// Inject the runners the Agent owns. The Gemini backend dispatches
    /// custom + built-in tool calls inline; without runners those calls
    /// fall back to a static error.
    pub fn with_runners(mut self, runners: GeminiRunners) -> Self {
        self.runners = runners;
        self
    }

    /// Provide a slot for `connect()` to write the typed connection into.
    /// Used by `Agent::start_gemini` to retain a `&GeminiConnection` for
    /// methods like `history_bytes()` that aren't on the `Connection` trait.
    pub fn with_typed_capture(
        mut self,
        slot: Arc<parking_lot::Mutex<Option<Arc<GeminiConnection>>>>,
    ) -> Self {
        self.typed_capture = Some(slot);
        self
    }
}


#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
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
            // Honor an explicit filesystem override; otherwise fall back
            // to NativeFilesystem on native and None on wasm.
            let fs: Option<crate::filesystem::SharedFilesystem> = self
                .config
                .filesystem
                .clone()
                .or_else(default_filesystem);

            let deps = BuiltinDeps {
                chat_client: Some(client.clone()),
                chat_model: self.config.model.clone(),
                image_client: Some(client.clone()),
                image_model: self.config.image_model.clone(),
                fs,
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
            self.config.capabilities.compaction_threshold,
        )?;

        let (steps_tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = Arc::new(LoopState::new(steps_tx));

        let conv_id = self
            .config
            .conversation_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let typed = Arc::new(GeminiConnection {
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
        });
        if let Some(slot) = &self.typed_capture {
            *slot.lock() = Some(typed.clone());
        }
        Ok(typed)
    }
}


/// Default filesystem used when `GeminiBackendConfig.filesystem` is
/// `None`. On native this is `NativeFilesystem`; on wasm there is no
/// portable default, so the fs builtins simply don't register until
/// the caller supplies one via `with_filesystem`.
#[cfg(feature = "native")]
fn default_filesystem() -> Option<crate::filesystem::SharedFilesystem> {
    Some(Arc::new(crate::filesystem::NativeFilesystem::new()))
}

#[cfg(not(feature = "native"))]
fn default_filesystem() -> Option<crate::filesystem::SharedFilesystem> {
    None
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


/// A live Gemini session that implements [`Connection`].
pub struct GeminiConnection {
    deps_template: TurnDeps,
    state: Arc<LoopState>,
    conversation_id: Arc<str>,
}

impl GeminiConnection {
    /// Snapshot the current conversation history as opaque bytes.
    /// Round-trips through `set_history_bytes`; the on-disk format is
    /// not part of the public API and may change between minor versions.
    pub fn history_bytes(&self) -> Result<Vec<u8>> {
        let snapshot = self.state.history.lock().clone();
        serde_json::to_vec(&snapshot)
            .map_err(|e| Error::other(format!("history_bytes: {e}")))
    }

    /// Replace the entire conversation history with one previously
    /// returned by `history_bytes`. Use this on connection start to
    /// resume a saved session; calling it mid-turn is undefined.
    pub fn set_history_bytes(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let restored: Vec<wire::Content> = serde_json::from_slice(bytes)
            .map_err(|e| Error::other(format!("set_history_bytes: {e}")))?;
        *self.state.history.lock() = restored;
        Ok(())
    }

    /// Manually trigger context compaction. Summarises older history
    /// entries and replaces them with a single synthetic turn, freeing
    /// context-window budget. Returns `true` if compaction changed the
    /// history, `false` if it was too short or the summarisation was a
    /// no-op. Never errors — failures are logged and silently skipped.
    pub async fn compact(&self) -> bool {
        compaction::try_compact(
            &self.state.history,
            &self.deps_template.client,
            &self.deps_template.config.model,
        )
        .await
    }

    /// Project the wire history into a flat, text-only sequence of
    /// `(role, text)` turns suitable for repainting a UI. Tool-call
    /// activity (FunctionCall / FunctionResponse) is dropped — this is
    /// the human-readable view, not a fidelity snapshot.
    pub fn transcript(&self) -> Vec<crate::types::TranscriptEntry> {
        let snap = self.state.history.lock().clone();
        project_history(&snap)
    }
}

/// Decode the opaque bytes produced by [`GeminiConnection::history_bytes`]
/// into a flat user-visible transcript, without needing a live
/// connection. Useful for repainting a UI on page load before any
/// agent has been started.
pub fn decode_transcript_bytes(bytes: &[u8]) -> Result<Vec<crate::types::TranscriptEntry>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let history: Vec<wire::Content> = serde_json::from_slice(bytes)
        .map_err(|e| Error::other(format!("decode_transcript_bytes: {e}")))?;
    Ok(project_history(&history))
}

fn project_history(history: &[wire::Content]) -> Vec<crate::types::TranscriptEntry> {
    use crate::types::{TranscriptEntry, TranscriptRole, TranscriptToolCall};
    use wire::{ContentRole, Part};
    let mut out = Vec::with_capacity(history.len());
    let mut pending_calls: Vec<TranscriptToolCall> = Vec::new();

    for content in history {
        let role = match content.role {
            ContentRole::User => TranscriptRole::User,
            ContentRole::Model => TranscriptRole::Assistant,
        };
        let mut buf = String::new();
        let mut calls_this_turn: Vec<TranscriptToolCall> = Vec::new();

        for part in &content.parts {
            match part {
                Part::Text { text } => buf.push_str(text),
                Part::Thought {
                    thought: false,
                    text: Some(text),
                    ..
                } => buf.push_str(text),
                Part::FunctionCall { function_call } => {
                    calls_this_turn.push(TranscriptToolCall {
                        name: function_call.name.clone(),
                        args: function_call.args.clone(),
                        result: None,
                        error: None,
                    });
                }
                Part::FunctionResponse { function_response } => {
                    // Match response to a pending call by name
                    if let Some(call) = pending_calls.iter_mut().find(|c| c.name == function_response.name && c.result.is_none()) {
                        call.result = Some(function_response.response.clone());
                    }
                }
                _ => {}
            }
        }

        if !calls_this_turn.is_empty() {
            pending_calls.extend(calls_this_turn.clone());
        }

        if !buf.is_empty() || !calls_this_turn.is_empty() {
            // Attach any completed tool calls to this entry
            let attached = if role == TranscriptRole::Assistant {
                std::mem::take(&mut pending_calls)
            } else {
                Vec::new()
            };
            out.push(TranscriptEntry { role, text: buf, tool_calls: attached });
        }
    }
    out
}


#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
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
        crate::runtime::spawn(async move {
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

    fn subscribe_steps(&self) -> StepStream {
        let rx = self.state.steps.subscribe();
        let mapped = BroadcastStream::new(rx)
            .map(|r| r.map_err(|e| Error::other(format!("gemini step lag: {e}"))));
        #[cfg(not(target_arch = "wasm32"))]
        {
            mapped.boxed()
        }
        #[cfg(target_arch = "wasm32")]
        {
            mapped.boxed_local()
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{DirEntry, EntryKind, Filesystem, Metadata, WalkEntry};
    use crate::tools::ToolRunner;
    use async_trait::async_trait;
    use parking_lot::Mutex;
    use serde_json::json;

    /// Test Filesystem that records every method invocation. Returns
    /// minimal valid responses for each call.
    #[derive(Debug, Default)]
    struct TrackingFs {
        calls: Mutex<Vec<String>>,
    }

    impl TrackingFs {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().clone()
        }
        fn record(&self, s: String) {
            self.calls.lock().push(s);
        }
    }

    #[async_trait]
    impl Filesystem for TrackingFs {
        async fn read(&self, path: &str) -> Result<Vec<u8>> {
            self.record(format!("read:{path}"));
            Ok(b"hello\n".to_vec())
        }
        async fn write_atomic(&self, path: &str, _bytes: &[u8]) -> Result<()> {
            self.record(format!("write_atomic:{path}"));
            Ok(())
        }
        async fn metadata(&self, path: &str) -> Result<Option<Metadata>> {
            self.record(format!("metadata:{path}"));
            Ok(None)
        }
        async fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>> {
            self.record(format!("read_dir:{path}"));
            Ok(vec![DirEntry {
                name: "stub".into(),
                kind: EntryKind::File,
                size: Some(0),
            }])
        }
        async fn walk(&self, path: &str, _max_depth: Option<usize>) -> Result<Vec<WalkEntry>> {
            self.record(format!("walk:{path}"));
            Ok(Vec::new())
        }
        async fn delete(&self, path: &str) -> Result<()> {
            self.record(format!("delete:{path}"));
            Ok(())
        }
    }

    /// `with_filesystem` must override the default and the runtime must
    /// route the 6 fs builtins through the supplied impl.
    #[tokio::test]
    async fn with_filesystem_override_flows_to_tools() {
        let fs = Arc::new(TrackingFs::default());
        let runner = Arc::new(ToolRunner::new());

        let cfg = GeminiBackendConfig::new("test-key")
            .with_capabilities(CapabilitiesConfig::unrestricted())
            .with_filesystem(fs.clone());

        let strategy = GeminiConnectionStrategy::new(cfg).with_runners(GeminiRunners {
            tool_runner: Some(runner.clone()),
            ..Default::default()
        });

        // connect() registers the builtins against our TrackingFs.
        let _conn = strategy.connect().await.unwrap();

        // Sanity: the fs builtins are now registered.
        let names = runner.names();
        for expected in [
            "list_directory",
            "view_file",
            "find_file",
            "search_directory",
            "create_file",
            "edit_file",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "missing builtin {expected} (got {names:?})",
            );
        }

        // Invoke list_directory — it must call TrackingFs::read_dir.
        let out = runner
            .execute("list_directory", json!({"path": "/synthetic/dir"}))
            .await
            .unwrap();
        assert_eq!(out["count"].as_u64(), Some(1));
        let calls = fs.calls();
        assert!(
            calls.iter().any(|c| c == "read_dir:/synthetic/dir"),
            "expected read_dir call recorded; got {calls:?}",
        );
    }
}
