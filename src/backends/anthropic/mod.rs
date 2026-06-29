//! Rust-native Anthropic (Claude Messages API) agent backend.
//!
//! A second `ConnectionStrategy`/`Connection` pair behind the same Layer-3
//! seam the Gemini backend implements — proving the model-agnostic
//! architecture by construction. Hits `POST /v1/messages` directly (BYOK
//! via `AnthropicBackendConfig::new`); a `with_base_url` override exists
//! for the future credit proxy (Phase C, out of scope here).
//!
//! Mirrors `backends/gemini/mod.rs` 1:1: `send` spawns the turn loop, a
//! broadcast channel feeds `subscribe_steps`, idle is tracked on an
//! `AtomicBool`, and tool calls dispatch inline through the registered
//! hooks + policies + `ToolRunner`. Only the wire shapes differ (see
//! `wire.rs` / `loop.rs`).
//!
//! Built-in tools are registered by REUSING the Gemini backend's
//! `register_builtins` with both client slots set to `None` — every
//! portable + filesystem builtin registers exactly as on Gemini, while
//! the two Gemini-client-coupled tools (`start_subagent`, `generate_image`)
//! simply don't register on this backend (Anthropic has no image endpoint;
//! a neutral subagent is the design's deferred `OneShot` refactor).

pub(crate) mod api;
#[allow(dead_code)] // backend-internal; some helpers are target/test-only
pub(crate) mod compaction;
#[allow(dead_code)] // wire DTOs: serde-populated fields aren't all read in Rust
pub(crate) mod wire;
#[path = "loop.rs"]
mod r#loop;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tracing::warn;

use crate::backends::anthropic::api::{AnthropicClient, SharedClient};
use crate::backends::anthropic::r#loop::{
    run_turn, to_wire_user_content, LoopConfig, LoopState, TurnDeps,
};
use crate::backends::anthropic::wire::ToolDef;
// Built-in tool registration is reused verbatim from the Gemini backend —
// the tools consume neutral `Tool::input_schema()` JSON, which Anthropic's
// `tools[].input_schema` takes raw.
use crate::backends::gemini::tools::{register_builtins, BuiltinDeps};
use crate::connections::{Connection, ConnectionStrategy, StepStream};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::tools::ToolRunner;
use crate::types::{
    CapabilitiesConfig, Step, SystemInstructions, ThinkingLevel, ToolResult,
};

pub use wire::{DEFAULT_MODEL, OPUS_MODEL, SONNET_MODEL};

const STEP_BROADCAST_CAPACITY: usize = 256;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the Anthropic Messages backend.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AnthropicBackendConfig {
    /// Anthropic API key (BYOK) — or, in credits mode, the proxy auth token.
    pub api_key: String,
    /// Chat model ID. Defaults to [`wire::DEFAULT_MODEL`].
    pub model: String,
    /// Optional system instructions (flattened to the top-level `system`).
    pub system_instructions: Option<SystemInstructions>,
    /// Optional extended-thinking level.
    pub thinking: Option<ThinkingLevel>,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    /// `max_tokens` for the response. Anthropic REQUIRES this; defaults to
    /// `wire::DEFAULT_MAX_TOKENS`.
    pub max_tokens: Option<u32>,
    /// Override the base URL (test server, proxy, regional endpoint).
    pub base_url: Option<url::Url>,
    /// Pre-existing conversation id to resume from. `None` → fresh UUID.
    pub conversation_id: Option<String>,
    /// Capability/built-in-tool selection.
    pub capabilities: CapabilitiesConfig,
    /// Filesystem impl the fs built-ins call into. `None` → `NativeFilesystem`
    /// on native, nothing on wasm (caller supplies OPFS).
    pub filesystem: Option<crate::filesystem::SharedFilesystem>,
    /// Per-request auth-token provider. When set, every HTTP request mints
    /// a fresh credential instead of reusing `api_key` (which becomes a
    /// fallback). Required for the credit proxy's 5-minute token window.
    pub api_key_provider: Option<crate::backends::AuthTokenProvider>,
    /// Extra headers attached to EVERY outbound request (e.g. an `X-PAYMENT`
    /// x402 authorization). Empty by default — a no-op.
    pub extra_headers: Vec<(String, String)>,
}

impl AnthropicBackendConfig {
    /// Create a config with the given API key and default model — BYOK,
    /// talks directly to `api.anthropic.com`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: DEFAULT_MODEL.to_string(),
            system_instructions: None,
            thinking: None,
            temperature: None,
            max_tokens: None,
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

    /// Plug in a custom [`Filesystem`] impl for the fs built-ins.
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.filesystem = Some(fs);
        self
    }

    /// Override the chat model ID (e.g. [`SONNET_MODEL`], [`OPUS_MODEL`]).
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

    /// Set the sampling temperature.
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Set `max_tokens` for the response.
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, c: CapabilitiesConfig) -> Self {
        self.capabilities = c;
        self
    }

    /// Route requests through an alternate base URL (e.g. the future
    /// localharness credit proxy). In credits mode `api_key` carries the
    /// proxy auth token. The proxy/credits routing itself is Phase C and
    /// out of scope — this is just the override seam.
    pub fn with_base_url(mut self, url: url::Url) -> Self {
        self.base_url = Some(url);
        self
    }
}

// =============================================================================
// Strategy
// =============================================================================

/// Injected runners for inline tool dispatch in the Anthropic backend — an
/// alias of the shared [`BackendRunners`](crate::backends::BackendRunners).
pub type AnthropicRunners = crate::backends::BackendRunners;

/// Factory that opens an [`AnthropicConnection`].
pub struct AnthropicConnectionStrategy {
    config: AnthropicBackendConfig,
    runners: AnthropicRunners,
    /// Optional out-slot: `connect()` stashes a clone of the typed
    /// `Arc<AnthropicConnection>` here before upcasting. `Agent::start_anthropic`
    /// uses this to keep a typed handle for backend-specific APIs.
    typed_capture: Option<Arc<parking_lot::Mutex<Option<Arc<AnthropicConnection>>>>>,
}

impl AnthropicConnectionStrategy {
    /// Create a strategy from a backend config.
    pub fn new(config: AnthropicBackendConfig) -> Self {
        Self {
            config,
            runners: AnthropicRunners::default(),
            typed_capture: None,
        }
    }

    /// Inject the runners the Agent owns (inline tool dispatch).
    pub fn with_runners(mut self, runners: AnthropicRunners) -> Self {
        self.runners = runners;
        self
    }

    /// Provide a slot for `connect()` to write the typed connection into.
    pub fn with_typed_capture(
        mut self,
        slot: Arc<parking_lot::Mutex<Option<Arc<AnthropicConnection>>>>,
    ) -> Self {
        self.typed_capture = Some(slot);
        self
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl ConnectionStrategy for AnthropicConnectionStrategy {
    async fn connect(&self) -> Result<Arc<dyn Connection>> {
        if self.config.api_key.trim().is_empty() {
            return Err(Error::config("AnthropicBackendConfig.api_key is empty"));
        }
        let mut client = AnthropicClient::new(self.config.api_key.clone())?;
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

        // Auto-register built-in tools per the capabilities config, reusing
        // the Gemini backend's `register_builtins`. Both client slots are
        // None: the fs + portable builtins register; the two
        // Gemini-client-coupled tools (start_subagent / generate_image)
        // don't register on this backend.
        if let Some(runner) = self.runners.tool_runner.as_ref() {
            let fs: Option<crate::filesystem::SharedFilesystem> =
                self.config.filesystem.clone().or_else(default_filesystem);
            let deps = BuiltinDeps {
                chat_client: None,
                chat_model: self.config.model.clone(),
                image_client: None,
                image_model: String::new(),
                fs,
                hooks: self.runners.hook_runner.clone(), // for subagent policy inheritance (M8); inert here (no chat_client)
            };
            let registered = register_builtins(runner, &self.config.capabilities, &deps);
            if !registered.is_empty() {
                tracing::debug!(?registered, "registered built-in tools (anthropic)");
            }
        }

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
            self.config.temperature,
            self.config.max_tokens,
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

        let typed = Arc::new(AnthropicConnection {
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
            model_override: parking_lot::Mutex::new(None),
        });
        if let Some(slot) = &self.typed_capture {
            *slot.lock() = Some(typed.clone());
        }
        Ok(typed)
    }
}

/// Default filesystem when `filesystem` is `None`: `NativeFilesystem` on
/// native, `None` on wasm.
#[cfg(feature = "native")]
fn default_filesystem() -> Option<crate::filesystem::SharedFilesystem> {
    Some(Arc::new(crate::filesystem::NativeFilesystem::new()))
}

#[cfg(not(feature = "native"))]
fn default_filesystem() -> Option<crate::filesystem::SharedFilesystem> {
    None
}

fn build_tool_declarations(runner: &ToolRunner) -> Vec<ToolDef> {
    let mut decls: Vec<ToolDef> = runner
        .iter_tools()
        .into_iter()
        .map(|tool| ToolDef {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.input_schema(),
            cache_control: None,
        })
        .collect();
    // Prompt caching: a single `cache_control` breakpoint on the LAST tool
    // pins the whole (stable) tool block — ~50 schemas — for the cache, which
    // tools+system then read at ~0.1× cost on every later turn. GA; no beta
    // header. The tool ORDER is deterministic (registration order), so the
    // cached prefix stays byte-stable across turns.
    if let Some(last) = decls.last_mut() {
        last.cache_control = Some(crate::backends::anthropic::wire::CacheControl::ephemeral());
    }
    decls
}

// =============================================================================
// Connection
// =============================================================================

/// A live Anthropic session that implements [`Connection`].
pub struct AnthropicConnection {
    deps_template: TurnDeps,
    state: Arc<LoopState>,
    conversation_id: Arc<str>,
    /// Optional PER-TURN thinking override (the difficulty router seam) — see
    /// [`GeminiConnection::set_thinking_override`]. `None` by default → no
    /// behavior change for callers that never set it.
    thinking_override: parking_lot::Mutex<Option<ThinkingLevel>>,
    /// Optional PER-TURN model override (difficulty router, #7). When `Some`,
    /// the NEXT [`Connection::send`] uses this model id instead of the
    /// configured one FOR THAT TURN ONLY. Safe because the credits/proxy path
    /// sends every model to the SAME endpoint (the model is just a request
    /// field), so a same-backend swap needs no connection rebuild. The wiring
    /// guarantees the override is always a same-family (`claude-*`) id no more
    /// capable than the user's selected model. `None` by default → byte-
    /// identical no-op for callers that never set it.
    model_override: parking_lot::Mutex<Option<String>>,
}

impl AnthropicConnection {
    /// Snapshot the current conversation history as opaque bytes.
    /// Round-trips through `set_history_bytes`. The on-disk format (a JSON
    /// array of Anthropic `Message`s) is not part of the public API.
    pub fn history_bytes(&self) -> Result<Vec<u8>> {
        crate::backends::state::history::encode(&self.state.history.lock())
    }

    /// Set (or clear, with `None`) the PER-TURN thinking override — the
    /// difficulty-router seam (parallels
    /// [`super::gemini::GeminiConnection::set_thinking_override`]). Applies to
    /// the NEXT turn only; the configured thinking is restored after. `None`
    /// (the default) is a no-op. NOTE: on Anthropic, thinking and temperature
    /// are mutually exclusive — the loop already drops temperature when
    /// thinking is on, so an override that turns thinking on/off composes the
    /// same way it does at session build.
    pub fn set_thinking_override(&self, level: Option<ThinkingLevel>) {
        *self.thinking_override.lock() = level;
    }

    /// Set (or clear, with `None`) the PER-TURN model override — the
    /// difficulty-router model seam (#7), parallel to
    /// [`set_thinking_override`](Self::set_thinking_override). Applies to the
    /// NEXT turn only; the configured model is restored after. `None` (the
    /// default) is a no-op. The caller MUST pass a same-backend (`claude-*`) id
    /// no more capable than the session's selected model — switching backends
    /// is unsafe (different wire format + history shape) and must never be done
    /// here. Cheap: does NOT rebuild the connection or touch history; the proxy
    /// routes every model to the same endpoint, so only the request's `model`
    /// field changes.
    pub fn set_model_override(&self, model: Option<String>) {
        *self.model_override.lock() = model;
    }

    /// Replace the entire conversation history with one previously returned
    /// by `history_bytes`. Use on connection start to resume a session.
    pub fn set_history_bytes(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let restored: Vec<wire::Message> = crate::backends::state::history::decode(bytes)?;
        *self.state.history.lock() = restored;
        Ok(())
    }

    /// Manually trigger context compaction. Returns `true` if compaction
    /// changed the history. Never errors — failures are logged + skipped.
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
    /// otherwise re-trigger compaction; the live step broadcast and
    /// `conversation_id` are left untouched.
    pub fn clear_history(&self) {
        self.state.history.lock().clear();
        *self.state.last_turn_usage.lock() = None;
        *self.state.last_structured_output.lock() = None;
        self.state
            .next_step_index
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Project the wire history into a flat, text-only `(role, text)`
    /// transcript suitable for repainting a UI. Tool-call activity is
    /// surfaced as `TranscriptToolCall`s (matched by `tool_use_id`).
    pub fn transcript(&self) -> Vec<crate::types::TranscriptEntry> {
        let snap = self.state.history.lock().clone();
        project_history(&snap)
    }
}

/// Decode opaque bytes from [`AnthropicConnection::history_bytes`] into a
/// flat transcript without a live connection.
pub fn decode_transcript_bytes(bytes: &[u8]) -> Result<Vec<crate::types::TranscriptEntry>> {
    let history: Vec<wire::Message> = crate::backends::state::history::decode_lenient(bytes)?;
    Ok(project_history(&history))
}

/// True iff `bytes` STRICTLY decode as this backend's history — the EXACT parse
/// [`AnthropicConnection::set_history_bytes`] performs. The browser persists ONE
/// history file across model switches, so it must gate cross-backend seeding on
/// this rather than on [`decode_transcript_bytes`]: that decoder is LENIENT
/// (`decode_lenient` skips per-entry failures, so a Gemini-format blob returns
/// `Ok(empty)` and slips past an `.is_ok()` check), after which `set_history_bytes`
/// crashed the whole session start with `missing field \`content\``.
pub fn history_loads(bytes: &[u8]) -> bool {
    crate::backends::state::history::decode::<wire::Message>(bytes).is_ok()
}

fn project_history(history: &[wire::Message]) -> Vec<crate::types::TranscriptEntry> {
    use crate::types::{TranscriptEntry, TranscriptRole, TranscriptToolCall};
    use wire::{Block, Role};

    let mut out: Vec<TranscriptEntry> = Vec::with_capacity(history.len());
    // Index of tool_use_id → (entry index, tool-call index within that
    // entry) so a later tool_result message can fill the call it answers.
    // Matching by id is the load-bearing Anthropic difference (Gemini
    // matches by name).
    let mut call_index: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();

    for msg in history {
        let role = match msg.role {
            Role::User => TranscriptRole::User,
            Role::Assistant => TranscriptRole::Assistant,
        };
        let mut buf = String::new();
        let mut calls_this_turn: Vec<(String, TranscriptToolCall)> = Vec::new();

        for block in &msg.content {
            match block {
                Block::Text { text } => buf.push_str(text),
                Block::Thinking { thinking, .. } => buf.push_str(thinking),
                Block::ToolUse { id, name, input } => {
                    calls_this_turn.push((
                        id.clone(),
                        TranscriptToolCall {
                            name: name.clone(),
                            args: input.clone(),
                            result: None,
                            error: None,
                        },
                    ));
                }
                Block::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    if let Some(&(ei, ci)) = call_index.get(tool_use_id) {
                        if let Some(call) = out.get_mut(ei).and_then(|e| e.tool_calls.get_mut(ci)) {
                            if is_error.unwrap_or(false) {
                                call.error = Some(content.to_string());
                            } else {
                                call.result = Some(content.clone());
                            }
                        }
                    }
                }
                Block::Image { .. } => {}
                Block::Other => {}
            }
        }

        if !buf.is_empty() || !calls_this_turn.is_empty() {
            let entry_idx = out.len();
            let tool_calls: Vec<TranscriptToolCall> = calls_this_turn
                .into_iter()
                .enumerate()
                .map(|(ci, (id, call))| {
                    call_index.insert(id, (entry_idx, ci));
                    call
                })
                .collect();
            out.push(TranscriptEntry {
                role,
                text: buf,
                tool_calls,
            });
        }
    }
    out
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Connection for AnthropicConnection {
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
        // Per-turn thinking override (difficulty router) for THIS turn only —
        // the template is untouched. `None` is a no-op.
        if let Some(level) = *self.thinking_override.lock() {
            deps.config.thinking = Some(level);
        }
        // Per-turn MODEL override (difficulty router, #7) — same discipline:
        // overrides the cloned per-turn model for THIS turn only; the template
        // keeps the session model. Safe because the proxy routes every model to
        // the same endpoint (model is a request field), and the wiring only
        // ever passes a same-backend, ceiling-clamped id. `None` is a no-op.
        if let Some(model) = self.model_override.lock().clone() {
            deps.config.model = model;
        }
        crate::runtime::spawn(async move {
            if let Err(e) = run_turn(deps, user, content).await {
                warn!(error = %e, "anthropic turn failed");
            }
        });
        Ok(())
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        self.send(Content::text(content)).await
    }

    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        // The Anthropic backend dispatches tools inline inside the loop.
        Ok(())
    }

    fn subscribe_steps(&self) -> StepStream {
        // A turn-failure Step (HTTP non-200, SSE decode error, in-stream
        // `error` event) is emitted by `loop.rs::emit_error` as a
        // System-sourced, Error-status Step. The shared `conversation.rs`
        // only surfaces an error to `chat()`/`text()` when the *stream
        // item itself* is `Err` (its `PollDecision::Error`) — a successful
        // Step with `status: Error` is otherwise swallowed (empty content →
        // no chunk → silent `Ok("")`). So the shared stream translates the
        // error Step into a stream `Err` carrying the real message. Refusal/
        // safety terminal Steps are Model-sourced and pass through untouched.
        crate::backends::subscribe_step_stream(self.state.steps.subscribe(), "anthropic")
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
    use crate::filesystem::NativeFilesystem;
    use crate::tools::ToolRunner;

    /// Regression (telemetry #37): a Gemini→Claude model switch crashed session
    /// start with `start_anthropic: set_history_bytes: missing field \`content\``.
    /// The browser persists ONE history file across backends; the session-start
    /// gate used the LENIENT `decode_transcript_bytes().is_ok()`, which returns
    /// `Ok(empty)` for a Gemini-format blob (`decode_lenient` skips the
    /// per-entry failures), so the incompatible history slipped through and the
    /// STRICT `set_history_bytes` then crashed. `history_loads` matches the
    /// strict restore, so the gate now rejects foreign-format bytes and the
    /// session starts fresh on the new backend instead of failing outright.
    #[test]
    fn history_loads_gates_foreign_backend_bytes() {
        // Gemini wire shape: `parts`, no `content`.
        let gemini_blob = br#"[{"role":"user","parts":[{"text":"hi"}]}]"#;
        // The LENIENT transcript decoder ACCEPTS it (yields an empty transcript)
        // — exactly why the old `.is_ok()` gate was insufficient.
        assert!(decode_transcript_bytes(gemini_blob).is_ok());
        // The STRICT gate (identical to what `set_history_bytes` runs) REJECTS it.
        assert!(!history_loads(gemini_blob));

        // A real Anthropic history loads; empty bytes are a valid fresh start.
        let history = vec![wire::Message {
            role: wire::Role::User,
            content: vec![wire::Block::Text { text: "hi".into() }],
        }];
        let bytes = crate::backends::state::history::encode(&history).unwrap();
        assert!(history_loads(&bytes));
        assert!(history_loads(b""));
    }
    use crate::types::StepStatus;
    use futures_util::stream::StreamExt;

    /// Parity guard: every builtin tool declared to Anthropic must carry a
    /// single-`type` JSON schema (no nullable unions / `additionalProperties`
    /// / `$ref` / `oneOf` / etc). This is the same class of guard the
    /// Gemini backend has (`builtin_tool_schemas_have_no_union_types`) —
    /// Anthropic ALSO rejects union-type tool schemas, so the same lint
    /// applies to the declarations this backend builds.
    fn assert_single_type(v: &serde_json::Value, tool: &str, path: &str) {
        match v {
            serde_json::Value::Object(map) => {
                if let Some(t) = map.get("type") {
                    assert!(
                        !t.is_array(),
                        "tool `{tool}` schema at `{path}.type` = {t} is an array union",
                    );
                }
                for (k, val) in map {
                    assert_single_type(val, tool, &format!("{path}.{k}"));
                }
            }
            serde_json::Value::Array(arr) => {
                for (i, val) in arr.iter().enumerate() {
                    assert_single_type(val, tool, &format!("{path}[{i}]"));
                }
            }
            _ => {}
        }
    }

    #[test]
    fn anthropic_tool_declarations_have_single_type_schemas() {
        let runner = ToolRunner::new();
        let deps = BuiltinDeps {
            chat_client: None,
            chat_model: String::new(),
            image_client: None,
            image_model: String::new(),
            fs: Some(Arc::new(NativeFilesystem::new()) as crate::filesystem::SharedFilesystem),
            hooks: None,
        };
        register_builtins(&runner, &CapabilitiesConfig::unrestricted(), &deps);
        let decls = build_tool_declarations(&runner);
        assert!(!decls.is_empty(), "expected builtins registered");
        for d in &decls {
            assert_single_type(&d.input_schema, &d.name, "input_schema");
        }
    }

    /// `start_subagent` / `generate_image` are Gemini-client-coupled and
    /// must NOT register on this backend (both client slots are None).
    #[test]
    fn gemini_client_coupled_tools_do_not_register() {
        let runner = ToolRunner::new();
        let deps = BuiltinDeps {
            chat_client: None,
            chat_model: String::new(),
            image_client: None,
            image_model: String::new(),
            fs: Some(Arc::new(NativeFilesystem::new()) as crate::filesystem::SharedFilesystem),
            hooks: None,
        };
        let registered = register_builtins(&runner, &CapabilitiesConfig::unrestricted(), &deps);
        assert!(
            !registered.iter().any(|n| n == "start_subagent"),
            "start_subagent must not register without a chat client"
        );
        assert!(
            !registered.iter().any(|n| n == "generate_image"),
            "generate_image must not register without an image client"
        );
        // But fs + portable builtins DO register.
        assert!(registered.iter().any(|n| n == "finish"));
        assert!(registered.iter().any(|n| n == "view_file"));
    }

    /// Empty api_key fails fast (mirrors the Gemini strategy). `Arc<dyn
    /// Connection>` isn't `Debug`, so match on the Result rather than
    /// `unwrap_err()`.
    #[tokio::test]
    async fn empty_api_key_errors() {
        let strategy = AnthropicConnectionStrategy::new(AnthropicBackendConfig::new("  "));
        match strategy.connect().await {
            Ok(_) => panic!("expected empty api_key to error"),
            Err(e) => assert!(e.to_string().contains("api_key is empty")),
        }
    }

    /// Transcript projection matches tool calls to results by id.
    /// REGRESSION: a single malformed entry must not blank the whole restored
    /// Anthropic transcript — decode is per-entry lenient (skip failures).
    #[test]
    fn decode_skips_malformed_entry() {
        let raw = br#"[{"role":"user","content":[{"type":"text","text":"q"}]},{"oops":1},{"role":"assistant","content":[{"type":"text","text":"a"}]}]"#;
        let entries = decode_transcript_bytes(raw).expect("must not error");
        assert_eq!(entries.len(), 2, "user + assistant kept, garbage skipped");
        assert_eq!(entries[0].text, "q");
        assert_eq!(entries[1].text, "a");
    }

    #[test]
    fn transcript_matches_tool_calls_by_id() {
        use wire::{Block, Message, Role};
        let history = vec![
            Message::user_text("read main.rs"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::Text {
                        text: "Reading.".into(),
                    },
                    Block::ToolUse {
                        id: "toolu_1".into(),
                        name: "view_file".into(),
                        input: serde_json::json!({"path": "main.rs"}),
                    },
                ],
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: serde_json::json!({"contents": "fn main() {}"}),
                    is_error: None,
                }],
            },
            Message::assistant_text("Done."),
        ];
        let entries = project_history(&history);
        // Shared cross-provider contract: one assistant entry, one correlated
        // result, no error. By-id correlation is the Anthropic specific.
        let result = crate::backends::state::transcript_contract::assert_single_call_result(
            &entries,
            "view_file",
        );
        assert_eq!(result["contents"], "fn main() {}");
    }

    /// Cross-provider contract (error half): an Anthropic `tool_result` with
    /// `is_error: true` must surface as the typed `error`, not a `result` —
    /// the same invariant the Gemini/OpenAI error tests assert.
    #[test]
    fn transcript_lifts_is_error_into_error_field() {
        use wire::{Block, Message, Role};
        let history = vec![
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "toolu_1".into(),
                    name: "view_file".into(),
                    input: serde_json::json!({"path": "missing"}),
                }],
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "toolu_1".into(),
                    content: serde_json::json!("no such file"),
                    is_error: Some(true),
                }],
            },
        ];
        let entries = project_history(&history);
        let err = crate::backends::state::transcript_contract::assert_single_call_error(
            &entries,
            "view_file",
        );
        assert_eq!(err, "\"no such file\"");
    }

    /// Build a bare `AnthropicConnection` whose loop state we can poke
    /// directly, with no client wiring needed.
    fn test_connection() -> Arc<AnthropicConnection> {
        let (steps_tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = Arc::new(LoopState::new(steps_tx));
        let client: SharedClient = Arc::new(AnthropicClient::new("k").unwrap());
        let config = LoopConfig::from_system(
            DEFAULT_MODEL.to_string(),
            None,
            None,
            None,
            None,
            Vec::new(),
            None,
        )
        .unwrap();
        Arc::new(AnthropicConnection {
            deps_template: TurnDeps {
                client,
                config,
                state: state.clone(),
                tool_runner: None,
                hook_runner: None,
                session_ctx: None,
            },
            state,
            conversation_id: "test".into(),
            thinking_override: parking_lot::Mutex::new(None),
            model_override: parking_lot::Mutex::new(None),
        })
    }

    /// REGRESSION: a turn-failure Step (System-sourced, Error-status, with an
    /// error message — exactly what `loop.rs::emit_error` broadcasts on a
    /// non-200 / SSE decode failure / in-stream `error` event) MUST surface as
    /// a stream `Err`, not a silently-swallowed success. Before the fix,
    /// `subscribe_steps` mapped every successful Step to `Ok(step)`, so the
    /// error Step (empty content → no chunk) made `chat()`/`text()` return an
    /// empty `Ok("")`. Now it propagates the real message.
    #[tokio::test]
    async fn error_step_surfaces_as_stream_error() {
        use crate::error::Error;

        let conn = test_connection();
        let mut stream = conn.subscribe_steps();

        // Mirror `loop.rs::emit_error`: System + Error + message.
        conn.state
            .steps
            .send(Step::turn_error(0, "anthropic HTTP 500: boom"))
            .expect("subscriber is live");

        let item = stream.next().await.expect("a stream item");
        match item {
            Ok(step) => panic!("error Step leaked as Ok: {step:?}"),
            Err(Error::Other(msg)) => assert!(
                msg.contains("anthropic HTTP 500: boom"),
                "expected the real error message, got: {msg}"
            ),
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    /// A normal Model-sourced terminal Step (status Done) passes through as a
    /// success — only System/Error turn-failures convert to a stream `Err`, so
    /// the fix doesn't poison ordinary completions.
    #[tokio::test]
    async fn done_step_passes_through_as_ok() {
        let conn = test_connection();
        let mut stream = conn.subscribe_steps();

        conn.state
            .steps
            .send(Step::turn_complete(
                "traj",
                0,
                StepStatus::Done,
                "all good",
                "",
                false,
                None,
                None,
            ))
            .expect("subscriber is live");

        let item = stream.next().await.expect("a stream item");
        let step = item.expect("Done step must pass through as Ok");
        assert_eq!(step.content, "all good");
        assert_eq!(step.status, StepStatus::Done);
    }

    /// End-to-end through the shared `Conversation`: a turn whose HTTP request
    /// can't even connect (unroutable base URL) drives `loop.rs::emit_error`,
    /// and `chat().text()` returns `Err` carrying the failure — NOT an empty
    /// `Ok("")`. This is the user-visible symptom the task reports.
    #[tokio::test]
    async fn chat_text_returns_err_on_connect_failure() {
        use crate::conversation::Conversation;

        // Port 1 is privileged + unbound → connection refused fast.
        let base = url::Url::parse("http://127.0.0.1:1/").unwrap();
        let cfg = AnthropicBackendConfig::new("k").with_base_url(base);
        let conn = AnthropicConnectionStrategy::new(cfg)
            .connect()
            .await
            .expect("connect (no network yet)");
        let conv = Conversation::new(conn);
        let resp = conv.chat("hi").await.expect("send dispatches");
        match resp.text().await {
            Ok(t) => panic!("expected an error, got empty success: {t:?}"),
            Err(e) => {
                let s = e.to_string();
                assert!(
                    s.contains("anthropic POST") || s.contains("anthropic HTTP"),
                    "expected the surfaced turn error, got: {s}"
                );
            }
        }
    }
}
