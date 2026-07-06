//! Async HTTPS client for the OpenAI Chat Completions API.
//!
//! Single public type: [`OpenAiClient`]. Two methods:
//! [`stream_chat`][OpenAiClient::stream_chat] (SSE turn loop) and
//! [`chat`][OpenAiClient::chat] (non-streaming one-shot, used by compaction).
//!
//! The SSE decoder ([`ChatSseStream`]) delegates the wire-agnostic
//! frame-buffering skeleton to the shared `backends::sse` module
//! (crate-private): CRLF+LF-tolerant frame splitting, partial-chunk buffering,
//! `data: [DONE]` sentinel termination. The only OpenAI-specific piece is
//! payload decoding — each frame is a `data: <json>` line decoded as a
//! [`ChatChunk`].
//!
//! The API key is held as a `Box<str>` so it never appears in `Debug` output
//! and is dropped reliably when the client is dropped. Auth is the standard
//! `Authorization: Bearer <key>` header; the credit-proxy path overrides the
//! base URL and carries the proxy auth token in that same header.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use futures_core::Stream;
use futures_util::stream::StreamExt;
use reqwest::{Client, Url};

use crate::backends::openai::wire::{ChatChunk, ChatRequest, ChatResponse};
use crate::backends::sse::{ByteStream, SseFrameStream};
use crate::error::{Error, Result};

const DEFAULT_BASE_URL: &str = "https://api.openai.com";
/// The `data: [DONE]` stream-terminating sentinel.
const DONE_SENTINEL: &[u8] = b"[DONE]";
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// HTTPS client for `api.openai.com` (or a credit-proxy base URL).
pub struct OpenAiClient {
    http: Client,
    api_key: Box<str>,
    key_provider: Option<crate::backends::KeyProvider>,
    base_url: Url,
}

impl fmt::Debug for OpenAiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiClient")
            .field("base_url", &self.base_url.as_str())
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl OpenAiClient {
    /// Build a client for the given API key, talking directly to
    /// `api.openai.com`.
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
        })
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

    /// Override the base URL (e.g. the localharness credit proxy, which
    /// already forwards `/v1/chat/completions`, or a test server). In credits
    /// mode the `api_key` carries the proxy auth token rather than a raw
    /// OpenAI key.
    pub fn with_base_url(mut self, url: Url) -> Self {
        self.base_url = url;
        self
    }

    fn completions_url(&self) -> Result<Url> {
        self.base_url
            .join("v1/chat/completions")
            .map_err(|e| Error::other(format!("invalid completions url: {e}")))
    }

    /// Non-streaming `POST /v1/chat/completions`. Used for one-shot
    /// completions (the compaction summary).
    pub async fn chat(&self, req: &ChatRequest) -> Result<ChatResponse> {
        let url = self.completions_url()?;
        // Force non-stream on the one-shot path regardless of caller flag.
        let mut body = req.clone();
        body.stream = false;
        body.stream_options = None;
        let response = self
            .http
            .post(url)
            .bearer_auth(self.current_key())
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::transport(format!("openai POST: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(Error::http_status(
                status.as_u16(),
                format!("openai HTTP {status}: {body}"),
            ));
        }

        response
            .json::<ChatResponse>()
            .await
            .map_err(|e| Error::decode("openai JSON", e.to_string()))
    }

    /// Streaming `POST /v1/chat/completions` (`stream: true`). Returns a
    /// [`ChatSseStream`] yielding one [`ChatChunk`] per SSE frame.
    pub async fn stream_chat(&self, req: &ChatRequest) -> Result<ChatSseStream> {
        let url = self.completions_url()?;
        let mut body = req.clone();
        body.stream = true;
        let response = self
            .http
            .post(url)
            .bearer_auth(self.current_key())
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::transport(format!("openai POST: {e}")))?;

        let debug_sse = std::env::var("LH_DEBUG_SSE").is_ok();
        if debug_sse {
            eprintln!(
                "[openai resp] status={} content-type={:?}",
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
                eprintln!("[openai ERROR] HTTP {status}: {body}");
            }
            return Err(Error::http_status(
                status.as_u16(),
                format!("openai HTTP {status}: {body}"),
            ));
        }

        let byte_stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| Error::transport(format!("openai chunk read: {e}"))));
        Ok(ChatSseStream::new(Box::pin(byte_stream)))
    }
}

// =============================================================================
// SSE stream
// =============================================================================

/// Decodes an OpenAI chat-completions SSE byte stream into [`ChatChunk`]s.
///
/// Frame format: `data: <json>` (no `event:` line), with the stream terminated
/// by a literal `data: [DONE]`. The wire-agnostic frame splitting (CRLF+LF
/// boundaries — browser fetch surfaces CRLF, the wasm gotcha — partial-chunk
/// buffering, `[DONE]` sentinel, EOF flush of a final unterminated frame) is
/// the shared `SseFrameStream`; this type only decodes each `data:` payload as
/// a [`ChatChunk`].
pub struct ChatSseStream {
    frames: SseFrameStream,
}

impl ChatSseStream {
    /// Wrap a raw byte stream. Public so unit tests can feed canned bytes.
    pub fn new(upstream: ByteStream) -> Self {
        Self {
            frames: SseFrameStream::new(upstream, Some(DONE_SENTINEL), "openai"),
        }
    }
}

impl Stream for ChatSseStream {
    type Item = Result<ChatChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.frames).poll_next(cx) {
            Poll::Ready(Some(Ok(payload))) => Poll::Ready(Some(decode_chunk(&payload))),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn decode_chunk(payload: &[u8]) -> Result<ChatChunk> {
    serde_json::from_slice::<ChatChunk>(payload).map_err(|e| {
        Error::decode(
            "openai sse decode",
            format!("{e}; payload: {}", String::from_utf8_lossy(payload)),
        )
    })
}

/// Re-export an `Arc<OpenAiClient>` for ergonomic cloning into spawned tasks.
pub type SharedClient = Arc<OpenAiClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::openai::wire::FinishReason;
    use bytes::Bytes;
    use futures_util::stream;

    fn bytes_from(parts: &[&[u8]]) -> ByteStream {
        let owned: Vec<Bytes> = parts.iter().map(|b| Bytes::copy_from_slice(b)).collect();
        Box::pin(stream::iter(owned.into_iter().map(Ok)))
    }

    /// The canonical streaming sequence: a couple of text deltas, a tool_call
    /// streamed as INDEX-KEYED fragments (id+name on the first, args split
    /// across two more), then a terminal chunk with finish_reason + usage, then
    /// the `[DONE]` sentinel. As complete `data:`-line SSE frames.
    fn canonical_frames() -> Vec<Vec<u8>> {
        let raw = [
            "data: {\"id\":\"c1\",\"model\":\"gpt-5-nano\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Read\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ing.\"}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_x\",\"type\":\"function\",\"function\":{\"name\":\"view_file\",\"arguments\":\"\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"main.rs\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}],\"usage\":{\"prompt_tokens\":12,\"completion_tokens\":33,\"total_tokens\":45}}\n\n",
            "data: [DONE]\n\n",
        ];
        raw.iter().map(|s| s.as_bytes().to_vec()).collect()
    }

    /// Drive the stream to completion and assert the decoded chunks reconstruct
    /// the right text, a tool_call with concatenated args, and the terminal
    /// finish_reason + usage. Shared by the framing variants below.
    async fn assert_canonical(mut s: ChatSseStream) {
        use std::collections::BTreeMap;
        let mut text = String::new();
        // index → (id, name, concatenated args)
        let mut tools: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
        let mut finish: Option<FinishReason> = None;
        let mut completion_tokens: Option<i32> = None;

        while let Some(chunk) = s.next().await {
            let chunk = chunk.unwrap();
            for choice in &chunk.choices {
                if let Some(t) = &choice.delta.content {
                    text.push_str(t);
                }
                for tc in &choice.delta.tool_calls {
                    let entry = tools.entry(tc.index).or_default();
                    if let Some(id) = &tc.id {
                        entry.0 = id.clone();
                    }
                    if let Some(f) = &tc.function {
                        if let Some(name) = &f.name {
                            entry.1 = name.clone();
                        }
                        if let Some(args) = &f.arguments {
                            entry.2.push_str(args);
                        }
                    }
                }
                if let Some(fr) = choice.finish_reason {
                    finish = Some(fr);
                }
            }
            if let Some(u) = chunk.usage {
                completion_tokens = u.completion_tokens;
            }
        }

        assert_eq!(text, "Reading.");
        let call = &tools[&0];
        assert_eq!(call.0, "call_x");
        assert_eq!(call.1, "view_file");
        // The fragments concatenate to valid JSON.
        let parsed: serde_json::Value = serde_json::from_str(&call.2).unwrap();
        assert_eq!(parsed["path"], "main.rs");
        assert_eq!(finish, Some(FinishReason::ToolCalls));
        assert_eq!(completion_tokens, Some(33));
    }

    #[tokio::test]
    async fn decodes_canonical_sequence_one_chunk() {
        let blob: Vec<u8> = canonical_frames().concat();
        let s = ChatSseStream::new(bytes_from(&[&blob]));
        assert_canonical(s).await;
    }

    #[tokio::test]
    async fn decodes_canonical_sequence_split_mid_frame() {
        // Re-chop into chunks that fall in the MIDDLE of frames, exercising
        // partial-frame buffering across chunk boundaries.
        let blob: Vec<u8> = canonical_frames().concat();
        let chunks: Vec<&[u8]> = blob.chunks(19).collect();
        let s = ChatSseStream::new(bytes_from(&chunks));
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
        let chunks: Vec<&[u8]> = crlf.chunks(11).collect();
        let s = ChatSseStream::new(bytes_from(&chunks));
        assert_canonical(s).await;
    }

    /// The `[DONE]` sentinel terminates the stream and drops anything buffered
    /// after it.
    #[tokio::test]
    async fn done_sentinel_terminates_stream() {
        let frames = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n".as_bytes(),
            "data: [DONE]\n\n".as_bytes(),
            // Anything after [DONE] must be dropped, not decoded.
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"LEAK\"}}]}\n\n".as_bytes(),
        ];
        let mut s = ChatSseStream::new(bytes_from(&frames));
        let first = s.next().await.unwrap().unwrap();
        assert_eq!(first.choices[0].delta.content.as_deref(), Some("hi"));
        assert!(s.next().await.is_none(), "[DONE] must terminate the stream");
    }

    /// A genuinely malformed JSON `data:` payload surfaces as an `Err` item
    /// (not a panic, not a silent drop).
    #[tokio::test]
    async fn malformed_json_yields_error_not_panic() {
        let frames = ["data: {not valid json}\n\n".as_bytes()];
        let mut s = ChatSseStream::new(bytes_from(&frames));
        let item = s.next().await.unwrap();
        assert!(item.is_err(), "malformed JSON must be an Err, got {item:?}");
    }
}
