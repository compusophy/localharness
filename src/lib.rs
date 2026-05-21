//! # localharness — Rust client for the Antigravity agent runtime
//!
//! Unofficial Rust port of the [`google-antigravity`](https://pypi.org/project/google-antigravity/)
//! Python SDK. Drives the same `localharness` binary using the same wire
//! protocol, so Rust callers can build agents against a Gemini-backed
//! runtime with the same semantics as the Python client.
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use localharness::{Agent, LocalAgentConfig};
//!
//! # async fn run() -> localharness::Result<()> {
//! let cfg = LocalAgentConfig::new()
//!     .with_system_instructions("You are a concise code reviewer.")
//!     .with_api_key(std::env::var("GEMINI_API_KEY").unwrap());
//!
//! let agent = Agent::start_local(cfg).await?;
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
//!
//! [`Agent`]: agent::Agent
//! [`Conversation`]: conversation::Conversation
//! [`ChatResponse`]: conversation::ChatResponse
//! [`connections::Connection`]: connections::Connection

pub mod agent;
pub mod backends;
pub mod connections;
pub mod content;
pub mod conversation;
pub mod error;
pub mod hooks;
pub mod policy;
pub mod proto;
pub mod tools;
pub mod triggers;
pub mod types;

pub use agent::{Agent, AgentConfig, GeminiAgentConfig, LocalAgentConfig};
pub use backends::gemini::{
    GeminiBackendConfig, GeminiConnection, GeminiConnectionStrategy,
};
pub use connections::local::LocalConfig;
pub use connections::{Connection, ConnectionStrategy};
pub use content::{Content, Media, MediaKind, Part};
pub use conversation::{ChatCursor, ChatResponse, Conversation};
pub use error::{Error, Result};
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
    SystemInstructions, ThinkingLevel, ToolCall, ToolResult, TriggerDelivery, UsageMetadata,
};
