//! Async HTTPS client for the Anthropic Messages API.
//!
//! Single public type: [`AnthropicClient`]. Two methods:
//! [`stream_messages`][AnthropicClient::stream_messages] (SSE turn loop)
//! and [`messages`][AnthropicClient::messages] (non-streaming one-shot,
//! used by `start_subagent` / compaction summary).
//!
//! The SSE decoder ([`MessagesSseStream`]) reuses the wire-agnostic
//! frame-buffering skeleton from the Gemini backend: CRLF+LF-tolerant
//! `take_frame`, partial-chunk buffering. The only Anthropic-specific
//! piece is payload decoding — Anthropic frames are
//! `event: <name>\ndata: <json>\n\n`; we ignore the `event:` line and
//! decode the `data:` JSON as a [`StreamEvent`] (the JSON carries the
//! event name in its `"type"` field, so the `event:` line is redundant).
//!
//! The API key is held as a `Box<str>` so it never appears in `Debug`
//! output and is dropped reliably when the client is dropped.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use futures_util::stream::StreamExt;
use reqwest::{Client, Url};
use tracing::trace;

use crate::backends::anthropic::wire::{
    MessagesRequest, MessagesResponse, StreamEvent, ANTHROPIC_VERSION,
};
use crate::error::{Error, Result};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// HTTPS client for `api.anthropic.com` (or a credit-proxy base URL).
pub struct AnthropicClient {
    http: Client,
    api_key: Box<str>,
    base_url: Url,
}

impl fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("base_url", &self.base_url.as_str())
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl AnthropicClient {
    /// Build a client for the given API key, talking directly to
    /// `api.anthropic.com`.
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let builder =
            Client::builder().user_agent(concat!("localharness/", env!("CARGO_PKG_VERSION")));
        // reqwest's wasm builder doesn't have .timeout() — browser fetch
        // controls timeouts, not the client config.
        #[cfg(not(target_arch = "wasm32"))]
        let builder = builder.timeout(DEFAULT_TIMEOUT);
        let http = builder
            .build()
            .map_err(|e| Error::other(format!("reqwest client build: {e}")))?;
        Ok(Self {
            http,
            api_key: api_key.into().into_boxed_str(),
            base_url: Url::parse(DEFAULT_BASE_URL).expect("default base url is valid"),
        })
    }

    /// Override the base URL (e.g. the future localharness credit proxy,
    /// or a test server). In credits mode the `api_key` carries the proxy
    /// auth token rather than a raw Anthropic key.
    pub fn with_base_url(mut self, url: Url) -> Self {
        self.base_url = url;
        self
    }

    fn messages_url(&self) -> Result<Url> {
        self.base_url
            .join("v1/messages")
            .map_err(|e| Error::other(format!("invalid messages url: {e}")))
    }

    /// Non-streaming `POST /v1/messages`. Used for one-shot completions
    /// (subagent, compaction summary).
    pub async fn messages(&self, req: &MessagesRequest) -> Result<MessagesResponse> {
        let url = self.messages_url()?;
        // Force non-stream on the one-shot path regardless of caller flag.
        let mut body = req.clone();
        body.stream = false;
        let response = self
            .http
            .post(url)
            .header("x-api-key", self.api_key.as_ref())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::other(format!("anthropic POST: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(Error::other(format!("anthropic HTTP {status}: {body}")));
        }

        response
            .json::<MessagesResponse>()
            .await
            .map_err(|e| Error::other(format!("anthropic JSON: {e}")))
    }

    /// Streaming `POST /v1/messages` (`stream: true`). Returns a
    /// [`MessagesSseStream`] yielding one [`StreamEvent`] per SSE frame.
    pub async fn stream_messages(&self, req: &MessagesRequest) -> Result<MessagesSseStream> {
        let url = self.messages_url()?;
        let mut body = req.clone();
        body.stream = true;
        let response = self
            .http
            .post(url)
            .header("x-api-key", self.api_key.as_ref())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::other(format!("anthropic POST: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(Error::other(format!("anthropic HTTP {status}: {body}")));
        }

        let byte_stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| Error::other(format!("anthropic chunk read: {e}"))));
        Ok(MessagesSseStream::new(Box::pin(byte_stream)))
    }
}

// =============================================================================
// SSE stream
// =============================================================================

// On native, the SSE byte stream must be `Send` so it can move into a
// `tokio::spawn`'d turn. On wasm32, browser fetch streams aren't Send —
// fine, everything single-threads through `spawn_local`.
#[cfg(not(target_arch = "wasm32"))]
type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + 'static>>;

/// Decodes an Anthropic Messages SSE byte stream into [`StreamEvent`]s.
///
/// Frame format: `event: <name>\ndata: <json>\n\n`. A frame ends at the
/// first blank line — `\n\n` (LF) or `\r\n\r\n` (CRLF; browser fetch
/// surfaces CRLF, the wasm gotcha). We concatenate every `data:` line in
/// a frame and decode the result as a [`StreamEvent`], ignoring the
/// `event:` line (the JSON's own `"type"` field carries the same name).
pub struct MessagesSseStream {
    upstream: ByteStream,
    buffer: BytesMut,
    done: bool,
}

impl MessagesSseStream {
    /// Wrap a raw byte stream. Public so unit tests can feed canned bytes.
    pub fn new(upstream: ByteStream) -> Self {
        Self {
            upstream,
            buffer: BytesMut::with_capacity(8 * 1024),
            done: false,
        }
    }

    /// Pull a complete frame's `data:` payload from `self.buffer` if one
    /// is fully buffered. Returns `None` when no frame boundary is present
    /// yet (partial chunk). Boundary = first blank line, CRLF or LF.
    fn take_frame(&mut self) -> Option<Vec<u8>> {
        let bytes = &self.buffer[..];
        let mut i = 0;
        while i < bytes.len() {
            // Prefer the longer CRLF boundary so we consume it whole.
            if i + 3 < bytes.len()
                && bytes[i] == b'\r'
                && bytes[i + 1] == b'\n'
                && bytes[i + 2] == b'\r'
                && bytes[i + 3] == b'\n'
            {
                let frame = self.buffer.split_to(i + 4);
                return Some(extract_data_payload(&frame));
            }
            if i + 1 < bytes.len() && bytes[i] == b'\n' && bytes[i + 1] == b'\n' {
                let frame = self.buffer.split_to(i + 2);
                return Some(extract_data_payload(&frame));
            }
            i += 1;
        }
        None
    }
}

impl Stream for MessagesSseStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.done {
                // Flush any remaining buffered frame even after upstream EOF.
                if let Some(payload) = self.take_frame() {
                    if payload.is_empty() {
                        continue;
                    }
                    return Poll::Ready(Some(decode_event(&payload)));
                }
                return Poll::Ready(None);
            }

            if let Some(payload) = self.take_frame() {
                if payload.is_empty() {
                    continue;
                }
                return Poll::Ready(Some(decode_event(&payload)));
            }

            match self.upstream.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    trace!(len = bytes.len(), "anthropic sse bytes");
                    self.buffer.extend_from_slice(&bytes);
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => self.done = true,
            }
        }
    }
}

fn extract_data_payload(frame: &[u8]) -> Vec<u8> {
    // Concatenate every `data:` line in this frame, trimming the prefix
    // and any single leading space. `event:`/`id:`/`retry:` lines ignored.
    let mut out = Vec::with_capacity(frame.len());
    let text = std::str::from_utf8(frame).unwrap_or("");
    for line in text.split('\n') {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            if !out.is_empty() {
                out.push(b'\n');
            }
            out.extend_from_slice(rest.as_bytes());
        }
    }
    out
}

fn decode_event(payload: &[u8]) -> Result<StreamEvent> {
    serde_json::from_slice::<StreamEvent>(payload).map_err(|e| {
        Error::other(format!(
            "anthropic sse decode: {e}; payload: {}",
            String::from_utf8_lossy(payload)
        ))
    })
}

/// Re-export an `Arc<AnthropicClient>` for ergonomic cloning into spawned
/// tasks.
pub type SharedClient = Arc<AnthropicClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::anthropic::wire::{Block, BlockDelta, StopReason};
    use futures_util::stream;

    fn bytes_from(parts: &[&[u8]]) -> ByteStream {
        let owned: Vec<Bytes> = parts.iter().map(|b| Bytes::copy_from_slice(b)).collect();
        Box::pin(stream::iter(owned.into_iter().map(Ok)))
    }

    /// The canonical full streaming sequence the task spec asks for:
    /// message_start → content_block_start(text) → 2 text_deltas →
    /// content_block_stop → content_block_start(tool_use) → 2
    /// input_json_delta fragments → content_block_stop → message_delta
    /// (stop_reason: tool_use) → message_stop. As a sequence of complete
    /// SSE frames (one `event:`/`data:`/blank-line group each).
    fn canonical_frames() -> Vec<Vec<u8>> {
        let raw = [
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"model\":\"claude-haiku-4-5-20251001\",\"role\":\"assistant\",\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Read\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ing.\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_x\",\"name\":\"view_file\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"main.rs\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":33}}\n\n",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ];
        raw.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    /// Drive the stream to completion and assert the decoded events
    /// reconstruct the right text, a tool_use with concatenated args, and
    /// the terminal stop reason + usage. Shared by the three framing
    /// variants below.
    async fn assert_canonical(mut s: MessagesSseStream) {
        let mut text = String::new();
        let mut tool_id = String::new();
        let mut tool_name = String::new();
        let mut tool_args = String::new();
        let mut stop: Option<StopReason> = None;
        let mut out_tokens: Option<i32> = None;
        let mut in_tokens: Option<i32> = None;
        let mut saw_stop_event = false;

        while let Some(ev) = s.next().await {
            match ev.unwrap() {
                StreamEvent::MessageStart { message } => {
                    in_tokens = message.usage.and_then(|u| u.input_tokens);
                }
                StreamEvent::ContentBlockStart {
                    content_block: Block::ToolUse { id, name, .. },
                    ..
                } => {
                    tool_id = id;
                    tool_name = name;
                }
                StreamEvent::ContentBlockDelta { delta, .. } => match delta {
                    BlockDelta::TextDelta { text: t } => text.push_str(&t),
                    BlockDelta::InputJsonDelta { partial_json } => tool_args.push_str(&partial_json),
                    _ => {}
                },
                StreamEvent::MessageDelta { delta, usage } => {
                    stop = delta.stop_reason;
                    out_tokens = usage.and_then(|u| u.output_tokens);
                }
                StreamEvent::MessageStop => saw_stop_event = true,
                _ => {}
            }
        }

        assert_eq!(text, "Reading.");
        assert_eq!(tool_id, "toolu_x");
        assert_eq!(tool_name, "view_file");
        // The two fragments concatenate to valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&tool_args).unwrap();
        assert_eq!(parsed["path"], "main.rs");
        assert_eq!(stop, Some(StopReason::ToolUse));
        assert_eq!(out_tokens, Some(33));
        assert_eq!(in_tokens, Some(12));
        assert!(saw_stop_event);
    }

    #[tokio::test]
    async fn decodes_canonical_sequence_one_chunk() {
        // Every frame concatenated into a single byte chunk.
        let blob: Vec<u8> = canonical_frames().concat();
        let s = MessagesSseStream::new(bytes_from(&[&blob]));
        assert_canonical(s).await;
    }

    #[tokio::test]
    async fn decodes_canonical_sequence_split_mid_frame() {
        // Concatenate all frames, then re-chop into chunks that fall in
        // the MIDDLE of frames (every 17 bytes), exercising partial-frame
        // buffering across chunk boundaries.
        let blob: Vec<u8> = canonical_frames().concat();
        let chunks: Vec<&[u8]> = blob.chunks(17).collect();
        let s = MessagesSseStream::new(bytes_from(&chunks));
        assert_canonical(s).await;
    }

    #[tokio::test]
    async fn decodes_canonical_sequence_crlf() {
        // Browser fetch surfaces SSE with CRLF frame separators (the wasm
        // gotcha). Rewrite every "\n" to "\r\n" and re-chop mid-frame.
        let blob: Vec<u8> = canonical_frames().concat();
        let crlf: Vec<u8> = String::from_utf8(blob)
            .unwrap()
            .replace('\n', "\r\n")
            .into_bytes();
        let chunks: Vec<&[u8]> = crlf.chunks(13).collect();
        let s = MessagesSseStream::new(bytes_from(&chunks));
        assert_canonical(s).await;
    }

    #[tokio::test]
    async fn ignores_ping_and_unknown_events() {
        let frames = [
            "event: ping\ndata: {\"type\":\"ping\"}\n\n".as_bytes(),
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_bytes(),
        ];
        let mut s = MessagesSseStream::new(bytes_from(&frames));
        let first = s.next().await.unwrap().unwrap();
        assert_eq!(first, StreamEvent::Ping);
        let second = s.next().await.unwrap().unwrap();
        assert_eq!(second, StreamEvent::MessageStop);
        assert!(s.next().await.is_none());
    }
}
