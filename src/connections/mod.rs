//! Transport abstraction.
//!
//! A [`Connection`] is a live session with a backend agent runtime. A
//! [`ConnectionStrategy`] is the factory that opens one. Conversation /
//! Agent code depends only on these traits — never on transport details.
//!
//! The only shipping backend is the Rust-native Gemini runtime under
//! [`crate::backends::gemini`]. The 0.1.x `LocalConnection` (Go binary
//! over WebSocket) was removed in 0.3.0; see `CHANGELOG.md`.

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::stream::BoxStream;

use crate::content::Content;
use crate::error::Result;
use crate::types::{Step, ToolResult};

/// A live, owned session with a backend.
///
/// Implementations are `Send + Sync` and may be shared via `Arc`. Every
/// method takes `&self` so handlers (tools, triggers) can call back
/// into the connection without exclusive access.
#[async_trait]
pub trait Connection: Send + Sync {
    /// True when the backend reports no active turn. Backed by an
    /// `AtomicBool` so callers may poll without contention.
    fn is_idle(&self) -> bool;

    /// The stable identifier the backend assigned to this conversation.
    fn conversation_id(&self) -> &str;

    /// Send a user prompt. Returns once the message is dispatched —
    /// the response arrives via [`subscribe_steps`].
    async fn send(&self, content: Content) -> Result<()>;

    /// Push an out-of-band trigger event into the agent. Unlike `send`,
    /// this does not switch the turn boundary.
    async fn send_trigger(&self, content: String) -> Result<()>;

    /// Return the next-batch results for outstanding tool calls.
    /// Backends that dispatch tools inline (Gemini) accept this as a
    /// no-op.
    async fn send_tool_results(&self, results: Vec<ToolResult>) -> Result<()>;

    /// Stream of steps as the backend produces them. Each call returns
    /// an independent cursor; the underlying source is typically a
    /// broadcast channel so late subscribers see steps that arrive
    /// after they subscribe.
    fn subscribe_steps(&self) -> BoxStream<'static, Result<Step>>;

    /// Park the caller until the backend transitions to idle.
    async fn wait_for_idle(&self) -> Result<()>;

    /// Tear the connection down. Idempotent.
    async fn shutdown(&self) -> Result<()>;
}

/// Opens a [`Connection`].
#[async_trait]
pub trait ConnectionStrategy: Send + Sync {
    async fn connect(&self) -> Result<Arc<dyn Connection>>;
}
