//! # localharness — Rust-native agent SDK for Gemini
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

// On wasm32 the upper architecture (Agent → Conversation → Connection)
// is temporarily gated behind `native` because its trait bounds require
// `Send` futures, which reqwest's browser fetch can't satisfy. The wasm
// surface exposes `error`, `content`, `types`, and the low-level
// `backends::gemini::api::GeminiClient` so a web demo can drive the
// Gemini REST API directly. Lifting the gate is M2.5: thread a
// `MaybeSend` shim through the Tool/Connection/Hook traits.
pub mod agent;
pub mod backends;
pub mod connections;
pub mod content;
pub mod conversation;
pub mod error;
pub mod filesystem;
pub(crate) mod runtime;
pub mod hooks;
pub mod policy;
pub mod tools;
pub mod triggers;
pub mod types;

// The browser-resident IDE. Gated on the `browser-app` feature AND a
// wasm target, so a native `cargo add localharness` never compiles it.
#[cfg(all(feature = "browser-app", target_arch = "wasm32"))]
mod app;

pub use agent::{Agent, AgentConfig, GeminiAgentConfig};
pub use backends::gemini::{
    decode_transcript_bytes, GeminiBackendConfig, GeminiConnection, GeminiConnectionStrategy,
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
    BuiltinTool, CapabilitiesConfig, GeminiConfig, GenerationConfig, HookResult, ModelConfig,
    ModelEntry, Step, StepSource, StepStatus, StepTarget, StepType, StreamChunk,
    SystemInstructions, ThinkingLevel, ToolCall, ToolResult, TranscriptEntry, TranscriptRole,
    TriggerDelivery, UsageMetadata,
};
