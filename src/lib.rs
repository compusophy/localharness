//! # localharness — Rust-native, model-agnostic agent SDK
//!
//! Build production agents with streaming text, custom tools, safety
//! policies, and background triggers — all from a single `cargo add`,
//! zero external binaries.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use localharness::{Agent, GeminiAgentConfig};
//!
//! # async fn run() -> localharness::Result<()> {
//! let cfg = GeminiAgentConfig::new(std::env::var("GEMINI_API_KEY").unwrap())
//!     .with_system_instructions("You are a concise code reviewer.");
//!
//! let agent = Agent::start_gemini(cfg).await?;
//! let response = agent.chat("What is 2+2?").await?;
//! println!("{}", response.text().await?);
//! agent.shutdown().await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Layers
//!
//! | Layer | Type | Purpose |
//! |-------|------|---------|
//! | 1 | [`Agent`] | High-level facade: connect, chat, shutdown. |
//! | 2 | [`Conversation`] / [`ChatResponse`] | Stateful session, multi-cursor streams. |
//! | 3 | [`connections::Connection`] | Transport abstraction. |
//! | aux | [`Filesystem`] | What the 6 fs-shaped built-in tools call into; swap the impl to target OPFS, an in-memory FS, etc. |
//!
//! [`Agent`]: agent::Agent
//! [`Conversation`]: conversation::Conversation
//! [`ChatResponse`]: conversation::ChatResponse
//! [`connections::Connection`]: connections::Connection
//! [`Filesystem`]: filesystem::Filesystem

// On wasm32 the crate is single-threaded (browser) and intentionally
// uses `Arc` over non-Send/Sync values via the `MaybeSendSync` marker
// (see `runtime.rs`). Clippy's `arc_with_non_send_sync` fires on every
// such use; it's by design on this target, so silence it crate-wide for
// wasm rather than peppering `#[allow]` across the modules.
#![cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]

// On wasm32 the upper architecture (Agent → Conversation → Connection)
// is temporarily gated behind `native` because its trait bounds require
// `Send` futures, which reqwest's browser fetch can't satisfy. The wasm
// surface exposes `error`, `content`, `types`, and the low-level
// `backends::gemini::api::GeminiClient` so a web demo can drive the
// Gemini REST API directly. Lifting the gate is M2.5: thread a
// `MaybeSend` shim through the Tool/Connection/Hook traits.
/// Layer-1 agent facade: connect, chat, shutdown.
pub mod agent;
/// Backend implementations (Gemini, MCP).
pub mod backends;
/// The crate-wide built-in tool registry (fs tools, ask_question, finish,
/// call_agent, ...) — backend-neutral; every backend registers from here.
/// Formerly `backends::gemini::tools` (a re-export shim remains there).
pub mod builtins;
/// Transport abstraction traits.
pub mod connections;
/// Multimodal input primitives (text, images, documents, audio, video).
pub mod content;
/// Stateful conversation session with multi-cursor streaming.
pub mod conversation;
/// Typed error hierarchy.
pub mod error;
/// The one `LHxxxx` error-code registry (compile / runtime / tx-revert).
pub mod error_codes;
/// Filesystem abstraction for built-in fs tools.
pub mod filesystem;
pub(crate) mod runtime;
/// Hook traits for observing and gating agent events.
pub mod hooks;
/// Declarative tool-execution policy engine.
pub mod policy;
/// Custom tool registration and dispatch.
pub mod tools;
/// Background triggers that push messages into the agent.
pub mod triggers;
/// Public boundary types (steps, tool calls, usage, config, etc.).
pub mod types;

/// Rust-subset to wasm compiler.
pub mod rustlite;

/// Solidity/EVM-subset to EVM-bytecode compiler foundation (the EVM analog of
/// [`rustlite`]): a bytecode assembler + worked dispatch/init scaffolding. See
/// `design/soliditylite.md`.
pub mod soliditylite;

/// Pure framebuffer rasterization + `Viewport` (the host::compose geometry
/// foundation; native-testable, used by `app::display`). See `src/raster.rs`.
pub mod raster;

/// Compositor scheduling for `host::compose` — the deferred-mutation module
/// table (native-testable control flow). See `src/compose.rs`.
pub mod compose;

/// Pure hex / address / amount encoding helpers (native-testable). Hoisted out
/// of `app::events` so they run under `cargo test`. See `src/encoding.rs`.
pub mod encoding;

/// Pure, deterministic CONVERGENT reconcile for cross-device shared-folder sync
/// (native-testable). Hoisted out of `app::sharedfs_sync` so the convergence /
/// symmetry property runs under `cargo test`. See `src/sharedfs_reconcile.rs`.
pub mod sharedfs_reconcile;

/// Pure signed-envelope layer for on-chain WebRTC signaling blobs — the SDP
/// sealing/sender-authentication core (native-testable; needs `wallet` for
/// k256). Hoisted out of `app::teams_sync` so the seal/unseal round-trip and
/// tamper/forgery rejection run under `cargo test`. See `src/signaling_seal.rs`.
#[cfg(feature = "wallet")]
pub mod signaling_seal;

/// Pure Last-Writer-Wins key/value CRDT for SessionRoom shared state (#22):
/// folds a set of decrypted ops into a converged map (order-independent,
/// idempotent, optional TTL). Native-testable. See `src/kv_reduce.rs`.
pub mod kv_reduce;

/// SessionRoom op sealing/opening + deterministic per-room key derivation (#22):
/// AES-256-GCM confidentiality under `K_room` inside a writer-signed,
/// room-bound `signaling_seal` envelope. Needs `wallet` for k256/keccak.
/// Native-testable. See `src/kv_room.rs`.
#[cfg(feature = "wallet")]
pub mod kv_room;

/// Pure typed-confirmation challenge gate for destructive tools
/// (native-testable, `turn_flow` hoisting pattern): single-use random nonce
/// bound to exact tool+args, valid only when typed by the USER. Enforced by
/// `app::chat::confirm_guard` at the dispatch layer. See `src/confirm.rs`.
pub mod confirm;

/// Pure turn-outcome classification for the continuous-execution chat loop
/// (native-testable). Hoisted out of `app::chat` so its guard tests run under
/// `cargo test`. See `src/turn_flow.rs`.
pub mod turn_flow;

/// Pure state machine for the turn-stage micro-pipeline ("paying → thinking
/// → streaming") shown inside a pending assistant turn (native-testable,
/// same hoisting pattern as `turn_flow`). See `src/turn_stage.rs`.
pub mod turn_stage;

/// Pure lessons-blob merging + prompt-section composition for the agent
/// LESSONS LOOP (native-testable). The browser `record_lesson` tool, the
/// headless CLI `call`, and the proxy scheduler worker all fold its output
/// into the system prompt. See `src/lessons.rs`.
pub mod lessons;

/// Pure subdomain-name validation (native-testable) — the single source of
/// truth shared by the browser create tools and kept in sync with the
/// on-chain `LocalharnessRegistryFacet._isValidName` rule. See `src/subdomain.rs`.
pub mod subdomain;

// Inline SVG QR-code generation for the app's share surfaces (device
// pairing, publish share, `?invite=` links). Feature-gated like `app`
// but NOT wasm-gated, so its unit test runs under a native
// `cargo test --features browser-app` (the `turn_flow` hoisting pattern).
#[cfg(feature = "browser-app")]
mod qr;

// Apex fresh-visitor landing markup — hoisted out of the wasm-gated `app/`
// tree (the raster.rs/compose.rs pattern) so the SHIPPING markup also
// renders natively: `cargo test --features browser-app landing_preview`
// writes `target/landing-preview.html` for screenshot review. The `test`
// arm keeps non-test native builds free of dead-code (only the wasm app
// and the preview test consume it).
#[cfg(all(feature = "browser-app", any(target_arch = "wasm32", test)))]
mod landing;

// The browser-resident IDE. Gated on the `browser-app` feature AND a
// wasm target, so a native `cargo add localharness` never compiles it.
#[cfg(all(feature = "browser-app", target_arch = "wasm32"))]
mod app;

// M6 spike: in-browser secp256k1 keypair via alloy's local signer.
// Pure-compute (no HTTP, no JS deps), so it builds on every target.
/// Secp256k1 keypair, BIP-39 mnemonics, and RLP encoding.
#[cfg(feature = "wallet")]
pub mod wallet;

// JSON-RPC client for the `LocalharnessRegistry` diamond on Tempo
// Moderato. Read-only views (`check_name`, `owner_of_name`,
// `tba_of_name`, `list_owned_tokens`) work on every target; the
// sponsored writes sign with a `k256` key (needs the wallet feature)
// and use `tokio::time::sleep` on native / `setTimeout` on wasm to
// poll the receipt. The diamond's address is baked in as
// `registry::REGISTRY_ADDRESS`; the RPC URL is `registry::RPC_URL`.
/// JSON-RPC client for the on-chain registry diamond.
#[cfg(feature = "wallet")]
pub mod registry;

// Tempo Transaction encoder (tx type 0x76). Implements Tempo's native
// account-abstraction tx format so users can pay fees in $LH instead
// of native and so a project-controlled fee_payer can sponsor user
// txs without users holding any balance. Wire format per
// docs.tempo.xyz/protocol/transactions/spec-tempo-transaction.
/// Tempo Transaction (tx type 0x76) encoder for native account abstraction.
#[cfg(feature = "wallet")]
pub mod tempo_tx;

/// App-injected x402 payment-signing hook (lets the backend `call_agent`
/// tool sign payments using the app-layer wallet).
pub mod x402_hook;

pub use agent::{Agent, AgentConfig, GeminiAgentConfig, MockAgentConfig};
#[cfg(feature = "anthropic")]
pub use agent::AnthropicAgentConfig;
#[cfg(feature = "openai")]
pub use agent::OpenAiAgentConfig;
#[cfg(feature = "local")]
pub use agent::LocalAgentConfig;
pub use backends::gemini::{
    decode_transcript_bytes, GeminiBackendConfig, GeminiConnection, GeminiConnectionStrategy,
};
pub use backends::mock::{
    MockConnection, MockConnectionBuilder, MockConnectionStrategy, MockRunners, ScriptedTurn,
};
#[cfg(feature = "anthropic")]
pub use backends::anthropic::{
    AnthropicBackendConfig, AnthropicConnection, AnthropicConnectionStrategy, AnthropicRunners,
};
#[cfg(feature = "openai")]
pub use backends::openai::{
    OpenAiBackendConfig, OpenAiConnection, OpenAiConnectionStrategy, OpenAiRunners,
};
#[cfg(feature = "native")]
pub use backends::mcp::{McpBridge, McpClient, McpToolDecl};
pub use connections::{Connection, ConnectionStrategy};
pub use content::{Content, Media, MediaKind, Part};
pub use conversation::{ChatCursor, ChatResponse, Conversation};
pub use error::{Error, Result};
pub use filesystem::{DirEntry, EntryKind, Filesystem, Metadata, SharedFilesystem, WalkEntry};
#[cfg(feature = "native")]
pub use filesystem::NativeFilesystem;
pub use hooks::{
    HookContext, HookRunner, OnSessionEndHook, OnSessionStartHook, OperationContext,
    PostToolCallHook, PostTurnHook, PreToolCallDecideHook, PreTurnHook, SessionContext,
    TurnContext,
};
pub use policy::{
    allow_all, deny_all, enforce, evaluate, is_path_in_workspace, secure_normalize_path,
    workspace_only, AskUserHandler, Decision, Policy, Predicate,
};
pub use tools::{ClosureTool, Tool, ToolContext, ToolRunner};
pub use triggers::{every, Trigger, TriggerContext, TriggerRunner};
pub use types::{
    BuiltinTool, CapabilitiesConfig, HookResult, Step, StepSource, StepStatus, StepTarget,
    StepType, StreamChunk, SystemInstructions, ThinkingLevel, ToolCall, ToolResult,
    TranscriptEntry, TranscriptRole, TriggerDelivery, UsageMetadata,
};
