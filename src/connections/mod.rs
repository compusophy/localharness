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
#[cfg(not(target_arch = "wasm32"))]
use futures_util::stream::BoxStream;
#[cfg(target_arch = "wasm32")]
use futures_util::stream::LocalBoxStream;

use crate::content::Content;
use crate::error::Result;
use crate::runtime::MaybeSendSync;
use crate::types::{Step, ToolResult};

/// Connection step stream alias. `BoxStream` on native (Send-bound,
/// for tokio::spawn compatibility); `LocalBoxStream` on wasm32 where
/// browser fetch streams aren't Send.
#[cfg(not(target_arch = "wasm32"))]
pub type StepStream = BoxStream<'static, Result<Step>>;
#[cfg(target_arch = "wasm32")]
pub type StepStream = LocalBoxStream<'static, Result<Step>>;

/// A live, owned session with a backend.
///
/// Implementations are `Send + Sync` and may be shared via `Arc`. Every
/// method takes `&self` so handlers (tools, triggers) can call back
/// into the connection without exclusive access.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Connection: MaybeSendSync {
    /// True when the backend reports no active turn. Backed by an
    /// `AtomicBool` so callers may poll without contention.
    fn is_idle(&self) -> bool;

    /// The stable identifier the backend assigned to this conversation.
    fn conversation_id(&self) -> &str;

    /// Send a user prompt. Returns once the message is dispatched —
    /// the response arrives via [`Connection::subscribe_steps`].
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
    fn subscribe_steps(&self) -> StepStream;

    /// Park the caller until the backend transitions to idle.
    async fn wait_for_idle(&self) -> Result<()>;

    /// Request cooperative cancellation of the in-flight turn. The backend
    /// stops at its next safe boundary (between streamed chunks / before
    /// the next model call or tool dispatch) and emits a terminal step, so
    /// the turn ends cleanly. Idempotent and safe to call when idle.
    /// Default: no-op, for backends without cancellation support.
    fn cancel_turn(&self) {}

    /// Tear the connection down. Idempotent.
    async fn shutdown(&self) -> Result<()>;
}

/// Opens a [`Connection`].
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait ConnectionStrategy: MaybeSendSync {
    /// Open a new connection to the backend.
    async fn connect(&self) -> Result<Arc<dyn Connection>>;
}
