//! Rust-native OpenAI (Chat Completions API) agent backend.
//!
//! A `ConnectionStrategy`/`Connection` pair behind the same Layer-3 seam the
//! Gemini + Anthropic backends implement. Hits `POST /v1/chat/completions`
//! directly (BYOK via `OpenAiBackendConfig::new`); a `with_base_url` override
//! routes through the `$LH` credit proxy, which already forwards
//! `/v1/chat/completions` to OpenAI with the platform key.
//!
//! Mirrors `backends/anthropic/mod.rs`: `send` spawns the turn loop, a
//! broadcast channel feeds `subscribe_steps`, idle is tracked on an
//! `AtomicBool`, and tool calls dispatch inline through the registered hooks +
//! policies + `ToolRunner`. Only the wire shapes differ (see `wire.rs` /
//! `loop.rs`).
//!
//! Built-in tools are registered by REUSING the Gemini backend's
//! `register_builtins` with both client slots set to `None` — every portable +
//! filesystem builtin registers, while the two Gemini-client-coupled tools
//! (`start_subagent`, `generate_image`) don't register on this backend.

pub mod api;
pub mod compaction;
pub mod wire;
#[path = "loop.rs"]
mod r#loop;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use async_trait::async_trait;
use tokio::sync::broadcast;
use tracing::warn;

use crate::backends::openai::api::{OpenAiClient, SharedClient};
use crate::backends::openai::r#loop::{
    run_turn, to_wire_user_content, tool_def_from, LoopConfig, LoopState, TurnDeps,
};
use crate::backends::openai::wire::ToolDef;
// Built-in tool registration is reused verbatim from the Gemini backend —
// the tools consume neutral `Tool::input_schema()` JSON, which OpenAI's
// `function.parameters` takes raw.
use crate::backends::gemini::tools::{register_builtins, BuiltinDeps};
use crate::connections::{Connection, ConnectionStrategy, StepStream};
use crate::content::Content;
use crate::error::{Error, Result};
use crate::tools::ToolRunner;
use crate::types::{CapabilitiesConfig, Step, SystemInstructions, ToolResult};

pub use wire::{DEFAULT_MODEL, MINI_MODEL, PRO_MODEL};

const STEP_BROADCAST_CAPACITY: usize = 256;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the OpenAI Chat Completions backend.
#[derive(Debug, Clone)]
pub struct OpenAiBackendConfig {
    /// OpenAI API key (BYOK) — or, in credits mode, the proxy auth token.
    pub api_key: String,
    /// Chat model ID. Defaults to [`wire::DEFAULT_MODEL`]. Any string is
    /// accepted — model ids are NOT validated (OpenAI flips them).
    pub model: String,
    /// Optional system instructions (prepended as a `role:"system"` message).
    pub system_instructions: Option<SystemInstructions>,
    /// Optional sampling temperature.
    pub temperature: Option<f32>,
    /// Optional `max_completion_tokens` cap for the response.
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
    /// Per-request auth-token provider. When set, every HTTP request mints a
    /// fresh credential instead of reusing `api_key` (which becomes a
    /// fallback). Required for the credit proxy's signed-token freshness window.
    pub api_key_provider: Option<crate::backends::AuthTokenProvider>,
}

impl OpenAiBackendConfig {
    /// Create a config with the given API key and default model — BYOK,
    /// talks directly to `api.openai.com`.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: DEFAULT_MODEL.to_string(),
            system_instructions: None,
            temperature: None,
            max_tokens: None,
            base_url: None,
            conversation_id: None,
            capabilities: CapabilitiesConfig::default(),
            filesystem: None,
            api_key_provider: None,
        }
    }

    /// Plug in a custom [`Filesystem`] impl for the fs built-ins.
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.filesystem = Some(fs);
        self
    }

    /// Override the chat model ID (e.g. [`MINI_MODEL`], [`PRO_MODEL`], or any
    /// other string — not validated).
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set system instructions.
    pub fn with_system_instructions(mut self, s: impl Into<SystemInstructions>) -> Self {
        self.system_instructions = Some(s.into());
        self
    }

    /// Set the sampling temperature.
    pub fn with_temperature(mut self, t: f32) -> Self {
        self.temperature = Some(t);
        self
    }

    /// Set `max_completion_tokens` for the response.
    pub fn with_max_tokens(mut self, n: u32) -> Self {
        self.max_tokens = Some(n);
        self
    }

    /// Configure which built-in tools are enabled.
    pub fn with_capabilities(mut self, c: CapabilitiesConfig) -> Self {
        self.capabilities = c;
        self
    }

    /// Route requests through an alternate base URL (e.g. the localharness
    /// credit proxy, which forwards `/v1/chat/completions`). In credits mode
    /// `api_key` carries the proxy auth token.
    pub fn with_base_url(mut self, url: url::Url) -> Self {
        self.base_url = Some(url);
        self
    }
}

// =============================================================================
// Strategy
// =============================================================================

/// Injected runners for inline tool dispatch in the OpenAI backend — an alias
/// of the shared [`BackendRunners`](crate::backends::BackendRunners).
pub type OpenAiRunners = crate::backends::BackendRunners;

/// Factory that opens an [`OpenAiConnection`].
pub struct OpenAiConnectionStrategy {
    config: OpenAiBackendConfig,
    runners: OpenAiRunners,
    /// Optional out-slot: `connect()` stashes a clone of the typed
    /// `Arc<OpenAiConnection>` here before upcasting. `Agent::start_openai`
    /// uses this to keep a typed handle for backend-specific APIs.
    typed_capture: Option<Arc<parking_lot::Mutex<Option<Arc<OpenAiConnection>>>>>,
}

impl OpenAiConnectionStrategy {
    /// Create a strategy from a backend config.
    pub fn new(config: OpenAiBackendConfig) -> Self {
        Self {
            config,
            runners: OpenAiRunners::default(),
            typed_capture: None,
        }
    }

    /// Inject the runners the Agent owns (inline tool dispatch).
    pub fn with_runners(mut self, runners: OpenAiRunners) -> Self {
        self.runners = runners;
        self
    }

    /// Provide a slot for `connect()` to write the typed connection into.
    pub fn with_typed_capture(
        mut self,
        slot: Arc<parking_lot::Mutex<Option<Arc<OpenAiConnection>>>>,
    ) -> Self {
        self.typed_capture = Some(slot);
        self
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl ConnectionStrategy for OpenAiConnectionStrategy {
    async fn connect(&self) -> Result<Arc<dyn Connection>> {
        if self.config.api_key.trim().is_empty() {
            return Err(Error::config("OpenAiBackendConfig.api_key is empty"));
        }
        let mut client = OpenAiClient::new(self.config.api_key.clone())?;
        if let Some(base) = &self.config.base_url {
            client = client.with_base_url(base.clone());
        }
        if let Some(provider) = &self.config.api_key_provider {
            client = client.with_key_provider(provider.0.clone());
        }
        let client: SharedClient = Arc::new(client);

        // Auto-register built-in tools per the capabilities config, reusing the
        // Gemini backend's `register_builtins`. Both client slots are None: the
        // fs + portable builtins register; the two Gemini-client-coupled tools
        // (start_subagent / generate_image) don't register on this backend.
        if let Some(runner) = self.runners.tool_runner.as_ref() {
            let fs: Option<crate::filesystem::SharedFilesystem> =
                self.config.filesystem.clone().or_else(default_filesystem);
            let deps = BuiltinDeps {
                chat_client: None,
                chat_model: self.config.model.clone(),
                image_client: None,
                image_model: String::new(),
                fs,
            };
            let registered = register_builtins(runner, &self.config.capabilities, &deps);
            if !registered.is_empty() {
                tracing::debug!(?registered, "registered built-in tools (openai)");
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

        let typed = Arc::new(OpenAiConnection {
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
    runner
        .iter_tools()
        .into_iter()
        .map(|tool| {
            tool_def_from(
                tool.name().to_string(),
                tool.description().to_string(),
                tool.input_schema(),
            )
        })
        .collect()
}

// =============================================================================
// Connection
// =============================================================================

/// A live OpenAI session that implements [`Connection`].
pub struct OpenAiConnection {
    deps_template: TurnDeps,
    state: Arc<LoopState>,
    conversation_id: Arc<str>,
}

impl OpenAiConnection {
    /// Snapshot the current conversation history as opaque bytes. Round-trips
    /// through `set_history_bytes`. The on-disk format (a JSON array of OpenAI
    /// `Message`s) is not part of the public API.
    pub fn history_bytes(&self) -> Result<Vec<u8>> {
        crate::backends::state::history::encode(&self.state.history.lock())
    }

    /// Replace the entire conversation history with one previously returned by
    /// `history_bytes`. Use on connection start to resume a session.
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
    /// [`crate::Agent::clear_history`].
    pub fn clear_history(&self) {
        self.state.history.lock().clear();
        *self.state.last_turn_usage.lock() = None;
        *self.state.last_structured_output.lock() = None;
        self.state
            .next_step_index
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// Project the wire history into a flat transcript suitable for repainting
    /// a UI. Tool-call activity is surfaced as `TranscriptToolCall`s (matched
    /// by `tool_call_id`).
    pub fn transcript(&self) -> Vec<crate::types::TranscriptEntry> {
        let snap = self.state.history.lock().clone();
        project_history(&snap)
    }
}

/// Decode opaque bytes from [`OpenAiConnection::history_bytes`] into a flat
/// transcript without a live connection.
pub fn decode_transcript_bytes(bytes: &[u8]) -> Result<Vec<crate::types::TranscriptEntry>> {
    let history: Vec<wire::Message> = crate::backends::state::history::decode_lenient(bytes)?;
    Ok(project_history(&history))
}

fn project_history(history: &[wire::Message]) -> Vec<crate::types::TranscriptEntry> {
    use crate::types::{TranscriptEntry, TranscriptRole, TranscriptToolCall};
    use wire::Role;

    let mut out: Vec<TranscriptEntry> = Vec::with_capacity(history.len());
    // Index of tool_call_id → (entry index, tool-call index) so a later
    // `tool`-role message can fill the call it answers (OpenAI correlates by
    // id, like Anthropic).
    let mut call_index: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();

    for msg in history {
        // A `tool`-role message is a tool result — fill the matching call and
        // never paint as a standalone transcript turn.
        if matches!(msg.role, Role::Tool) {
            if let Some(id) = &msg.tool_call_id {
                if let Some(&(ei, ci)) = call_index.get(id) {
                    if let Some(call) =
                        out.get_mut(ei).and_then(|e| e.tool_calls.get_mut(ci))
                    {
                        let body = msg.content.clone().unwrap_or_default();
                        // Try to surface a structured result; fall back to text.
                        match serde_json::from_str::<serde_json::Value>(&body) {
                            Ok(v) => call.result = Some(v),
                            Err(_) => call.result = Some(serde_json::Value::String(body)),
                        }
                    }
                }
            }
            continue;
        }

        let role = match msg.role {
            Role::Assistant => TranscriptRole::Assistant,
            // System turns are synthetic preamble; paint as user-side context.
            Role::User | Role::System => TranscriptRole::User,
            Role::Tool => unreachable!("handled above"),
        };

        let text = msg.content.clone().unwrap_or_default();
        let calls_this_turn: Vec<(String, TranscriptToolCall)> = msg
            .tool_calls
            .iter()
            .map(|c| {
                let args = serde_json::from_str(&c.function.arguments)
                    .unwrap_or(serde_json::Value::Null);
                (
                    c.id.clone(),
                    TranscriptToolCall {
                        name: c.function.name.clone(),
                        args,
                        result: None,
                        error: None,
                    },
                )
            })
            .collect();

        if !text.is_empty() || !calls_this_turn.is_empty() {
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
                text,
                tool_calls,
            });
        }
    }
    out
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Connection for OpenAiConnection {
    fn is_idle(&self) -> bool {
        self.state.idle.load(Ordering::Acquire)
    }

    fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    async fn send(&self, content: Content) -> Result<()> {
        // Clone is cheap; the original rides along so pre-turn hooks inspect
        // the SDK-level prompt, not wire JSON.
        let user = to_wire_user_content(content.clone())?;
        let deps = self.deps_template.clone();
        crate::runtime::spawn(async move {
            if let Err(e) = run_turn(deps, user, content).await {
                warn!(error = %e, "openai turn failed");
            }
        });
        Ok(())
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        self.send(Content::text(content)).await
    }

    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        // The OpenAI backend dispatches tools inline inside the loop.
        Ok(())
    }

    fn subscribe_steps(&self) -> StepStream {
        // A turn-failure Step (HTTP non-200, SSE decode error) is emitted by
        // `loop.rs::emit_error` as a System-sourced, Error-status Step; the
        // shared stream translates it into a stream `Err` carrying the real
        // message (otherwise it would be swallowed as an empty success).
        crate::backends::subscribe_step_stream(self.state.steps.subscribe(), "openai")
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
    use crate::types::StepStatus;
    use futures_util::stream::StreamExt;

    /// Parity guard: every builtin tool declared to OpenAI must carry a
    /// single-`type` JSON schema (no nullable unions / `additionalProperties` /
    /// `$ref` / `oneOf` / etc). OpenAI's strict-tools path rejects union-type
    /// schemas, so the same lint the Gemini/Anthropic backends carry applies.
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
    fn openai_tool_declarations_have_single_type_schemas() {
        let runner = ToolRunner::new();
        let deps = BuiltinDeps {
            chat_client: None,
            chat_model: String::new(),
            image_client: None,
            image_model: String::new(),
            fs: Some(Arc::new(NativeFilesystem::new()) as crate::filesystem::SharedFilesystem),
        };
        register_builtins(&runner, &CapabilitiesConfig::unrestricted(), &deps);
        let decls = build_tool_declarations(&runner);
        assert!(!decls.is_empty(), "expected builtins registered");
        for d in &decls {
            // The schema lives under function.parameters for OpenAI.
            assert_single_type(&d.function.parameters, &d.function.name, "parameters");
        }
    }

    /// `start_subagent` / `generate_image` are Gemini-client-coupled and must
    /// NOT register on this backend (both client slots are None).
    #[test]
    fn gemini_client_coupled_tools_do_not_register() {
        let runner = ToolRunner::new();
        let deps = BuiltinDeps {
            chat_client: None,
            chat_model: String::new(),
            image_client: None,
            image_model: String::new(),
            fs: Some(Arc::new(NativeFilesystem::new()) as crate::filesystem::SharedFilesystem),
        };
        let registered = register_builtins(&runner, &CapabilitiesConfig::unrestricted(), &deps);
        assert!(!registered.iter().any(|n| n == "start_subagent"));
        assert!(!registered.iter().any(|n| n == "generate_image"));
        // But fs + portable builtins DO register.
        assert!(registered.iter().any(|n| n == "finish"));
        assert!(registered.iter().any(|n| n == "view_file"));
    }

    /// Empty api_key fails fast (mirrors the Gemini/Anthropic strategies).
    #[tokio::test]
    async fn empty_api_key_errors() {
        let strategy = OpenAiConnectionStrategy::new(OpenAiBackendConfig::new("  "));
        match strategy.connect().await {
            Ok(_) => panic!("expected empty api_key to error"),
            Err(e) => assert!(e.to_string().contains("api_key is empty")),
        }
    }

    /// REGRESSION: a single malformed entry must not blank the whole restored
    /// OpenAI transcript — decode is per-entry lenient (skip failures).
    #[test]
    fn decode_skips_malformed_entry() {
        let raw = br#"[{"role":"user","content":"q"},{"oops":1},{"role":"assistant","content":"a"}]"#;
        let entries = decode_transcript_bytes(raw).expect("must not error");
        assert_eq!(entries.len(), 2, "user + assistant kept, garbage skipped");
        assert_eq!(entries[0].text, "q");
        assert_eq!(entries[1].text, "a");
    }

    /// Transcript projection matches tool calls to their `tool`-role results by
    /// `tool_call_id`.
    #[test]
    fn transcript_matches_tool_calls_by_id() {
        use wire::{FunctionCall, Message, Role, ToolCall};
        let history = vec![
            Message::user_text("read main.rs"),
            Message {
                role: Role::Assistant,
                content: Some("Reading.".into()),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: "view_file".into(),
                        arguments: r#"{"path":"main.rs"}"#.into(),
                    },
                }],
                tool_call_id: None,
            },
            Message::tool_result("call_1", r#"{"contents":"fn main() {}"}"#),
            Message::assistant_text("Done."),
        ];
        let entries = project_history(&history);
        // Shared cross-provider contract: one assistant entry, one correlated
        // result, no error. By-id correlation is the OpenAI specific.
        let result = crate::backends::state::transcript_contract::assert_single_call_result(
            &entries,
            "view_file",
        );
        assert_eq!(result["contents"], "fn main() {}");
    }

    /// Build a bare `OpenAiConnection` whose loop state we can poke directly,
    /// with no client wiring needed.
    fn test_connection() -> Arc<OpenAiConnection> {
        let (steps_tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = Arc::new(LoopState::new(steps_tx));
        let client: SharedClient = Arc::new(OpenAiClient::new("k").unwrap());
        let config =
            LoopConfig::from_system(DEFAULT_MODEL.to_string(), None, None, None, Vec::new(), None)
                .unwrap();
        Arc::new(OpenAiConnection {
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
        })
    }

    /// REGRESSION: a turn-failure Step (System-sourced, Error-status) MUST
    /// surface as a stream `Err`, not a silently-swallowed success.
    #[tokio::test]
    async fn error_step_surfaces_as_stream_error() {
        use crate::error::Error;

        let conn = test_connection();
        let mut stream = conn.subscribe_steps();
        conn.state
            .steps
            .send(Step::turn_error(0, "openai HTTP 500: boom"))
            .expect("subscriber is live");

        let item = stream.next().await.expect("a stream item");
        match item {
            Ok(step) => panic!("error Step leaked as Ok: {step:?}"),
            Err(Error::Other(msg)) => assert!(
                msg.contains("openai HTTP 500: boom"),
                "expected the real error message, got: {msg}"
            ),
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    /// End-to-end through the shared `Conversation`: a turn whose HTTP request
    /// can't connect drives `loop.rs::emit_error`, and `chat().text()` returns
    /// `Err` carrying the failure — NOT an empty `Ok("")`.
    #[tokio::test]
    async fn chat_text_returns_err_on_connect_failure() {
        use crate::conversation::Conversation;

        let base = url::Url::parse("http://127.0.0.1:1/").unwrap();
        let cfg = OpenAiBackendConfig::new("k").with_base_url(base);
        let conn = OpenAiConnectionStrategy::new(cfg)
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
                    s.contains("openai POST") || s.contains("openai HTTP"),
                    "expected the surfaced turn error, got: {s}"
                );
            }
        }
    }

    /// The wasm-relevant trait bounds compile: `OpenAiConnection` is a
    /// `Connection` and the strategy is a `ConnectionStrategy`. On native both
    /// must additionally be `Send + Sync` (the `?Send` async_trait variant only
    /// kicks in on wasm32). This static assertion fails to compile if a future
    /// edit drops a bound the multi-threaded runtime requires.
    #[test]
    fn connection_trait_bounds_hold() {
        fn assert_connection<T: Connection>() {}
        fn assert_strategy<T: ConnectionStrategy>() {}
        assert_connection::<OpenAiConnection>();
        assert_strategy::<OpenAiConnectionStrategy>();
        #[cfg(not(target_arch = "wasm32"))]
        {
            fn assert_send_sync<T: Send + Sync>() {}
            assert_send_sync::<OpenAiConnection>();
            assert_send_sync::<OpenAiConnectionStrategy>();
        }
    }

    /// A clear sanity check on the terminal step plumbing.
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
    }
}
