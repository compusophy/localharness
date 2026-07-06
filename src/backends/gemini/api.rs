//! Async HTTPS client for the Gemini REST API.
//!
//! Single public type: [`GeminiClient`]. One useful method:
//! [`stream_generate`][GeminiClient::stream_generate], which posts a
//! [`crate::backends::gemini::wire::GenerateContentRequest`]
//! and returns a `Stream` of decoded SSE chunks.
//!
//! The API key is held as a `Box<str>` so it never appears in `Debug`
//! output (we implement `Debug` manually) and is dropped reliably when
//! the client is dropped.

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

use futures_core::Stream;
use futures_util::stream::StreamExt;
use reqwest::{Client, Url};

use crate::backends::gemini::wire::{GenerateChunk, GenerateContentRequest};
use crate::backends::sse::{ByteStream, SseFrameStream};
use crate::error::{Error, Result};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Per-request API-key provider (shared across backends — see
/// [`crate::backends::KeyProvider`]).
pub use crate::backends::KeyProvider;

pub struct GeminiClient {
    http: Client,
    api_key: Box<str>,
    key_provider: Option<KeyProvider>,
    base_url: Url,
    /// Extra headers attached to EVERY outbound request (e.g. an `X-PAYMENT`
    /// x402 authorization). Empty by default — a no-op.
    extra_headers: Vec<(String, String)>,
}

impl fmt::Debug for GeminiClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeminiClient")
            .field("base_url", &self.base_url.as_str())
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl GeminiClient {
    pub fn new(api_key: impl Into<String>) -> Result<Self> {
        let builder = Client::builder()
            .user_agent(concat!("localharness/", env!("CARGO_PKG_VERSION")));
        // reqwest's wasm builder doesn't have .timeout() — fetch timeouts
        // are controlled by the browser, not the client config.
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

    pub fn with_base_url(mut self, url: Url) -> Self {
        self.base_url = url;
        self
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

    /// Install a per-request key provider (see [`KeyProvider`]). The static
    /// key from [`new`][Self::new] becomes a fallback only.
    pub fn with_key_provider(mut self, provider: KeyProvider) -> Self {
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

    /// Non-streaming `generateContent`. Use for one-shot endpoints like
    /// image generation where there's no benefit to SSE.
    pub async fn generate(
        &self,
        model: &str,
        req: &GenerateContentRequest,
    ) -> Result<GenerateChunk> {
        let path = format!("v1beta/models/{model}:generateContent");
        let url = self
            .base_url
            .join(&path)
            .map_err(|e| Error::other(format!("invalid model url: {e}")))?;

        let response = self
            .apply_extra_headers(
                self.http
                    .post(url)
                    .header("x-goog-api-key", self.current_key()),
            )
            .json(req)
            .send()
            .await
            .map_err(|e| Error::transport(format!("gemini POST: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(Error::http_status(
                status.as_u16(),
                format!("gemini HTTP {status}: {body}"),
            ));
        }

        response
            .json::<GenerateChunk>()
            .await
            .map_err(|e| Error::decode("gemini JSON", e.to_string()))
    }

    /// Streaming `generateContent`. Returns a `Stream` that yields one
    /// `GenerateChunk` per SSE `data:` frame (excluding the synthetic
    /// `data: [DONE]` terminator).
    ///
    /// The returned stream is `Send + 'static` so callers can move it into
    /// a `tokio::spawn`'d task without lifetime gymnastics.
    pub async fn stream_generate(
        &self,
        model: &str,
        req: &GenerateContentRequest,
    ) -> Result<GeminiSseStream> {
        let path = format!("v1beta/models/{model}:streamGenerateContent");
        let mut url = self
            .base_url
            .join(&path)
            .map_err(|e| Error::other(format!("invalid model url: {e}")))?;
        url.query_pairs_mut().append_pair("alt", "sse");

        let response = self
            .apply_extra_headers(
                self.http
                    .post(url)
                    .header("x-goog-api-key", self.current_key())
                    .header("accept", "text/event-stream"),
            )
            .json(req)
            .send()
            .await
            .map_err(|e| Error::transport(format!("gemini POST: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(Error::http_status(
                status.as_u16(),
                format!("gemini HTTP {status}: {body}"),
            ));
        }

        let byte_stream = response.bytes_stream().map(|res| {
            res.map_err(|e| Error::transport(format!("gemini chunk read: {e}")))
        });
        Ok(GeminiSseStream::new(Box::pin(byte_stream)))
    }
}

// =============================================================================
// SSE stream
// =============================================================================

/// Decodes a Gemini SSE byte stream into parsed [`GenerateChunk`]s.
///
/// Gemini's SSE format is conventional: lines starting with `data:`
/// carry JSON; blank lines separate frames; an optional `data: [DONE]`
/// closes the stream. The wire-agnostic frame splitting (CRLF+LF-tolerant
/// boundaries, partial-chunk buffering, `[DONE]` sentinel, EOF flush) is
/// the shared `SseFrameStream` (crate-private, in `backends::sse`); this
/// type only decodes each payload as a [`GenerateChunk`].
pub struct GeminiSseStream {
    frames: SseFrameStream,
}

impl GeminiSseStream {
    fn new(upstream: ByteStream) -> Self {
        Self {
            frames: SseFrameStream::new(upstream, Some(b"[DONE]"), "gemini"),
        }
    }
}

impl Stream for GeminiSseStream {
    type Item = Result<GenerateChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.frames).poll_next(cx) {
            Poll::Ready(Some(Ok(payload))) => Poll::Ready(Some(decode_chunk(&payload))),
            Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

fn decode_chunk(payload: &[u8]) -> Result<GenerateChunk> {
    serde_json::from_slice::<GenerateChunk>(payload)
        .map_err(|e| Error::decode("gemini sse decode",
            format!("{e}; payload: {}", String::from_utf8_lossy(payload))))
}

// Re-export an `Arc<GeminiClient>` for ergonomic cloning into spawned tasks.
pub type SharedClient = Arc<GeminiClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use futures_util::stream;

    fn bytes_from(parts: &[&[u8]]) -> ByteStream {
        let owned: Vec<Bytes> = parts.iter().map(|b| Bytes::copy_from_slice(b)).collect();
        Box::pin(stream::iter(owned.into_iter().map(Ok)))
    }

    /// THE token-staleness regression (on-chain feedback #46): the credit
    /// proxy rejects signed auth tokens older than 5 minutes, so a client
    /// must mint a FRESH credential per request when a provider is set —
    /// never cache the first one — and fall back to the static key without.
    #[test]
    fn key_provider_mints_fresh_credential_per_request() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let c = GeminiClient::new("static-key").unwrap();
        assert_eq!(c.current_key(), "static-key");

        let calls = Arc::new(AtomicU32::new(0));
        let calls2 = calls.clone();
        let c = c.with_key_provider(Arc::new(move || {
            let n = calls2.fetch_add(1, Ordering::SeqCst) + 1;
            format!("fresh-{n}")
        }));
        assert_eq!(c.current_key(), "fresh-1");
        assert_eq!(c.current_key(), "fresh-2");
        assert_eq!(calls.load(Ordering::SeqCst), 2, "minted per request, never cached");
    }

    #[tokio::test]
    async fn decodes_two_frames() {
        let bytes = bytes_from(&[
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]}}]}\n\n",
            b"data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\ndata: [DONE]\n\n",
        ]);
        let mut s = GeminiSseStream::new(bytes);
        let first = s.next().await.unwrap().unwrap();
        assert_eq!(first.candidates.len(), 1);
        let second = s.next().await.unwrap().unwrap();
        assert_eq!(second.candidates[0].finish_reason.unwrap(),
            crate::backends::gemini::wire::FinishReason::Stop);
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn decodes_crlf_terminated_frames() {
        // Browser fetch surfaces Gemini's SSE with CRLF line endings.
        // The parser must split on \r\n\r\n, not just \n\n.
        let bytes = bytes_from(&[
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]}}]}\r\n\r\n",
            b"data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\r\n\r\ndata: [DONE]\r\n\r\n",
        ]);
        let mut s = GeminiSseStream::new(bytes);
        let first = s.next().await.unwrap().unwrap();
        assert_eq!(first.candidates.len(), 1);
        let second = s.next().await.unwrap().unwrap();
        assert_eq!(
            second.candidates[0].finish_reason.unwrap(),
            crate::backends::gemini::wire::FinishReason::Stop
        );
        assert!(s.next().await.is_none());
    }

    #[tokio::test]
    async fn handles_split_across_chunks() {
        let bytes = bytes_from(&[
            b"data: {\"candi",
            b"dates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]}}]}\n\n",
        ]);
        let mut s = GeminiSseStream::new(bytes);
        let first = s.next().await.unwrap().unwrap();
        assert_eq!(first.candidates[0].content.as_ref().unwrap().parts.len(), 1);
    }

    // ----- collect helper for multi-frame assertions -----

    async fn collect_texts(mut s: GeminiSseStream) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(chunk) = s.next().await {
            let chunk = chunk.unwrap();
            for cand in chunk.candidates {
                if let Some(content) = cand.content {
                    for part in content.parts {
                        if let crate::backends::gemini::wire::Part::Text { text } = part {
                            out.push(text);
                        }
                    }
                }
            }
        }
        out
    }

    /// REGRESSION: SSE permits the stream's last event to omit the trailing
    /// blank line. Before the fix, the `done` flush only drained
    /// `\n\n`-terminated frames, so a complete final `data: {...}` with no
    /// trailing blank line was silently dropped — and that frame is exactly
    /// the one Gemini uses to deliver `finishReason` + `usageMetadata`.
    #[tokio::test]
    async fn flushes_final_frame_without_trailing_blank_line() {
        let bytes = bytes_from(&[
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]}}]}\n\n",
            // No trailing \n\n on the final frame.
            b"data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}",
        ]);
        let mut s = GeminiSseStream::new(bytes);
        let first = s.next().await.unwrap().unwrap();
        assert_eq!(first.candidates.len(), 1);
        let second = s.next().await.unwrap().unwrap();
        assert_eq!(
            second.candidates[0].finish_reason.unwrap(),
            crate::backends::gemini::wire::FinishReason::Stop,
            "the final unterminated frame must still be decoded"
        );
        assert!(s.next().await.is_none());
    }

    /// A single frame, no terminator at all, whole stream is one EOF flush.
    #[tokio::test]
    async fn flushes_single_unterminated_frame() {
        let bytes = bytes_from(&[
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"only\"}]}}]}",
        ]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["only".to_string()]);
    }

    /// Final frame terminated only by a single `\n` (not a blank line) at EOF.
    #[tokio::test]
    async fn flushes_final_frame_with_single_newline() {
        let bytes = bytes_from(&[b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"x\"}]}}]}\n"]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["x".to_string()]);
    }

    /// Multiple complete events arriving in ONE upstream chunk must all be
    /// surfaced, in order.
    #[tokio::test]
    async fn multiple_events_in_one_chunk() {
        let bytes = bytes_from(&[concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"a\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"b\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"c\"}]}}]}\n\n",
        ).as_bytes()]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["a", "b", "c"]);
    }

    /// `data: [DONE]` mid-stream sets done and suppresses any frames after it,
    /// while frames before it are delivered.
    #[tokio::test]
    async fn done_sentinel_terminates() {
        let bytes = bytes_from(&[concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"a\"}]}}]}\n\n",
            "data: [DONE]\n\n",
            // Anything after [DONE] must be ignored.
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"after\"}]}}]}\n\n",
        ).as_bytes()]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["a".to_string()]);
    }

    /// Empty `data:` lines and unrelated SSE fields (`event:`, `id:`, `:`
    /// comment / keepalive) must not produce phantom chunks.
    #[tokio::test]
    async fn skips_empty_and_non_data_lines() {
        let bytes = bytes_from(&[concat!(
            ": keepalive comment\n\n",
            "event: message\nid: 42\n\n",
            "data:\n\n", // empty data payload -> skipped (would be invalid JSON)
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"real\"}]}}]}\n\n",
        ).as_bytes()]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["real".to_string()]);
    }

    /// A `data:` field with NO leading space (Gemini sends `data: ` but the
    /// SSE spec allows `data:value`). Only ONE leading space is stripped.
    #[tokio::test]
    async fn data_field_without_leading_space() {
        let bytes = bytes_from(&[b"data:{\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"ns\"}]}}]}\n\n"]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["ns".to_string()]);
    }

    /// A frame boundary split across the chunk boundary: the first chunk ends
    /// mid-CRLF-terminator (`...}\r\n\r`) and the second supplies the final
    /// `\n`. The frame must only fire once the full terminator arrives.
    #[tokio::test]
    async fn crlf_terminator_split_across_chunks() {
        let bytes = bytes_from(&[
            b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"hi\"}]}}]}\r\n\r",
            b"\ndata: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\r\n\r\n",
        ]);
        let s = GeminiSseStream::new(bytes);
        // Both frames decode; the second carries the finishReason.
        let texts = collect_texts(s).await;
        assert_eq!(texts, vec!["hi".to_string()]);
    }

    /// A multibyte UTF-8 character split across two network chunks must not
    /// corrupt the decoded text. The frame stays buffered until the
    /// blank-line terminator arrives, by which point the char is whole — so
    /// `from_utf8` on the assembled frame never sees a partial code point.
    #[tokio::test]
    async fn multibyte_char_split_across_chunks() {
        // "é" is 0xC3 0xA9. Split it across the chunk boundary.
        let full = "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"é\"}]}}]}\n\n";
        let raw = full.as_bytes();
        let split_at = raw.iter().position(|&b| b == 0xC3).unwrap();
        let (head, tail) = raw.split_at(split_at + 1); // keep the 0xC3 lead byte in head
        let bytes = bytes_from(&[head, tail]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["é".to_string()]);
    }

    /// Mixed LF and CRLF terminators in the same stream both work.
    #[tokio::test]
    async fn mixed_lf_and_crlf_terminators() {
        let bytes = bytes_from(&[concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"lf\"}]}}]}\n\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"crlf\"}]}}]}\r\n\r\n",
        ).as_bytes()]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["lf", "crlf"]);
    }

    /// A genuinely malformed JSON payload surfaces as an Err item (and does
    /// not panic); a subsequent valid frame is unaffected only if it comes
    /// first — here we assert the error is propagated, not swallowed.
    #[tokio::test]
    async fn malformed_json_yields_error_not_panic() {
        let bytes = bytes_from(&[b"data: {not json}\n\n"]);
        let mut s = GeminiSseStream::new(bytes);
        let item = s.next().await.unwrap();
        assert!(item.is_err(), "malformed JSON must be an Err, got {item:?}");
    }

    /// Heartbeat / blank frames (`\n\n` with no `data:`) are silently skipped,
    /// never yielding an empty/None-misinterpreted chunk.
    #[tokio::test]
    async fn bare_blank_frames_skipped() {
        let bytes = bytes_from(&[concat!(
            "\n\n",
            "\r\n\r\n",
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"v\"}]}}]}\n\n",
        ).as_bytes()]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["v".to_string()]);
    }

    /// Empty upstream (no bytes at all) terminates cleanly with no items.
    #[tokio::test]
    async fn empty_stream_yields_nothing() {
        let bytes = bytes_from(&[]);
        let mut s = GeminiSseStream::new(bytes);
        assert!(s.next().await.is_none());
        // Idempotent: polling again still None (no panic on drained buffer).
        assert!(s.next().await.is_none());
    }

    /// A trailing `data: [DONE]` with NO blank line after it (EOF) must still
    /// be recognized as the sentinel and not decoded as a chunk.
    #[tokio::test]
    async fn done_sentinel_unterminated_at_eof() {
        let bytes = bytes_from(&[concat!(
            "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"a\"}]}}]}\n\n",
            "data: [DONE]", // no trailing newline
        ).as_bytes()]);
        let s = GeminiSseStream::new(bytes);
        assert_eq!(collect_texts(s).await, vec!["a".to_string()]);
    }
}
