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
use tokio::sync::broadcast;
use tracing::warn;

use crate::backends::gemini::api::{GeminiClient, SharedClient};
use crate::backends::gemini::r#loop::{
    run_turn, to_wire_user_content, LoopConfig, LoopState, TurnDeps,
};
use crate::backends::gemini::tools::{register_builtins, BuiltinDeps};
use crate::connections::{Connection, ConnectionStrategy, StepStream};
use crate::content::Content;
use crate::error::{Error, Result};
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
    /// Cap on output tokens (`maxOutputTokens`) per model call. `None` lets
    /// Gemini apply its own default — which, for a 3.x model doing dynamic
    /// thinking, can be exhausted by reasoning alone on a hard task, leaving
    /// no budget for a final answer (the turn ends `MAX_TOKENS` with empty
    /// text → "(empty response)"). Set this high so a hard task can BOTH reason
    /// AND answer in one call.
    pub max_output_tokens: Option<u32>,
    /// Sampling temperature (`generationConfig.temperature`). `None` lets
    /// Gemini apply its default. A low value (e.g. 0.2) favors first-try-valid
    /// code/edits. Composes with `thinking` — both ride `generationConfig`.
    pub temperature: Option<f32>,
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
    /// Per-request auth-token provider. When set, every HTTP request mints
    /// a fresh credential instead of reusing `api_key` (which becomes a
    /// fallback). Required for the credit proxy's 5-minute token window.
    pub api_key_provider: Option<crate::backends::AuthTokenProvider>,
    /// Extra headers attached to EVERY outbound request (e.g. an `X-PAYMENT`
    /// x402 authorization). Empty by default — a no-op.
    pub extra_headers: Vec<(String, String)>,
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
            max_output_tokens: None,
            temperature: None,
            base_url: None,
            conversation_id: None,
            capabilities: CapabilitiesConfig::default(),
            filesystem: None,
            api_key_provider: None,
            extra_headers: Vec::new(),
        }
    }

    /// Attach extra headers to every outbound HTTP request (e.g. an
    /// `X-PAYMENT` x402 authorization). No-op when empty.
    pub fn with_extra_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((name.into(), value.into()));
        self
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

    /// Cap output tokens (`maxOutputTokens`) per model call. Set this high
    /// enough that a hard task can both reason (dynamic thinking) AND emit a
    /// final answer in one call; an unset/low cap lets thinking starve the
    /// text on a 3.x model, ending the turn `MAX_TOKENS` with no output.
    pub fn with_max_output_tokens(mut self, max: u32) -> Self {
        self.max_output_tokens = Some(max);
        self
    }

    /// Set the sampling temperature (`generationConfig.temperature`). A low
    /// value (e.g. 0.2) favors first-try-valid code/edits.
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, c: CapabilitiesConfig) -> Self {
        self.capabilities = c;
        self
    }

    /// Route requests through an alternate base URL (e.g. the
    /// localharness credit proxy) instead of
    /// `generativelanguage.googleapis.com`. In credits mode the
    /// `api_key` carries the proxy auth token rather than a Gemini key.
    pub fn with_base_url(mut self, url: url::Url) -> Self {
        self.base_url = Some(url);
        self
    }
}

// =============================================================================
// Strategy
// =============================================================================


/// Injected runners for inline tool dispatch in the Gemini backend — an
/// alias of the shared [`BackendRunners`](crate::backends::BackendRunners).
pub type GeminiRunners = crate::backends::BackendRunners;

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
        if let Some(provider) = &self.config.api_key_provider {
            client = client.with_key_provider(provider.0.clone());
        }
        if !self.config.extra_headers.is_empty() {
            client = client.with_extra_headers(self.config.extra_headers.clone());
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

        let mut loop_config = LoopConfig::from_system(
            self.config.model.clone(),
            self.config.system_instructions.as_ref(),
            self.config.thinking,
            self.config.response_schema.as_deref(),
            tool_decls,
            self.config.capabilities.compaction_threshold,
        )?;
        loop_config.max_output_tokens = self.config.max_output_tokens;
        loop_config.temperature = self.config.temperature;

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
            thinking_override: parking_lot::Mutex::new(None),
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
    /// Optional PER-TURN thinking override (the difficulty router seam). When
    /// `Some`, the NEXT [`Connection::send`] overrides the baked-in
    /// `LoopConfig.thinking` for that turn only; when `None` the session's
    /// configured level applies. Interior-mutable so the in-tab loop can retune
    /// thinking per turn WITHOUT rebuilding the connection. `None` by default →
    /// behavior is identical to before for every caller that never sets it.
    thinking_override: parking_lot::Mutex<Option<ThinkingLevel>>,
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

    /// Set (or clear, with `None`) the PER-TURN thinking override — the
    /// difficulty-router seam. Applies to the NEXT turn's model call only; the
    /// session's configured thinking is restored after. `None` (the default)
    /// means "use the configured level". Cheap to call between turns; does NOT
    /// rebuild the connection or touch history.
    pub fn set_thinking_override(&self, level: Option<ThinkingLevel>) {
        *self.thinking_override.lock() = level;
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

    /// Wipe the entire conversation history, returning the connection to a
    /// fresh, empty context. Synchronous (no network). Backs
    /// [`crate::Agent::clear_history`] — the in-tab `clear_context` tool.
    /// Resets only the history and the per-turn bookkeeping that could
    /// otherwise re-trigger compaction on the now-tiny history; the live
    /// step broadcast and `conversation_id` are left untouched.
    pub fn clear_history(&self) {
        self.state.history.lock().clear();
        *self.state.last_turn_usage.lock() = None;
        *self.state.last_structured_output.lock() = None;
        self.state
            .next_step_index
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Project the wire history into a flat sequence of user/assistant
    /// turns suitable for repainting a UI. Tool-call activity
    /// (FunctionCall / FunctionResponse) is surfaced as `TranscriptToolCall`s
    /// (matched by name), so a restored session shows what the agent DID, not
    /// just what it said.
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
    // Per-entry lenient: decode the array generically, then try each entry on
    // its own and SKIP the ones that fail, rather than letting a single
    // malformed/older-format entry blank the WHOLE restored transcript. Only a
    // top-level "this isn't a JSON array" error is fatal.
    let raw: Vec<serde_json::Value> = serde_json::from_slice(bytes)
        .map_err(|e| Error::other(format!("decode_transcript_bytes: {e}")))?;
    let history: Vec<wire::Content> = raw
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    Ok(project_history(&history))
}

fn project_history(history: &[wire::Content]) -> Vec<crate::types::TranscriptEntry> {
    use crate::types::{TranscriptEntry, TranscriptRole, TranscriptToolCall};
    use std::collections::{HashMap, VecDeque};
    use wire::{ContentRole, Part};
    let mut out: Vec<TranscriptEntry> = Vec::with_capacity(history.len());

    // Gemini lays the wire out as a Model content `[Text?, FunctionCall*]`
    // IMMEDIATELY followed by a User content `[FunctionResponse*]` (the SDK
    // sends tool results back as a user turn — see `loop.rs`). So a call's
    // result lands in the NEXT content, AFTER the assistant entry is already
    // pushed. We can't drain at push time (the old bug — `result` stayed
    // None forever). Instead we push the assistant entry with results pending
    // and remember WHERE each call lives — a per-name FIFO of
    // `(entry_idx, call_idx)` — so a later FunctionResponse fills the call it
    // answers. Matching by name is the load-bearing Gemini difference
    // (FunctionResponse carries no call id; Anthropic matches by id).
    let mut pending: HashMap<String, VecDeque<(usize, usize)>> = HashMap::new();

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
                Part::FunctionCall { function_call, .. } => {
                    calls_this_turn.push(TranscriptToolCall {
                        name: function_call.name.clone(),
                        args: function_call.args.clone(),
                        result: None,
                        error: None,
                    });
                }
                Part::FunctionResponse { function_response } => {
                    // Fill the earliest unanswered call of this name. The live
                    // path encodes tool failures as `{"error": "..."}` in the
                    // wire response (see `loop.rs`); lift that into the typed
                    // `error` field so replay renders the red error pill, not a
                    // green result pill — byte-identical to a live error.
                    if let Some((ei, ci)) = pending
                        .get_mut(&function_response.name)
                        .and_then(|q| q.pop_front())
                    {
                        if let Some(call) =
                            out.get_mut(ei).and_then(|e| e.tool_calls.get_mut(ci))
                        {
                            let resp = &function_response.response;
                            match resp.get("error").and_then(|e| e.as_str()) {
                                Some(msg) => call.error = Some(msg.to_string()),
                                None => call.result = Some(resp.clone()),
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        if !buf.is_empty() || !calls_this_turn.is_empty() {
            let entry_idx = out.len();
            // Index each call so a later FunctionResponse can fill it.
            for (ci, call) in calls_this_turn.iter().enumerate() {
                pending
                    .entry(call.name.clone())
                    .or_default()
                    .push_back((entry_idx, ci));
            }
            out.push(TranscriptEntry {
                role,
                text: buf,
                tool_calls: calls_this_turn,
            });
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
        // Clone is cheap (media parts are `Bytes`); the original rides along
        // so pre-turn hooks inspect the SDK-level prompt, not wire JSON.
        let user = to_wire_user_content(content.clone())?;
        let mut deps = self.deps_template.clone();
        // Apply the per-turn thinking override (difficulty router) for THIS
        // turn only — the template is untouched, so the next turn without an
        // override falls back to the configured level. `None` is a no-op.
        if let Some(level) = *self.thinking_override.lock() {
            deps.config.thinking = Some(level);
        }
        crate::runtime::spawn(async move {
            if let Err(e) = run_turn(deps, user, content).await {
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
        // Turn-failure Steps (System/Error from `emit_error`) surface as
        // stream `Err` — uniform across backends; previously gemini passed
        // them through as `Ok`, which downstream read as an empty success.
        crate::backends::subscribe_step_stream(self.state.steps.subscribe(), "gemini")
    }

    async fn wait_for_idle(&self) -> Result<()> {
        loop {
            if self.is_idle() {
                return Ok(());
            }
            self.state.idle_notify.notified().await;
        }
    }

    fn cancel_turn(&self) {
        self.state.cancel.store(true, Ordering::Release);
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

    /// `clear_history` empties the conversation AND resets the per-turn
    /// bookkeeping (so a stale prompt-token count can't re-trigger
    /// compaction on the now-tiny history). Backs `Agent::clear_history` —
    /// the in-tab `clear_context` tool.
    #[tokio::test]
    async fn clear_history_empties_history_and_resets_counters() {
        let capture: Arc<Mutex<Option<Arc<GeminiConnection>>>> = Arc::new(Mutex::new(None));
        let cfg =
            GeminiBackendConfig::new("test-key").with_capabilities(CapabilitiesConfig::unrestricted());
        let strategy = GeminiConnectionStrategy::new(cfg)
            .with_runners(GeminiRunners {
                tool_runner: Some(Arc::new(ToolRunner::new())),
                ..Default::default()
            })
            .with_typed_capture(capture.clone());
        let _conn = strategy.connect().await.unwrap();
        let gc = capture.lock().take().expect("typed capture filled by connect");

        // Seed a turn + non-default per-turn bookkeeping.
        gc.state.history.lock().push(wire::Content {
            role: wire::ContentRole::User,
            parts: vec![wire::Part::Text { text: "hello".into() }],
        });
        *gc.state.last_turn_usage.lock() = Some(crate::types::UsageMetadata::default());
        *gc.state.last_structured_output.lock() = Some(json!({ "x": 1 }));
        gc.state
            .next_step_index
            .store(7, std::sync::atomic::Ordering::Relaxed);

        gc.clear_history();

        assert!(gc.state.history.lock().is_empty(), "history not cleared");
        assert!(gc.state.last_turn_usage.lock().is_none(), "usage not reset");
        assert!(
            gc.state.last_structured_output.lock().is_none(),
            "structured output not reset"
        );
        assert_eq!(
            gc.state
                .next_step_index
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "step index not reset"
        );
        // The public snapshot reflects the wipe too.
        let snapshot: Vec<wire::Content> =
            serde_json::from_slice(&gc.history_bytes().unwrap()).unwrap();
        assert!(snapshot.is_empty(), "history_bytes snapshot not empty");
    }

    /// Transcript projection matches each FunctionResponse back to the
    /// FunctionCall it answers (Gemini matches by NAME — the response carries
    /// no call id — and the result lands in the User content that FOLLOWS the
    /// Model content, so the assistant entry is already pushed when the result
    /// arrives). REGRESSION: before the fix, `pending_calls` was drained when
    /// the assistant entry was pushed, so the result was never attached
    /// (`result` stayed `None`) and a reload lost every tool result.
    /// REGRESSION: a persisted-history entry missing `parts` (older on-disk
    /// format) must NOT fail the whole `decode_transcript_bytes` — that left a
    /// returning user with a blank transcript despite having history. The bad
    /// entry decodes as empty (skipped); the rest projects normally.
    #[test]
    fn decode_tolerates_entry_missing_parts() {
        // First content has no `parts` field at all; second is a normal turn.
        let raw = br#"[{"role":"user"},{"role":"model","parts":[{"text":"hi there"}]}]"#;
        let entries = decode_transcript_bytes(raw).expect("must not error on missing parts");
        assert_eq!(entries.len(), 1, "the part-less entry is skipped, the real one kept");
        assert_eq!(entries[0].text, "hi there");
    }

    /// A Claude-backed agent persists model turns with role `assistant`; the
    /// Gemini decoder must accept it (alias for `model`) instead of blanking
    /// the transcript. Also: a totally-unparseable entry is skipped, not fatal.
    #[test]
    fn decode_accepts_assistant_role_and_skips_garbage() {
        let raw = br#"[{"role":"user","parts":[{"text":"q"}]},{"role":"assistant","parts":[{"text":"a"}]},{"garbage":true}]"#;
        let entries = decode_transcript_bytes(raw).expect("must not error");
        assert_eq!(entries.len(), 2, "user + assistant kept, garbage skipped");
        assert_eq!(entries[0].text, "q");
        assert_eq!(entries[1].text, "a");
        assert!(matches!(entries[1].role, crate::types::TranscriptRole::Assistant));
    }

    #[test]
    fn transcript_attaches_results_to_calls_by_name() {
        use wire::{Content, ContentRole, FunctionCall, FunctionResponse, Part};
        let history = vec![
            Content {
                role: ContentRole::User,
                parts: vec![Part::Text {
                    text: "read main.rs".into(),
                }],
            },
            Content {
                role: ContentRole::Model,
                parts: vec![
                    Part::Text {
                        text: "Reading.".into(),
                    },
                    Part::FunctionCall {
                        function_call: FunctionCall {
                            name: "view_file".into(),
                            args: json!({"path": "main.rs"}),
                        },
                        thought_signature: None,
                    },
                ],
            },
            Content {
                role: ContentRole::User,
                parts: vec![Part::FunctionResponse {
                    function_response: FunctionResponse {
                        name: "view_file".into(),
                        response: json!({"contents": "fn main() {}"}),
                    },
                }],
            },
            Content {
                role: ContentRole::Model,
                parts: vec![Part::Text { text: "Done.".into() }],
            },
        ];
        let entries = project_history(&history);
        let asst = entries
            .iter()
            .find(|e| !e.tool_calls.is_empty())
            .expect("assistant entry with a tool call");
        assert_eq!(asst.tool_calls.len(), 1);
        assert_eq!(asst.tool_calls[0].name, "view_file");
        assert_eq!(
            asst.tool_calls[0].result.as_ref().expect("result attached")["contents"],
            "fn main() {}"
        );
        assert!(asst.tool_calls[0].error.is_none());
    }

    /// A FunctionResponse encoding the live-path `{"error": "..."}` failure
    /// convention surfaces as the typed `error` (red pill on replay), not a
    /// success `result` — byte-identical to a live tool error.
    #[test]
    fn transcript_lifts_error_convention_into_error_field() {
        use wire::{Content, ContentRole, FunctionCall, FunctionResponse, Part};
        let history = vec![
            Content {
                role: ContentRole::Model,
                parts: vec![Part::FunctionCall {
                    function_call: FunctionCall {
                        name: "view_file".into(),
                        args: json!({"path": "missing"}),
                    },
                    thought_signature: None,
                }],
            },
            Content {
                role: ContentRole::User,
                parts: vec![Part::FunctionResponse {
                    function_response: FunctionResponse {
                        name: "view_file".into(),
                        response: json!({"error": "no such file"}),
                    },
                }],
            },
        ];
        let entries = project_history(&history);
        let call = &entries[0].tool_calls[0];
        assert_eq!(call.error.as_deref(), Some("no such file"));
        assert!(call.result.is_none());
    }
}
