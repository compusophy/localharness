//! Stateful conversation session.
//!
//! Wraps a `Connection` and provides:
//!
//! * `send()` / `receive_steps()` — low-level streaming.
//! * `chat()` — one-shot: send a prompt, receive a `ChatResponse` whose
//!   stream of `StreamChunk` events terminates at the end of a turn.
//! * `history()` / `last_response()` / `cumulative_usage()` — introspection.
//!
//! `ChatResponse` is a multi-cursor lazy stream. Every call to
//! `ChatResponse::chunks()` returns a fresh cursor that replays from chunk
//! zero, in the same vein as the Python SDK's per-cursor design. The
//! upstream pull happens once; cursors share the buffered chunks.

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures_core::Stream;
use futures_util::stream::StreamExt;
use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::connections::Connection;
use crate::content::Content;
use crate::error::{Error, Result};
use crate::types::{Step, StreamChunk, ToolCall, UsageMetadata};

// =============================================================================
// Conversation
// =============================================================================

/// Stateful conversation session wrapping a [`Connection`].
///
/// Provides `chat()` for turn-level semantics and `send()` / `receive_steps()`
/// for lower-level streaming. Tracks history, usage, and structured output.
pub struct Conversation {
    connection: Arc<dyn Connection>,
    state: Arc<Mutex<ConversationState>>,
}

#[derive(Default)]
struct ConversationState {
    history: Vec<Step>,
    cumulative_usage: UsageMetadata,
    last_turn_usage: Option<UsageMetadata>,
    last_response: Option<String>,
    last_structured_output: Option<serde_json::Value>,
    turn_count: u64,
}

impl Conversation {
    /// Wrap a connection in a conversation session.
    pub fn new(connection: Arc<dyn Connection>) -> Self {
        Self {
            connection,
            state: Arc::new(Mutex::new(ConversationState::default())),
        }
    }

    /// Clone of the underlying connection handle.
    pub fn connection(&self) -> Arc<dyn Connection> {
        self.connection.clone()
    }

    /// The backend-assigned conversation identifier.
    pub fn conversation_id(&self) -> String {
        self.connection.conversation_id().to_string()
    }

    /// Cooperatively cancel the in-flight turn (e.g. a UI stop button).
    /// The backend stops at its next safe boundary and emits a terminal
    /// step. No-op when idle or on backends without cancellation support.
    pub fn cancel_turn(&self) {
        self.connection.cancel_turn();
    }

    /// All steps received so far, in order.
    pub fn history(&self) -> Vec<Step> {
        self.state.lock().history.clone()
    }

    /// Number of user turns sent in this session.
    pub fn turn_count(&self) -> u64 {
        self.state.lock().turn_count
    }

    /// Token usage accumulated across all turns.
    pub fn cumulative_usage(&self) -> UsageMetadata {
        self.state.lock().cumulative_usage.clone()
    }

    /// Token usage from the most recent turn only.
    pub fn last_turn_usage(&self) -> Option<UsageMetadata> {
        self.state.lock().last_turn_usage.clone()
    }

    /// The model's last textual response, if any.
    pub fn last_response(&self) -> Option<String> {
        self.state.lock().last_response.clone()
    }

    /// The model's last structured output (JSON), if any.
    pub fn last_structured_output(&self) -> Option<serde_json::Value> {
        self.state.lock().last_structured_output.clone()
    }

    /// Raw send: dispatches the prompt and returns once the bytes are on
    /// the wire. Use `chat()` for higher-level turn semantics.
    pub async fn send(&self, content: Content) -> Result<()> {
        {
            let mut state = self.state.lock();
            state.last_turn_usage = Some(UsageMetadata::default());
        }
        self.connection.send(content).await
    }

    /// Drains steps from the connection, accumulating into history and
    /// usage as they arrive. The stream terminates when the connection
    /// closes — callers wanting per-turn termination should use `chat()`.
    pub fn receive_steps(&self) -> crate::connections::StepStream {
        let upstream = self.connection.subscribe_steps();
        let state = self.state.clone();
        let mapped = upstream.map(move |res| {
            if let Ok(step) = &res {
                let mut s = state.lock();
                s.history.push(step.clone());
                if let Some(u) = &step.usage_metadata {
                    s.cumulative_usage.accumulate(u);
                    if let Some(turn) = s.last_turn_usage.as_mut() {
                        turn.accumulate(u);
                    } else {
                        let mut fresh = UsageMetadata::default();
                        fresh.accumulate(u);
                        s.last_turn_usage = Some(fresh);
                    }
                }
                if step.is_terminal_response() {
                    s.last_response = Some(step.content.clone());
                }
                if let Some(out) = &step.structured_output {
                    s.last_structured_output = Some(out.clone());
                }
            }
            res
        });
        // BoxStream requires Send; wasm fetch streams aren't.
        #[cfg(not(target_arch = "wasm32"))]
        {
            mapped.boxed()
        }
        #[cfg(target_arch = "wasm32")]
        {
            mapped.boxed_local()
        }
    }

    /// Sends a prompt and returns the response stream. The returned
    /// `ChatResponse` produces `StreamChunk` events until the turn ends.
    pub async fn chat(&self, content: impl Into<Content>) -> Result<ChatResponse> {
        // Subscribe BEFORE sending so the producer doesn't miss the first
        // step in the rare case the harness responds before we register.
        let steps = self.receive_steps();
        {
            let mut s = self.state.lock();
            s.turn_count = s.turn_count.saturating_add(1);
        }
        self.send(content.into()).await?;
        Ok(ChatResponse::new(steps, self.state.clone()))
    }
}

// =============================================================================
// ChatResponse
// =============================================================================

/// A streaming response from a single chat turn.
///
/// Multi-cursor: each call to [`ChatResponse::chunks`] returns an independent
/// cursor that replays from chunk zero. The upstream pull happens once.
pub struct ChatResponse {
    inner: Arc<ChatInner>,
}

struct ChatInner {
    state: Mutex<ChatBuf>,
    notify: Notify,
}

struct ChatBuf {
    chunks: Vec<StreamChunk>,
    done: bool,
    error: Option<String>,
}

impl ChatResponse {
    fn new(
        mut step_stream: crate::connections::StepStream,
        conv_state: Arc<Mutex<ConversationState>>,
    ) -> Self {
        let inner = Arc::new(ChatInner {
            state: Mutex::new(ChatBuf {
                chunks: Vec::new(),
                done: false,
                error: None,
            }),
            notify: Notify::new(),
        });
        let inner_clone = inner.clone();
        crate::runtime::spawn(async move {
            let mut emitted_text = String::new();
            while let Some(step) = step_stream.next().await {
                match step {
                    Ok(step) => {
                        let mut new_chunks = step_to_chunks(&step, emitted_text.len());
                        for chunk in &new_chunks {
                            if let StreamChunk::Text { text, .. } = chunk {
                                emitted_text.push_str(text);
                            }
                        }
                        if !new_chunks.is_empty() {
                            let mut buf = inner_clone.state.lock();
                            buf.chunks.append(&mut new_chunks);
                            drop(buf);
                            inner_clone.notify.notify_waiters();
                        }
                        if step.is_terminal_response() {
                            let mut s = conv_state.lock();
                            let final_text = if !step.content.is_empty() {
                                step.content.clone()
                            } else {
                                emitted_text.clone()
                            };
                            if !final_text.is_empty() {
                                s.last_response = Some(final_text);
                            }
                            break;
                        }
                    }
                    Err(e) => {
                        let mut buf = inner_clone.state.lock();
                        buf.error = Some(e.to_string());
                        buf.done = true;
                        drop(buf);
                        inner_clone.notify.notify_waiters();
                        return;
                    }
                }
            }
            let mut buf = inner_clone.state.lock();
            buf.done = true;
            drop(buf);
            inner_clone.notify.notify_waiters();
        });

        Self { inner }
    }

    /// A fresh cursor that replays every chunk from the start. Multiple
    /// cursors can be live at once and advance independently.
    pub fn chunks(&self) -> ChatCursor {
        ChatCursor {
            inner: self.inner.clone(),
            pos: 0,
            notify: None,
        }
    }

    /// Filtered cursor that yields only conversational text deltas.
    pub fn text_stream(&self) -> futures_util::stream::BoxStream<'static, Result<String>> {
        self.chunks()
            .filter_map(|res| async move {
                match res {
                    Ok(StreamChunk::Text { text, .. }) => Some(Ok(text)),
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                }
            })
            .boxed()
    }

    /// Filtered cursor that yields only thought (reasoning) deltas.
    pub fn thoughts(&self) -> futures_util::stream::BoxStream<'static, Result<String>> {
        self.chunks()
            .filter_map(|res| async move {
                match res {
                    Ok(StreamChunk::Thought { text, .. }) => Some(Ok(text)),
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                }
            })
            .boxed()
    }

    /// Filtered cursor that yields strongly-typed `ToolCall`s as the model
    /// dispatches them.
    pub fn tool_calls(&self) -> futures_util::stream::BoxStream<'static, Result<ToolCall>> {
        self.chunks()
            .filter_map(|res| async move {
                match res {
                    Ok(StreamChunk::ToolCall(t)) => Some(Ok(t)),
                    Ok(_) => None,
                    Err(e) => Some(Err(e)),
                }
            })
            .boxed()
    }

    /// Drain the stream and return the full concatenated text response.
    pub async fn text(&self) -> Result<String> {
        let mut out = String::new();
        let mut cursor = self.chunks();
        while let Some(res) = cursor.next().await {
            if let StreamChunk::Text { text, .. } = res? {
                out.push_str(&text);
            }
        }
        Ok(out)
    }

    /// Drain the stream and return every chunk in order.
    pub async fn resolve(&self) -> Result<Vec<StreamChunk>> {
        let mut cursor = self.chunks();
        let mut out = Vec::new();
        while let Some(res) = cursor.next().await {
            out.push(res?);
        }
        Ok(out)
    }
}

// =============================================================================
// Cursor
// =============================================================================

/// An independent cursor over a [`ChatResponse`]'s chunk buffer.
///
/// Implements [`Stream`] of `Result<StreamChunk>`. Multiple cursors
/// can be live concurrently and advance at different rates.
pub struct ChatCursor {
    inner: Arc<ChatInner>,
    pos: usize,
    notify: Option<Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
}

enum PollDecision {
    Yield(StreamChunk),
    Done,
    Error(String),
    Park,
}

impl Stream for ChatCursor {
    type Item = Result<StreamChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            // If we're waiting on a notification, finish that first.
            if let Some(fut) = self.notify.as_mut() {
                match fut.as_mut().poll(cx) {
                    Poll::Ready(()) => {
                        self.notify = None;
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }

            let snapshot = {
                let buf = self.inner.state.lock();
                if self.pos < buf.chunks.len() {
                    PollDecision::Yield(buf.chunks[self.pos].clone())
                } else if buf.done {
                    match &buf.error {
                        Some(e) => PollDecision::Error(e.clone()),
                        None => PollDecision::Done,
                    }
                } else {
                    PollDecision::Park
                }
            };
            match snapshot {
                PollDecision::Yield(chunk) => {
                    self.pos += 1;
                    return Poll::Ready(Some(Ok(chunk)));
                }
                PollDecision::Done => return Poll::Ready(None),
                PollDecision::Error(msg) => return Poll::Ready(Some(Err(Error::other(msg)))),
                PollDecision::Park => {}
            }

            // Park on the notification. Construct the future from a
            // 'static-bound Arc clone so it satisfies Send + 'static.
            let inner = self.inner.clone();
            self.notify = Some(Box::pin(async move {
                inner.notify.notified().await;
            }));
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

/// Convert one step into zero or more `StreamChunk`s. `text_emitted` is the
/// running tally of text BYTES this turn has yielded so far (the caller passes
/// `emitted_text.len()`); passing it in lets us recover the tail when a harness
/// emits the final content without preceding `content_delta`s.
fn step_to_chunks(step: &Step, text_emitted: usize) -> Vec<StreamChunk> {
    let mut out = Vec::new();
    if !step.thinking_delta.is_empty() {
        out.push(StreamChunk::Thought {
            step_index: step.step_index,
            text: step.thinking_delta.clone(),
        });
    }
    if !step.content_delta.is_empty() {
        out.push(StreamChunk::Text {
            step_index: step.step_index,
            text: step.content_delta.clone(),
        });
    } else if step.is_terminal_response() {
        // No delta was sent but `content` advanced — emit the un-emitted suffix.
        // `text_emitted` is a BYTE offset; use `str::get` so an offset that
        // doesn't land on a char boundary (a harness split a multibyte char
        // across deltas) degrades to a no-op instead of panicking on a bad
        // byte slice.
        if let Some(suffix) = step.content.get(text_emitted..) {
            if !suffix.is_empty() {
                out.push(StreamChunk::Text {
                    step_index: step.step_index,
                    text: suffix.to_string(),
                });
            }
        }
    }
    for tc in &step.tool_calls {
        out.push(StreamChunk::ToolCall(tc.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turn-terminating step carrying `content` and no delta — the path that
    /// triggers tail recovery (`content.get(text_emitted..)`).
    fn terminal_step(content: &str) -> Step {
        serde_json::from_value(serde_json::json!({
            "content": content,
            "is_complete_response": true,
        }))
        .expect("valid Step json")
    }

    fn recovered_text(chunks: &[StreamChunk]) -> String {
        chunks
            .iter()
            .filter_map(|c| match c {
                StreamChunk::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn recovery_does_not_panic_on_non_char_boundary_offset() {
        // "héllo": 'é' is two bytes, so byte offset 2 lands MID-char. The old
        // byte slice `content[2..]` panicked; recovery must now degrade safely
        // (no panic, and no corrupt partial-char suffix).
        let step = terminal_step("héllo");
        let chunks = step_to_chunks(&step, 2);
        assert_eq!(
            recovered_text(&chunks),
            "",
            "a non-char-boundary offset must emit no (corrupt) text suffix",
        );
    }

    #[test]
    fn recovery_emits_suffix_on_valid_boundary() {
        // Byte offset 1 is a valid boundary (after 'h'); suffix is "éllo".
        let step = terminal_step("héllo");
        assert_eq!(recovered_text(&step_to_chunks(&step, 1)), "éllo");
    }

    #[test]
    fn recovery_is_noop_when_everything_emitted() {
        let step = terminal_step("hi");
        assert_eq!(recovered_text(&step_to_chunks(&step, 2)), "");
    }
}
