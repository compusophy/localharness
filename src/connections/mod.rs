//! Transport abstraction.
//!
//! A `Connection` is a live session with a single backend (local harness,
//! remote service, etc.). A `ConnectionStrategy` is the factory that opens
//! one. Conversation/Agent code depends only on these traits — never on
//! WebSocket, subprocess, or proto details.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::BoxStream;

use crate::content::Content;
use crate::error::Result;
use crate::types::{Step, ToolResult};

pub mod local;

/// A live, owned session with a backend.
///
/// Implementations are `Send + Sync` and may be shared via `Arc`. All methods
/// take `&self` so handlers (tool runners, triggers) can call back into the
/// connection without exclusive access.
#[async_trait]
pub trait Connection: Send + Sync {
    /// True when the backend reports no active turn. Implementations back
    /// this with an `AtomicBool` so callers can poll without contention.
    fn is_idle(&self) -> bool;

    /// The stable identifier the backend assigned to this conversation.
    fn conversation_id(&self) -> &str;

    /// Send a user prompt (textual or multimodal). Returns once the message
    /// is dispatched — the response arrives via `receive_steps`.
    async fn send(&self, content: Content) -> Result<()>;

    /// Push an out-of-band trigger event into the agent. Unlike `send`, this
    /// does not switch the turn boundary.
    async fn send_trigger(&self, content: String) -> Result<()>;

    /// Return the next-batch results for outstanding tool calls.
    async fn send_tool_results(&self, results: Vec<ToolResult>) -> Result<()>;

    /// Stream of steps as the backend produces them. Each call returns an
    /// independent cursor; the underlying source is a broadcast channel so
    /// late subscribers see only steps that arrive after they subscribe.
    fn subscribe_steps(&self) -> BoxStream<'static, Result<Step>>;

    /// Park the caller until the backend transitions to idle.
    async fn wait_for_idle(&self) -> Result<()>;

    /// Tear the connection down. Idempotent. Implementations must not panic
    /// if the backing process has already exited.
    async fn shutdown(&self) -> Result<()>;
}

/// Opens a `Connection`. Strategies own the configuration needed to bring up
/// a backend; the act of connecting is async because it may spawn processes,
/// negotiate handshakes, and contact the network.
#[async_trait]
pub trait ConnectionStrategy: Send + Sync {
    async fn connect(&self) -> Result<Arc<dyn Connection>>;
}
