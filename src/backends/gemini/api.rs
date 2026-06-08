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

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use futures_util::stream::StreamExt;
use reqwest::{Client, Url};
use tracing::trace;

use crate::backends::gemini::wire::{GenerateChunk, GenerateContentRequest};
use crate::error::{Error, Result};

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
#[cfg(not(target_arch = "wasm32"))]
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

pub struct GeminiClient {
    http: Client,
    api_key: Box<str>,
    base_url: Url,
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
            base_url: Url::parse(DEFAULT_BASE_URL).expect("default base url is valid"),
        })
    }

    pub fn with_base_url(mut self, url: Url) -> Self {
        self.base_url = url;
        self
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
            .http
            .post(url)
            .header("x-goog-api-key", self.api_key.as_ref())
            .json(req)
            .send()
            .await
            .map_err(|e| Error::other(format!("gemini POST: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(Error::other(format!("gemini HTTP {status}: {body}")));
        }

        response
            .json::<GenerateChunk>()
            .await
            .map_err(|e| Error::other(format!("gemini JSON: {e}")))
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
            .http
            .post(url)
            .header("x-goog-api-key", self.api_key.as_ref())
            .header("accept", "text/event-stream")
            .json(req)
            .send()
            .await
            .map_err(|e| Error::other(format!("gemini POST: {e}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(Error::other(format!("gemini HTTP {status}: {body}")));
        }

        let byte_stream = response.bytes_stream().map(|res| {
            res.map_err(|e| Error::other(format!("gemini chunk read: {e}")))
        });
        Ok(GeminiSseStream::new(Box::pin(byte_stream)))
    }
}

// =============================================================================
// SSE stream
// =============================================================================

// On native, the SSE byte stream must be `Send` so it can move into a
// `tokio::spawn`'d turn. On wasm32, reqwest's browser fetch stream isn't
// Send — that's fine because everything single-threads through
// `wasm_bindgen_futures::spawn_local`.
#[cfg(not(target_arch = "wasm32"))]
type ByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes>> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
type ByteStream =
    Pin<Box<dyn Stream<Item = Result<Bytes>> + 'static>>;

/// Decodes a Gemini SSE byte stream into parsed [`GenerateChunk`]s.
///
/// Gemini's SSE format is conventional: lines starting with `data:`
/// carry JSON; blank lines separate frames; an optional `data: [DONE]`
/// closes the stream. We tolerate CR/LF and partial chunks; the decoder
/// buffers until it sees a frame boundary.
pub struct GeminiSseStream {
    upstream: ByteStream,
    buffer: BytesMut,
    done: bool,
}

impl GeminiSseStream {
    fn new(upstream: ByteStream) -> Self {
        Self {
            upstream,
            buffer: BytesMut::with_capacity(8 * 1024),
            done: false,
        }
    }

    /// Pull a complete frame's bytes from `self.buffer` if one is
    /// present. Returns the JSON payload (without the `data:` prefix
    /// or trailing newlines), or `None` if no full frame is buffered.
    ///
    /// A frame ends at the first blank line. Two byte sequences mark
    /// that boundary: `\n\n` (LF) or `\r\n\r\n` (CRLF). Browser fetch
    /// surfaces Gemini's SSE with CRLF, so we must accept both.
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

    /// Consume the ENTIRE remaining buffer as one final frame, regardless
    /// of whether it ends in a blank line. Called only at upstream EOF: the
    /// SSE/WHATWG event-stream rule is that a stream's last event need NOT
    /// be terminated by a trailing blank line, so a complete final
    /// `data: {...}` with no `\n\n` would otherwise be silently dropped —
    /// and for Gemini that frame is exactly the one carrying `finishReason`
    /// + `usageMetadata`. Returns `None` once the buffer is empty.
    fn take_remaining(&mut self) -> Option<Vec<u8>> {
        if self.buffer.is_empty() {
            return None;
        }
        let frame = self.buffer.split_to(self.buffer.len());
        Some(extract_data_payload(&frame))
    }
}

impl Stream for GeminiSseStream {
    type Item = Result<GenerateChunk>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.done {
                // Flush any remaining buffered frame even after upstream EOF.
                // First drain blank-line-terminated frames; once those are
                // gone, treat whatever is left as the final unterminated
                // frame (SSE permits the last event to omit the trailing
                // blank line — see `take_remaining`).
                let payload = match self.take_frame() {
                    Some(p) => p,
                    None => match self.take_remaining() {
                        Some(p) => p,
                        None => return Poll::Ready(None),
                    },
                };
                if payload.is_empty() {
                    continue;
                }
                if payload == b"[DONE]" {
                    continue;
                }
                return Poll::Ready(Some(decode_chunk(&payload)));
            }

            if let Some(payload) = self.take_frame() {
                if payload.is_empty() {
                    continue;
                }
                if payload == b"[DONE]" {
                    // `[DONE]` is authoritative: terminate the stream and drop
                    // anything buffered after it, rather than letting the EOF
                    // flush leak post-sentinel frames as chunks.
                    self.done = true;
                    self.buffer.clear();
                    continue;
                }
                return Poll::Ready(Some(decode_chunk(&payload)));
            }

            match self.upstream.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    trace!(len = bytes.len(), "gemini sse bytes");
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
    // and any single leading space.
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
        // Other SSE fields (event:, id:, retry:) are ignored.
    }
    out
}

fn decode_chunk(payload: &[u8]) -> Result<GenerateChunk> {
    serde_json::from_slice::<GenerateChunk>(payload)
        .map_err(|e| Error::other(format!("gemini sse decode: {e}; payload: {}",
            String::from_utf8_lossy(payload))))
}

// Re-export an `Arc<GeminiClient>` for ergonomic cloning into spawned tasks.
pub type SharedClient = Arc<GeminiClient>;

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    fn bytes_from(parts: &[&[u8]]) -> ByteStream {
        let owned: Vec<Bytes> = parts.iter().map(|b| Bytes::copy_from_slice(b)).collect();
        Box::pin(stream::iter(owned.into_iter().map(Ok)))
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
