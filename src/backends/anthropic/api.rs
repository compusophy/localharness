//! Async HTTPS client for the Anthropic Messages API.
//!
//! Single public type: [`AnthropicClient`]. Two methods:
//! [`stream_messages`][AnthropicClient::stream_messages] (SSE turn loop)
//! and [`messages`][AnthropicClient::messages] (non-streaming one-shot,
//! used by `start_subagent` / compaction summary).
//!
//! The SSE decoder ([`MessagesSseStream`]) delegates the wire-agnostic
//! frame-buffering skeleton to the shared `backends::sse` module (crate-private):
//! CRLF+LF-tolerant frame splitting, partial-chunk buffering. The only
//! Anthropic-specific piece is payload decoding — Anthropic frames are
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

use futures_core::Stream;
use futures_util::stream::StreamExt;
use reqwest::{Client, Url};

use crate::backends::anthropic::wire::{
    MessagesRequest, MessagesResponse, StreamEvent, ANTHROPIC_VERSION,
};
use crate::backends::sse::{ByteStream, SseFrameStream};
use crate::error::{Error, Result};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// HTTPS client for `api.anthropic.com` (or a credit-proxy base URL).
pub struct AnthropicClient {
    http: Client,
    api_key: Box<str>,
    key_provider: Option<crate::backends::KeyProvider>,
    base_url: Url,
    /// Extra headers attached to EVERY outbound request (e.g. an `X-PAYMENT`
    /// x402 authorization). Empty by default — a no-op.
    extra_headers: Vec<(String, String)>,
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
            key_provider: None,
            base_url: Url::parse(DEFAULT_BASE_URL).expect("default base url is valid"),
            extra_headers: Vec::new(),
        })
    }

    /// Attach extra headers to every outbound request (e.g. an `X-PAYMENT`
    /// x402 authorization). No-op when empty.
    pub fn with_extra_headers(mut self, headers: Vec<(String, String)>) -> Self {
        self.extra_headers = headers;
        self
    }

    /// Apply [`Self::extra_headers`] onto a request builder (no-op when empty).
    fn apply_extra_headers(&self, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        for (name, value) in &self.extra_headers {
            rb = rb.header(name.as_str(), value.as_str());
        }
        rb
    }

    /// Install a per-request key provider (see
    /// [`crate::backends::KeyProvider`]). The static key from
    /// [`new`][Self::new] becomes a fallback only.
    pub fn with_key_provider(mut self, provider: crate::backends::KeyProvider) -> Self {
        self.key_provider = Some(provider);
        self
    }

    /// The credential for the NEXT request: freshly minted by the provider
    /// when one is installed, else the static key.
    fn current_key(&self) -> String {
        match &self.key_provider {
            Some(p) => p(),
            None => self.api_key.to_string(),
        }
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
            .apply_extra_headers(
                self.http
                    .post(url)
                    .header("x-api-key", self.current_key())
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .header("content-type", "application/json"),
            )
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
            return Err(Error::http_status(
                status.as_u16(),
                format!("anthropic HTTP {status}: {body}"),
            ));
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
            .apply_extra_headers(
                self.http
                    .post(url)
                    .header("x-api-key", self.current_key())
                    .header("anthropic-version", ANTHROPIC_VERSION)
                    .header("content-type", "application/json")
                    .header("accept", "text/event-stream"),
            )
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::other(format!("anthropic POST: {e}")))?;

        let debug_sse = std::env::var("LH_DEBUG_SSE").is_ok();
        if debug_sse {
            eprintln!(
                "[anthropic resp] status={} content-type={:?}",
                response.status(),
                response.headers().get("content-type"),
            );
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            if debug_sse {
                eprintln!("[anthropic ERROR] HTTP {status}: {body}");
            }
            return Err(Error::http_status(
                status.as_u16(),
                format!("anthropic HTTP {status}: {body}"),
            ));
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

/// Decodes an Anthropic Messages SSE byte stream into [`StreamEvent`]s.
///
/// Frame format: `event: <name>\ndata: <json>\n\n`. The wire-agnostic frame
/// splitting (CRLF+LF-tolerant boundaries — browser fetch surfaces CRLF, the
/// wasm gotcha — partial-chunk buffering, EOF flush of a final unterminated
/// frame) is the shared `SseFrameStream` (crate-private); this type only decodes each
/// `data:` payload as a [`StreamEvent`], ignoring the `event:` line (the
/// JSON's own `"type"` field carries the same name). No `[DONE]` sentinel —
/// Anthropic ends streams with a `message_stop` event.
pub struct MessagesSseStream {
    frames: SseFrameStream,
}

impl MessagesSseStream {
    /// Wrap a raw byte stream. Public so unit tests can feed canned bytes.
    pub fn new(upstream: ByteStream) -> Self {
        Self {
            frames: SseFrameStream::new(upstream, None, "anthropic"),
        }
    }
}

impl Stream for MessagesSseStream {
    type Item = Result<StreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.frames).poll_next(cx) {
            Poll::Ready(Some(Ok(payload))) => Poll::Ready(Some(decode_event(&payload))),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
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
    use bytes::Bytes;
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

    /// Drain a stream into the list of decoded event variant tags (cheap,
    /// order-preserving), surfacing decode errors as the literal "ERR".
    async fn collect_kinds(mut s: MessagesSseStream) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(ev) = s.next().await {
            match ev {
                Ok(StreamEvent::MessageStart { .. }) => out.push("message_start".into()),
                Ok(StreamEvent::ContentBlockStart { .. }) => out.push("content_block_start".into()),
                Ok(StreamEvent::ContentBlockDelta { .. }) => out.push("content_block_delta".into()),
                Ok(StreamEvent::ContentBlockStop { .. }) => out.push("content_block_stop".into()),
                Ok(StreamEvent::MessageDelta { .. }) => out.push("message_delta".into()),
                Ok(StreamEvent::MessageStop) => out.push("message_stop".into()),
                Ok(StreamEvent::Ping) => out.push("ping".into()),
                Ok(StreamEvent::Error { .. }) => out.push("error".into()),
                Ok(StreamEvent::Unknown) => out.push("unknown".into()),
                Err(_) => out.push("ERR".into()),
            }
        }
        out
    }

    /// REGRESSION: SSE permits the stream's LAST event to omit the trailing
    /// blank line (WHATWG event-stream rule). The Gemini decoder handles this
    /// with `take_remaining`; the Anthropic decoder originally only re-ran
    /// `take_frame` at EOF, which requires a blank-line boundary — so a final
    /// `data: {...}` with no trailing `\n\n` was silently dropped. For
    /// Anthropic that frame can be the `message_delta` carrying `stop_reason`
    /// + cumulative `output_tokens`, OR the `message_stop`.
    #[tokio::test]
    async fn flushes_final_frame_without_trailing_blank_line() {
        let frames = [
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n".as_bytes(),
            // Final frame: complete event, NO trailing blank line.
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}".as_bytes(),
        ];
        let s = MessagesSseStream::new(bytes_from(&frames));
        let kinds = collect_kinds(s).await;
        assert_eq!(
            kinds,
            vec!["content_block_stop", "message_delta"],
            "the final unterminated frame must still be decoded, not dropped at EOF"
        );
    }

    /// A single complete frame with NO terminator at all — the whole stream is
    /// one EOF flush.
    #[tokio::test]
    async fn flushes_single_unterminated_frame() {
        let frames =
            ["event: message_stop\ndata: {\"type\":\"message_stop\"}".as_bytes()];
        let s = MessagesSseStream::new(bytes_from(&frames));
        assert_eq!(collect_kinds(s).await, vec!["message_stop"]);
    }

    /// Final frame terminated by a single `\n` (not a blank line) at EOF — the
    /// SSE field is complete but the event-terminating blank line is missing.
    #[tokio::test]
    async fn flushes_final_frame_with_single_newline() {
        let frames =
            ["event: message_stop\ndata: {\"type\":\"message_stop\"}\n".as_bytes()];
        let s = MessagesSseStream::new(bytes_from(&frames));
        assert_eq!(collect_kinds(s).await, vec!["message_stop"]);
    }

    /// A `:`-comment / keepalive line and a bare blank-line heartbeat must not
    /// produce a phantom event (empty `data:` payload → skipped, not decoded
    /// as an error). Anthropic sends periodic `event: ping` but a raw `:`
    /// comment is also legal SSE.
    #[tokio::test]
    async fn skips_comment_and_blank_heartbeat_frames() {
        let frames = [
            ": this is an SSE comment / keepalive\n\n".as_bytes(),
            "\n".as_bytes(), // bare blank-line heartbeat (no data:)
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_bytes(),
        ];
        let s = MessagesSseStream::new(bytes_from(&frames));
        assert_eq!(
            collect_kinds(s).await,
            vec!["message_stop"],
            "comment + blank-line frames must be skipped, not surfaced as events/errors"
        );
    }

    /// A multi-line `data:` field (two `data:` lines in one frame) is
    /// newline-joined before JSON decode — the SSE spec way to carry a payload
    /// containing a newline. (Anthropic doesn't currently split JSON this way,
    /// but the decoder must follow the spec.)
    #[tokio::test]
    async fn joins_multiline_data_field() {
        // {"type":"message_stop"} split across two data: lines.
        let frames =
            ["event: message_stop\ndata: {\"type\":\ndata: \"message_stop\"}\n\n".as_bytes()];
        let s = MessagesSseStream::new(bytes_from(&frames));
        assert_eq!(collect_kinds(s).await, vec!["message_stop"]);
    }

    /// A multibyte UTF-8 character ("é" = 0xC3 0xA9) split across two network
    /// chunks must not corrupt the decoded text. The frame stays buffered
    /// until the blank-line terminator, by which point the char is whole.
    #[tokio::test]
    async fn multibyte_char_split_across_chunks() {
        let full = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"é\"}}\n\n";
        let raw = full.as_bytes();
        let split_at = raw.iter().position(|&b| b == 0xC3).unwrap();
        let (head, tail) = raw.split_at(split_at + 1); // 0xC3 lead byte in head
        let mut s = MessagesSseStream::new(bytes_from(&[head, tail]));
        let ev = s.next().await.unwrap().unwrap();
        match ev {
            StreamEvent::ContentBlockDelta {
                delta: BlockDelta::TextDelta { text },
                ..
            } => assert_eq!(text, "é"),
            other => panic!("expected a text_delta carrying é, got {other:?}"),
        }
        assert!(s.next().await.is_none());
    }

    /// A genuinely malformed JSON `data:` payload surfaces as an `Err` item
    /// (not a panic, not a silent drop).
    #[tokio::test]
    async fn malformed_json_yields_error_not_panic() {
        let frames = ["event: message_delta\ndata: {not valid json}\n\n".as_bytes()];
        let mut s = MessagesSseStream::new(bytes_from(&frames));
        let item = s.next().await.unwrap();
        assert!(item.is_err(), "malformed JSON must be an Err, got {item:?}");
    }

    /// Accumulate a multi-block stream EXACTLY as `loop.rs::run_turn` does —
    /// text deltas across TWO separate text blocks (indices 0 and 2) all fold
    /// into one text string; `input_json_delta` fragments route to the right
    /// tool block by `index`; and a `tool_use` block with ZERO input deltas
    /// (no args) yields an empty fragment (the loop resolves that to `{}`).
    /// This locks the per-index routing contract the loop relies on.
    #[tokio::test]
    async fn accumulates_multiple_blocks_with_empty_and_filled_tool_args() {
        let frames: Vec<&[u8]> = vec![
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"usage\":{\"input_tokens\":5,\"output_tokens\":1}}}\n\n".as_bytes(),
            // Block 0: text.
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".as_bytes(),
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"foo \"}}\n\n".as_bytes(),
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n".as_bytes(),
            // Block 1: tool_use with NO input deltas (empty args).
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_empty\",\"name\":\"list_subdomains\"}}\n\n".as_bytes(),
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n".as_bytes(),
            // Block 2: more text — must concat with block 0's text.
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n".as_bytes(),
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"bar\"}}\n\n".as_bytes(),
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":2}\n\n".as_bytes(),
            // Block 3: tool_use WITH args split across two fragments.
            "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_full\",\"name\":\"view_file\"}}\n\n".as_bytes(),
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":3,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n".as_bytes(),
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":3,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.rs\\\"}\"}}\n\n".as_bytes(),
            "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":3}\n\n".as_bytes(),
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":20}}\n\n".as_bytes(),
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_bytes(),
        ];
        let mut s = MessagesSseStream::new(bytes_from(&frames));

        let mut text = String::new();
        // index → (id, name, concatenated args)
        let mut tools: std::collections::BTreeMap<u32, (String, String, String)> =
            std::collections::BTreeMap::new();
        while let Some(ev) = s.next().await {
            match ev.unwrap() {
                StreamEvent::ContentBlockStart {
                    index,
                    content_block: Block::ToolUse { id, name, .. },
                } => {
                    tools.insert(index, (id, name, String::new()));
                }
                StreamEvent::ContentBlockDelta { index, delta } => match delta {
                    BlockDelta::TextDelta { text: t } => text.push_str(&t),
                    BlockDelta::InputJsonDelta { partial_json } => {
                        if let Some(e) = tools.get_mut(&index) {
                            e.2.push_str(&partial_json);
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        }

        assert_eq!(text, "foo bar", "text from blocks 0 and 2 concatenates");
        // Empty-args tool: no fragments → empty string (loop → {}).
        let empty = &tools[&1];
        assert_eq!(empty.0, "toolu_empty");
        assert_eq!(empty.1, "list_subdomains");
        assert_eq!(empty.2, "", "no input deltas → empty args fragment");
        // Filled tool: fragments concatenate into valid JSON, no bleed from
        // the empty block.
        let full = &tools[&3];
        assert_eq!(full.0, "toolu_full");
        let parsed: serde_json::Value = serde_json::from_str(&full.2).unwrap();
        assert_eq!(parsed["path"], "a.rs");
    }
}
