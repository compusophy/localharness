//! Local in-browser model backend — the [`Connection`] / [`ConnectionStrategy`]
//! seam for Gemma 3 270M running fully in the tab via Burn's wgpu/WebGPU
//! backend. No proxy, no `$LH`, no API key: weights live in OPFS and inference
//! happens on-device.
//!
//! Mirrors `backends/anthropic/mod.rs` 1:1 in structure — a
//! [`LocalConnectionStrategy`] factory, a [`LocalConnection`] holding
//! `Arc<LoopState>`, a broadcast step channel, and the `clear_history` /
//! `compact` / `history_bytes` / `transcript` session surface (on the
//! [`Connection`] trait). The differences from the network backends:
//!
//! * **No HTTP.** There is no client and no network streaming loop. A turn is a
//!   bounded generate → parse-tool → dispatch → feed-result → re-loop driven by
//!   [`generate_streamed`], emitting per-token text-delta [`Step`]s while the
//!   GPU decodes (the transcript paints live, same shape as the network
//!   backends) and a terminal text [`Step`] (plus per-call ToolCall /
//!   ToolResult steps when a tool is invoked).
//! * **Best-effort tool calling.** The built-in tools ARE registered (reusing
//!   the Gemini backend's `register_builtins`, both LLM-client slots `None`, so
//!   the 8 fs builtins run over OPFS). Each round's generated text is scanned
//!   for a philschmid `tool_code` fence ([`tool_parse`](super::tool_parse));
//!   when one parses, the call dispatches inline through the registered hooks +
//!   `ToolRunner`. Gemma 3 270M is a tiny base model, so most turns have no
//!   fence and fall straight through to plain text — the loop degrades cleanly.
//! * **In-memory history.** A `Vec<(role, text)>` — no wire format. `compact`
//!   is a no-op (nothing to summarise over the network); `history_bytes` is a
//!   plain JSON snapshot that round-trips through `set_history_bytes`.
//! * **Weights may be absent.** The model is only present after the user opts in
//!   to the ~570 MB download. When the weights/tokenizer aren't in OPFS yet,
//!   `connect()` still succeeds (so the session starts) but `send()` emits a
//!   clear "model not downloaded" error Step.
//!
//! Gated on `feature = "local"`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{broadcast, Notify};

// Reuse the Gemini backend's built-in tool registration so the 8 fs builtins
// (and the portable ones) run over the supplied filesystem (OPFS on wasm),
// exactly as the Anthropic backend does. `FINISH_TOOL_NAME` lets the local
// tool loop honour an explicit `finish()` call.
use crate::builtins::{register_builtins, BuiltinDeps, FINISH_TOOL_NAME};
use crate::connections::{Connection, ConnectionStrategy, StepStream};
use crate::content::{Content, Part};
use crate::error::{Error, Result};
use crate::hooks::{HookRunner, SessionContext};
use crate::tools::ToolRunner;
use crate::types::{
    CapabilitiesConfig, Step, StepStatus, SystemInstructions, ToolCall, ToolResult,
    TranscriptEntry, TranscriptRole,
};

use super::gemma::{GemmaConfig, GemmaModel};
use super::generate::generate_streamed;
use super::tokenizer::{self, GemmaTokenizer};
use super::tool_parse::parse_tool_code;
use super::weights;
use super::LocalBackend;

const STEP_BROADCAST_CAPACITY: usize = 256;

/// Default OPFS path for the downloaded Gemma weights (`model.safetensors`).
/// The download handler writes here; `connect()` reads it back. Kept `pub`
/// so the browser-app download flow references the same path.
pub const WEIGHTS_PATH: &str = ".lh_local_model.safetensors";

/// Default OPFS path for the downloaded Gemma tokenizer (`tokenizer.json`).
pub const TOKENIZER_PATH: &str = ".lh_local_tokenizer.json";

/// How many new tokens a single turn generates (greedy, KV-cached decode —
/// keep modest so an in-tab turn stays responsive).
const MAX_NEW_TOKENS: usize = 256;

/// Maximum model↔tool round-trips per user turn. The local model is a tiny
/// fast-path; a low cap keeps a runaway tool loop cheap (each round re-prefills
/// the re-rendered prompt). Best-effort tool calling means most turns fall
/// through on round 1 anyway.
const MAX_TOOL_ROUNDS: u32 = 5;

// =============================================================================
// Configuration
// =============================================================================

/// Configuration for the local (in-browser Gemma) backend.
#[derive(Clone)]
#[non_exhaustive]
pub struct LocalBackendConfig {
    /// Model id label (e.g. `"gemma-3-270m"`). Informational only — there is
    /// one local model today.
    pub model: String,
    /// Optional system instructions, flattened into the prompt preamble.
    pub system_instructions: Option<SystemInstructions>,
    /// Capability/built-in-tool selection (accepted for parity; the local
    /// backend does not dispatch tools).
    pub capabilities: CapabilitiesConfig,
    /// Pre-existing conversation id to resume from. `None` → a fresh UUID.
    pub conversation_id: Option<String>,
    /// Filesystem the weights/tokenizer are read from (OPFS in the browser).
    /// `None` → the model can't be loaded and `send()` reports it.
    pub filesystem: Option<crate::filesystem::SharedFilesystem>,
    /// Override the OPFS weights path (defaults to [`WEIGHTS_PATH`]).
    pub weights_path: String,
    /// Override the OPFS tokenizer path (defaults to [`TOKENIZER_PATH`]).
    pub tokenizer_path: String,
}

impl LocalBackendConfig {
    /// Create a config for the one local model with default OPFS paths.
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            system_instructions: None,
            capabilities: CapabilitiesConfig::default(),
            conversation_id: None,
            filesystem: None,
            weights_path: WEIGHTS_PATH.to_string(),
            tokenizer_path: TOKENIZER_PATH.to_string(),
        }
    }

    /// Plug in the [`Filesystem`] the weights/tokenizer are read from.
    ///
    /// [`Filesystem`]: crate::filesystem::Filesystem
    pub fn with_filesystem(mut self, fs: crate::filesystem::SharedFilesystem) -> Self {
        self.filesystem = Some(fs);
        self
    }

    /// Set the model id label.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set system instructions.
    pub fn with_system_instructions(mut self, s: impl Into<SystemInstructions>) -> Self {
        self.system_instructions = Some(s.into());
        self
    }

    /// Configure which built-in tools are enabled (parity only).
    pub fn with_capabilities(mut self, c: CapabilitiesConfig) -> Self {
        self.capabilities = c;
        self
    }
}

// =============================================================================
// Loaded engine
// =============================================================================

/// The loaded model + tokenizer + device. Constructed once in `connect()` when
/// the weights are present in OPFS.
struct Engine {
    model: GemmaModel<LocalBackend>,
    tokenizer: GemmaTokenizer,
    device: burn::backend::wgpu::WgpuDevice,
}

impl Engine {
    /// Run one greedy generation for `prompt`, streaming decoded text slices
    /// through `on_delta` as they stabilise (returning `false` cancels — the
    /// send loop feeds the turn's cancel flag through so Stop takes effect
    /// mid-generation, not only between tool rounds). Async because the
    /// per-token GPU read-back ([`generate_streamed`]) must use the
    /// non-blocking path on wasm.
    async fn run(&self, prompt: &str, on_delta: impl FnMut(&str) -> bool) -> String {
        generate_streamed(
            &self.model,
            &self.tokenizer,
            prompt,
            MAX_NEW_TOKENS,
            &self.device,
            on_delta,
        )
        .await
    }
}

/// Try to load the engine from the OPFS bytes. Returns `Ok(None)` (NOT an error)
/// when the files simply aren't downloaded yet — that is the expected first-run
/// state, surfaced to the user as a clear `send()` message rather than a failed
/// session start.
async fn load_engine(config: &LocalBackendConfig) -> Result<Option<Engine>> {
    let Some(fs) = config.filesystem.as_ref() else {
        return Ok(None);
    };
    // Weights absent → not downloaded yet. Treat a read error as "absent".
    let weights = match fs.read(&config.weights_path).await {
        Ok(b) if !b.is_empty() => b,
        _ => return Ok(None),
    };
    let tok_bytes = match fs.read(&config.tokenizer_path).await {
        Ok(b) if !b.is_empty() => b,
        _ => return Ok(None),
    };

    let device = burn::backend::wgpu::WgpuDevice::default();
    // wasm: the browser's adapter/device request is async-only — cubecl's lazy
    // blocking init panics ("call init() before load()"). Initialize the wgpu
    // runtime explicitly BEFORE the first tensor op. Native inits lazily fine.
    #[cfg(target_arch = "wasm32")]
    burn::backend::wgpu::init_setup_async::<burn::backend::wgpu::graphics::WebGpu>(
        &device,
        Default::default(),
    )
    .await;
    let cfg = GemmaConfig::gemma_3_270m();
    let model = GemmaModel::<LocalBackend>::init(cfg, &device);
    let model = weights::load_gemma(model, &weights, &device)
        .map_err(|e| Error::other(format!("local model load: {e}")))?;
    let tokenizer = tokenizer::load(&tok_bytes)
        .map_err(|e| Error::other(format!("local tokenizer load: {e}")))?;
    Ok(Some(Engine {
        model,
        tokenizer,
        device,
    }))
}

// =============================================================================
// Loop state + history
// =============================================================================

/// A single in-memory history turn.
#[derive(Clone, Serialize, Deserialize)]
struct Turn {
    role: TurnRole,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum TurnRole {
    User,
    Model,
}

/// Per-connection mutable state. History is a plain `Vec<Turn>` (no wire
/// format) — the local analogue of the Anthropic backend's `Vec<Message>`.
struct LoopState {
    history: Mutex<Vec<Turn>>,
    idle: AtomicBool,
    idle_notify: Notify,
    cancel: AtomicBool,
    steps: broadcast::Sender<Step>,
    next_step_index: AtomicU32,
    /// The optional system preamble, prepended to the rendered prompt.
    system: Option<String>,
}

impl LoopState {
    fn new(steps: broadcast::Sender<Step>, system: Option<String>) -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            idle: AtomicBool::new(true),
            idle_notify: Notify::new(),
            cancel: AtomicBool::new(false),
            steps,
            next_step_index: AtomicU32::new(0),
            system,
        }
    }

    fn alloc_step_index(&self) -> u32 {
        self.next_step_index.fetch_add(1, Ordering::Relaxed)
    }

    fn emit(&self, step: Step) {
        let _ = self.steps.send(step);
    }

    /// Render the running history (plus the optional system preamble) into a
    /// single flat prompt string the base model continues. When a `ToolRunner`
    /// is supplied and has tools, also prepends a short tool-use instruction +
    /// the list of available tool names/descriptions so a (tool-capable) model
    /// can emit a `tool_code` fence. With `None` (or no tools) it degrades to a
    /// plain prompt. Best-effort: the base 270M model rarely emits a clean
    /// fence, so the caller still gracefully handles plain text.
    fn render_prompt_with_tools(&self, runner: Option<&ToolRunner>) -> String {
        let mut buf = String::new();
        if let Some(sys) = &self.system {
            if !sys.is_empty() {
                buf.push_str(sys);
                buf.push_str("\n\n");
            }
        }
        if let Some(r) = runner {
            let tools = r.iter_tools();
            if !tools.is_empty() {
                buf.push_str(
                    "You can call functions. To call one, output EXACTLY a fenced block:\n\
                     ```tool_code\nname(arg=value)\n```\n\
                     Available functions:\n",
                );
                for t in &tools {
                    buf.push_str("- ");
                    buf.push_str(t.name());
                    buf.push_str(": ");
                    buf.push_str(t.description());
                    buf.push('\n');
                }
                buf.push('\n');
            }
        }
        let hist = self.history.lock();
        for t in hist.iter() {
            match t.role {
                TurnRole::User => buf.push_str("User: "),
                TurnRole::Model => buf.push_str("Assistant: "),
            }
            buf.push_str(&t.text);
            buf.push('\n');
        }
        // NO trailing space after the colon: a dangling "▁" token boundary
        // collapses the 270M base model into degenerate output ("1000…" for a
        // factual continuation — proven by a native A/B with identical
        // weights). The model emits the leading space itself.
        buf.push_str("Assistant:");
        buf
    }

    /// Emit a `ToolCall` observability step so a UI can render the tool block.
    /// Sourced `Done` (NOT `Active`) — exactly like `state::LoopState`'s
    /// `emit_chunk_step`: the call is dispatched INLINE below, and the Agent's
    /// `spawn_tool_dispatcher` RE-EXECUTES any non-`Done` registered tool-call
    /// step it sees on the broadcast, so an `Active` step here would double-fire
    /// every tool (side effects applied twice, hooks evaluated twice).
    fn emit_tool_call_step(&self, tc: &ToolCall) {
        self.emit(Step::tool_call(
            self.alloc_step_index(),
            tc.clone(),
            StepStatus::Done,
        ));
    }

    /// Emit a `ToolCall`-kind step carrying the resolved [`ToolResult`] so a UI
    /// can flip the tool block from "running" to ok/err. Sourced `Model` /
    /// targeted `Environment` (NOT `System`+`Error`) so it never trips the
    /// `subscribe_steps` "turn-failure" translation that converts a
    /// System+Error step into a stream `Err` — a *tool* error must not abort
    /// the whole turn. The error message rides in `error`; the result value is
    /// reflected back into history as a ```` ```tool_output ```` turn for the
    /// next model round.
    fn emit_tool_result_step(&self, result: &ToolResult) {
        self.emit(Step::tool_result(self.alloc_step_index(), result.clone()));
    }
}

// Flatten `SystemInstructions` into a plain preamble string — the shared
// backend-neutral renderer (also used by the Anthropic backend).
use crate::backends::render_system;

/// Extract the user prompt text from a `Content`. Media parts are dropped (the
/// local text model has no multimodal path).
fn content_to_text(content: Content) -> String {
    let mut buf = String::new();
    for p in content.parts {
        if let Part::Text(t) = p {
            buf.push_str(&t);
        }
    }
    buf
}

// =============================================================================
// Strategy
// =============================================================================

/// Injected runners for inline tool dispatch in the local backend — an
/// alias of the shared [`BackendRunners`](crate::backends::BackendRunners).
pub type LocalRunners = crate::backends::BackendRunners;

/// Factory that opens a [`LocalConnection`].
pub struct LocalConnectionStrategy {
    config: LocalBackendConfig,
    runners: LocalRunners,
}

impl LocalConnectionStrategy {
    /// Create a strategy from a backend config.
    pub fn new(config: LocalBackendConfig) -> Self {
        Self {
            config,
            runners: LocalRunners::default(),
        }
    }

    /// Inject the runners the Agent owns (parity with the other backends).
    pub fn with_runners(mut self, runners: LocalRunners) -> Self {
        self.runners = runners;
        self
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl ConnectionStrategy for LocalConnectionStrategy {
    async fn connect(&self) -> Result<Arc<dyn Connection>> {
        let system = self.config.system_instructions.as_ref().map(render_system);

        // Attempt to load the engine. Absent weights are NOT an error (first
        // run before the opt-in download) — the connection still opens and
        // `send()` reports the missing model.
        let engine = load_engine(&self.config).await?;

        // Register the built-in tools onto the runner `Agent::start_local`
        // handed us, reusing the Gemini backend's `register_builtins` exactly
        // as the Anthropic backend does. Both LLM-client slots are `None`: the
        // 8 fs builtins + portable tools register (over OPFS on wasm); the two
        // Gemini-client-coupled tools (start_subagent / generate_image) skip.
        // `connect()` is `&self`, so the runner `Arc`s are CLONED out.
        if let Some(runner) = self.runners.tool_runner.as_ref() {
            let deps = BuiltinDeps {
                chat_client: None,
                chat_model: self.config.model.clone(),
                image_client: None,
                image_model: String::new(),
                fs: self.config.filesystem.clone(),
                hooks: self.runners.hook_runner.clone(), // for subagent policy inheritance (M8); inert here (no chat_client)
            };
            let registered = register_builtins(runner, &self.config.capabilities, &deps);
            if !registered.is_empty() {
                tracing::debug!(?registered, "registered built-in tools (local)");
            }
        }

        let (steps_tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = Arc::new(LoopState::new(steps_tx, system));

        let conv_id = self
            .config
            .conversation_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        let typed = Arc::new(LocalConnection {
            state,
            engine: engine.map(Arc::new),
            conversation_id: conv_id.into(),
            tool_runner: self.runners.tool_runner.clone(),
            hook_runner: self.runners.hook_runner.clone(),
            session_ctx: self.runners.session_ctx.clone(),
        });
        Ok(typed)
    }
}

// =============================================================================
// Connection
// =============================================================================

/// A live local-model session implementing [`Connection`].
pub struct LocalConnection {
    state: Arc<LoopState>,
    /// The loaded engine. `None` until the user downloads the weights.
    engine: Option<Arc<Engine>>,
    conversation_id: Arc<str>,
    /// Tool runner for inline dispatch (fs builtins + the agent's custom
    /// tools). `None` for bare/test connections — the loop then just emits the
    /// model's text.
    tool_runner: Option<Arc<ToolRunner>>,
    /// Hook runner for pre/post tool-call gating.
    hook_runner: Option<Arc<HookRunner>>,
    /// Session context root for hook dispatch.
    session_ctx: Option<SessionContext>,
}

impl LocalConnection {
    /// True when the weights have been loaded (the user completed the download).
    pub fn is_model_loaded(&self) -> bool {
        self.engine.is_some()
    }
}

// The session surface (history snapshot/restore, compaction, transcript)
// lives on the `Connection` trait impl below — R6 moved it off the inherent
// impl so `Agent` needs no typed backend handle.

/// Decode opaque history bytes from [`LocalConnection::history_bytes`] into a
/// flat transcript without a live connection.
pub fn decode_transcript_bytes(bytes: &[u8]) -> Result<Vec<TranscriptEntry>> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let history: Vec<Turn> = serde_json::from_slice(bytes)
        .map_err(|e| Error::other(format!("decode_transcript_bytes: {e}")))?;
    Ok(history
        .into_iter()
        .map(|t| TranscriptEntry {
            role: match t.role {
                TurnRole::User => TranscriptRole::User,
                TurnRole::Model => TranscriptRole::Assistant,
            },
            text: t.text,
            tool_calls: Vec::new(),
        })
        .collect())
}

/// Build the turn-terminating step (model-sourced, user-facing, DONE) carrying
/// the generated text.
fn terminal_step(state: &LoopState, traj: &str, text: String, finished: bool) -> Step {
    Step::turn_complete(
        traj,
        state.alloc_step_index(),
        StepStatus::Done,
        text,
        "",
        finished,
        None,
        None,
    )
}

/// Build a System/Error terminal step. `subscribe_steps` translates this into a
/// stream `Err` so the failure surfaces to `chat()`/`text()` instead of being
/// swallowed as an empty success (same convention as the Anthropic backend).
fn error_step(state: &LoopState, message: String) -> Step {
    Step::turn_error(state.alloc_step_index(), message)
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Connection for LocalConnection {
    fn is_idle(&self) -> bool {
        self.state.idle.load(Ordering::Acquire)
    }

    fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    async fn send(&self, content: Content) -> Result<()> {
        let state = self.state.clone();
        let engine = self.engine.clone();
        let tool_runner = self.tool_runner.clone();
        let hook_runner = self.hook_runner.clone();
        let traj = uuid::Uuid::new_v4().to_string();

        // ONE turn context shared by the pre-turn gate, the per-call tool
        // hooks, and the post-turn hooks (mirrors the network backends'
        // `session_ctx.child()`).
        let turn_ctx = self
            .session_ctx
            .as_ref()
            .map(|s| s.child())
            .unwrap_or_default();

        // Pre-turn gate — BEFORE the prompt enters history, so a denied
        // prompt never pollutes context. On deny the model is never run; the
        // turn_error Step becomes a stream `Err` via `subscribe_step_stream`.
        if let Some(denied) =
            crate::backends::dispatch::gate_pre_turn(hook_runner.as_ref(), &turn_ctx, &content)
                .await
        {
            state.emit(error_step(&state, denied));
            return Ok(());
        }

        let prompt = content_to_text(content);
        state.idle.store(false, Ordering::Release);
        state.cancel.store(false, Ordering::Release);
        state.history.lock().push(Turn {
            role: TurnRole::User,
            text: prompt,
        });

        crate::runtime::spawn(async move {
            let Some(engine) = engine else {
                state.emit(error_step(
                    &state,
                    "local model not downloaded — open the model tab and download Gemma first"
                        .to_string(),
                ));
                state.idle.store(true, Ordering::Release);
                state.idle_notify.notify_waiters();
                return;
            };

            // Bounded generate → parse-tool → dispatch → feed-result → re-loop.
            // Best-effort: any round whose output has no `tool_code` fence (the
            // common case for the tiny base model) ends the turn with that text
            // as the terminal Step, exactly as the old single-shot path did.
            let mut final_text = String::new();
            // The model called `finish` this turn — flags the terminal step
            // as `StepType::Finish` (see `gemini::loop`).
            let mut finished_turn = false;
            let mut rounds = 0u32;
            loop {
                rounds += 1;
                if rounds > MAX_TOOL_ROUNDS || state.cancel.load(Ordering::Acquire) {
                    break;
                }

                let rendered = state.render_prompt_with_tools(tool_runner.as_deref());
                // Stream each newly-stable decoded slice as a text-delta Step
                // (the SAME shape the network backends' `EmitCtx::push_text`
                // emits), so the transcript paints tokens as they generate.
                // One step_index per round, like `EmitCtx`. The callback
                // returns the negated cancel flag so Stop halts generation
                // mid-round. The terminal `turn_complete` below still carries
                // the authoritative (trimmed) full text; `step_to_chunks`'s
                // suffix recovery only emits what the deltas didn't cover.
                let delta_index = state.alloc_step_index();
                let reply = engine
                    .run(&rendered, |delta| {
                        state.emit(Step::text_delta(&traj, delta_index, delta));
                        !state.cancel.load(Ordering::Acquire)
                    })
                    .await;
                // The base model keeps continuing the flat transcript past its
                // own turn (fabricating "User:" lines) — cut at the first one.
                let reply = match reply.find("\nUser:") {
                    Some(i) => reply[..i].trim().to_string(),
                    None => reply.trim().to_string(),
                };

                // Parse a tool call only when a runner is present; otherwise the
                // model output is always plain text.
                let parsed = tool_runner
                    .as_ref()
                    .and_then(|_| parse_tool_code(&reply));

                let Some((name, args)) = parsed else {
                    // Plain text → terminal turn.
                    final_text = reply.clone();
                    state.history.lock().push(Turn {
                        role: TurnRole::Model,
                        text: reply,
                    });
                    break;
                };

                // Always record the model's tool-call turn in history.
                state.history.lock().push(Turn {
                    role: TurnRole::Model,
                    text: reply.clone(),
                });

                // Explicit `finish()` ends the turn (the model is done).
                if name == FINISH_TOOL_NAME {
                    final_text = reply;
                    finished_turn = true;
                    break;
                }

                let tool_call = ToolCall {
                    name: name.clone(),
                    args,
                    id: None,
                    canonical_path: None,
                };
                state.emit_tool_call_step(&tool_call);

                // The shared pipeline (mirror the Gemini loop): pre-hooks +
                // policy → execute → `{"error": ...}` lift → post-hooks.
                let post_result = crate::backends::dispatch::dispatch_tool_call(
                    tool_runner.as_ref(),
                    hook_runner.as_ref(),
                    &turn_ctx,
                    &tool_call,
                )
                .await;
                state.emit_tool_result_step(&post_result);

                // Feed the result back as a ```tool_output``` user turn and
                // re-loop so the model can react.
                let result_value = post_result.result.unwrap_or(Value::Null);
                let out = serde_json::to_string(&result_value).unwrap_or_default();
                state.history.lock().push(Turn {
                    role: TurnRole::User,
                    text: format!("```tool_output\n{out}\n```"),
                });
            }

            state.emit(terminal_step(&state, &traj, final_text.clone(), finished_turn));

            // Post-turn hooks observe the completed turn's final text — fired
            // after the terminal step, never on denied or failed turns (the
            // gate / not-downloaded branches returned before this point).
            crate::backends::dispatch::dispatch_post_turn(
                hook_runner.as_ref(),
                &turn_ctx,
                &final_text,
            )
            .await;

            state.idle.store(true, Ordering::Release);
            state.idle_notify.notify_waiters();
        });
        Ok(())
    }

    async fn send_trigger(&self, content: String) -> Result<()> {
        self.send(Content::text(content)).await
    }

    async fn send_tool_results(&self, _results: Vec<ToolResult>) -> Result<()> {
        // The local backend dispatches tools inline inside `send`'s loop, so
        // there are no out-of-band results to inject here.
        Ok(())
    }

    fn subscribe_steps(&self) -> StepStream {
        // A System/Error turn-failure Step (e.g. "model not downloaded")
        // surfaces as a stream `Err` — the uniform backend convention.
        crate::backends::subscribe_step_stream(self.state.steps.subscribe(), "local")
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

    /// Snapshot the in-memory history as opaque JSON bytes. Round-trips through
    /// `set_history_bytes`.
    fn history_bytes(&self) -> Result<Option<Vec<u8>>> {
        let snapshot = self.state.history.lock().clone();
        serde_json::to_vec(&snapshot)
            .map(Some)
            .map_err(|e| Error::other(format!("history_bytes: {e}")))
    }

    /// Replace the entire history with one previously returned by
    /// `history_bytes`.
    fn set_history_bytes(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let restored: Vec<Turn> = serde_json::from_slice(bytes)
            .map_err(|e| Error::other(format!("set_history_bytes: {e}")))?;
        *self.state.history.lock() = restored;
        Ok(())
    }

    // compact: trait default (`false`) — the local in-memory backend has no
    // remote summariser, so compaction is a no-op with the history unchanged.

    /// Wipe the conversation history, returning the connection to an empty
    /// context. Synchronous (no network). Backs [`crate::Agent::clear_history`].
    fn clear_history(&self) {
        self.state.history.lock().clear();
        self.state.next_step_index.store(0, Ordering::Relaxed);
    }

    /// Project the in-memory history into a flat `(role, text)` transcript. The
    /// local backend has no tool-call activity, so every entry's `tool_calls`
    /// is empty.
    fn transcript(&self) -> Vec<TranscriptEntry> {
        self.state
            .history
            .lock()
            .iter()
            .map(|t| TranscriptEntry {
                role: match t.role {
                    TurnRole::User => TranscriptRole::User,
                    TurnRole::Model => TranscriptRole::Assistant,
                },
                text: t.text.clone(),
                tool_calls: Vec::new(),
            })
            .collect()
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::conversation::Conversation;
    use serde_json::json;

    /// With no weights present, a turn surfaces a clear "not downloaded" error
    /// through the shared `Conversation` rather than an empty success.
    #[tokio::test]
    async fn send_without_weights_errors_clearly() {
        let cfg = LocalBackendConfig::new("gemma-3-270m"); // no filesystem → no engine
        let conn = LocalConnectionStrategy::new(cfg)
            .connect()
            .await
            .expect("connect succeeds even without weights");
        let conv = Conversation::new(conn);
        let resp = conv.chat("hi").await.expect("send dispatches");
        match resp.text().await {
            Ok(t) => panic!("expected a 'not downloaded' error, got: {t:?}"),
            Err(e) => assert!(
                e.to_string().contains("not downloaded"),
                "expected the not-downloaded message, got: {e}"
            ),
        }
    }

    /// History bytes round-trip through `set_history_bytes` / the transcript.
    #[test]
    fn history_round_trips() {
        let (tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = Arc::new(LoopState::new(tx, None));
        state.history.lock().push(Turn {
            role: TurnRole::User,
            text: "hello".into(),
        });
        state.history.lock().push(Turn {
            role: TurnRole::Model,
            text: "hi there".into(),
        });
        let conn = LocalConnection {
            state,
            engine: None,
            conversation_id: "test".into(),
            tool_runner: None,
            hook_runner: None,
            session_ctx: None,
        };
        let bytes = conn.history_bytes().unwrap().expect("local keeps history");
        let entries = decode_transcript_bytes(&bytes).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].role, TranscriptRole::User);
        assert_eq!(entries[1].text, "hi there");
    }

    /// The dispatch glue: a fenced `tool_code` call parsed out of model text
    /// drives the real `ToolRunner.execute` path the `send` loop uses. Proves a
    /// stubbed tool actually runs with the parsed args (no model needed).
    #[tokio::test]
    async fn fenced_call_dispatches_through_runner() {
        use crate::tools::{ClosureTool, ToolRunner};
        use std::sync::Arc;

        let runner = ToolRunner::new();
        runner.register(ClosureTool::new(
            "echo",
            "echo back the message",
            json!({
                "type": "object",
                "properties": { "msg": { "type": "string" } }
            }),
            |args, _ctx| async move {
                let msg = args["msg"].as_str().unwrap_or("").to_string();
                Ok(json!({ "echoed": msg }))
            },
        ));
        let runner = Arc::new(runner);

        // The exact parse → execute steps the loop runs.
        let reply = "ok\n```tool_code\necho(msg=\"hi there\")\n```";
        let (name, args) = parse_tool_code(reply).expect("a parsed call");
        assert_eq!(name, "echo");
        let out = runner.execute(&name, args).await.expect("execute");
        assert_eq!(out["echoed"], "hi there");
    }

    /// `render_prompt_with_tools` lists the registered tools + the `tool_code`
    /// instruction so a tool-capable model can emit a fence; with no runner it
    /// matches the plain prompt.
    #[test]
    fn prompt_lists_registered_tools() {
        use crate::tools::{ClosureTool, ToolRunner};

        let (tx, _) = broadcast::channel::<Step>(STEP_BROADCAST_CAPACITY);
        let state = LoopState::new(tx, None);
        state.history.lock().push(Turn {
            role: TurnRole::User,
            text: "read it".into(),
        });

        let runner = ToolRunner::new();
        runner.register(ClosureTool::new(
            "view_file",
            "read a file",
            json!({ "type": "object" }),
            |_a, _c| async move { Ok(json!({})) },
        ));

        let with = state.render_prompt_with_tools(Some(&runner));
        assert!(with.contains("```tool_code"));
        assert!(with.contains("view_file: read a file"));
        // No runner → no tool preamble (degrades to a plain prompt).
        let without = state.render_prompt_with_tools(None);
        assert!(!without.contains("```tool_code"));
        assert!(without.contains("User: read it"));
    }

    /// NATIVE forward-pass validation — the definitive end-to-end proof. Ignored
    /// by default (needs the ~570MB checkpoint downloaded). Run with:
    ///   GEMMA_DIR=target/gemma-test cargo test --features local -- --ignored --nocapture gemma_native_forward
    /// Loads the REAL `unsloth/gemma-3-270m` checkpoint into the Burn model on
    /// the native wgpu backend, runs a greedy continuation, and prints it. A
    /// COHERENT continuation validates the loader end-to-end — the HF→Burn name
    /// map, the `q/k/v/o` transpose, the `(1+w)` RMSNorm convention, the RoPE
    /// permutation, and the tied-embedding head. Garbage output means one of
    /// those conventions is wrong (most likely the RoPE pairing direction).
    #[tokio::test]
    #[ignore]
    async fn gemma_native_forward() {
        let dir = std::env::var("GEMMA_DIR")
            .expect("set GEMMA_DIR to a folder with model.safetensors + tokenizer.json");
        let weights = std::fs::read(format!("{dir}/model.safetensors")).expect("read weights");
        let tok_bytes = std::fs::read(format!("{dir}/tokenizer.json")).expect("read tokenizer.json");

        let device = burn::backend::wgpu::WgpuDevice::default();
        let model = super::super::gemma::GemmaModel::<super::super::LocalBackend>::init(
            super::super::gemma::GemmaConfig::gemma_3_270m(),
            &device,
        );
        let model =
            super::super::weights::load_gemma(model, &weights, &device).expect("load_gemma");
        let tok = super::super::tokenizer::GemmaTokenizer::from_bytes(&tok_bytes)
            .expect("load tokenizer");

        let prompt = std::env::var("GEMMA_PROMPT")
            .unwrap_or_else(|_| "The capital of France is".to_string());
        let prompt = prompt.as_str();
        let out = super::super::generate::generate(&model, &tok, prompt, 16, &device).await;
        println!("\n=== GEMMA NATIVE FORWARD ===\nprompt: {prompt:?}\noutput:  {out:?}\n============================\n");
        assert!(
            !out.trim().is_empty(),
            "empty continuation — immediate EOS or a loader bug"
        );
    }
}
