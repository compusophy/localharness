//! Shared SSE frame decoder for the streaming HTTP backends.
//!
//! The Gemini and Anthropic SSE streams sit on the same wire-agnostic frame
//! skeleton: buffer raw bytes, split frames at the first blank line,
//! concatenate each frame's `data:` lines, and flush a final unterminated
//! frame at upstream EOF. That skeleton lives HERE, once; backend-specific
//! event parsing (`GenerateChunk` vs `StreamEvent`) stays in each backend's
//! `api.rs` on top of the raw payloads this module yields.
//!
//! GOTCHA (load-bearing): browser fetch surfaces SSE with CRLF line endings,
//! so a frame boundary is EITHER `\n\n` (LF) or `\r\n\r\n` (CRLF). Do not
//! regress this to LF-only — it bricks streaming on wasm.

use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures_core::Stream;
use tracing::trace;

use crate::error::Result;

// On native, the SSE byte stream must be `Send` so it can move into a
// `tokio::spawn`'d turn. On wasm32, reqwest's browser fetch stream isn't
// Send — that's fine because everything single-threads through
// `wasm_bindgen_futures::spawn_local`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub(crate) type ByteStream = Pin<Box<dyn Stream<Item = Result<Bytes>> + 'static>>;

/// Splits a raw SSE byte stream into per-frame `data:` payloads.
///
/// Yields one `Vec<u8>` per complete frame: every `data:` line in the frame,
/// newline-joined, with the prefix and a single leading space stripped
/// (`event:` / `id:` / `retry:` / comment lines ignored). Empty payloads
/// (heartbeats, comment-only frames) are skipped, never yielded. An optional
/// `sentinel` payload (Gemini's `[DONE]`) is authoritative: it terminates the
/// stream and drops anything buffered after it. At upstream EOF, blank-line-
/// terminated frames drain first, then any remaining bytes flush as ONE final
/// unterminated frame — the WHATWG event-stream rule is that a stream's last
/// event need not be terminated by a trailing blank line, and for both
/// backends that final frame is exactly the one carrying the stop/finish
/// metadata.
pub(crate) struct SseFrameStream {
    upstream: ByteStream,
    buffer: BytesMut,
    done: bool,
    /// Payload that terminates the stream (e.g. `b"[DONE]"`). `None` = no
    /// sentinel convention (Anthropic).
    sentinel: Option<&'static [u8]>,
    /// Backend label for trace logging (e.g. `"gemini"`).
    label: &'static str,
}

impl SseFrameStream {
    pub(crate) fn new(
        upstream: ByteStream,
        sentinel: Option<&'static [u8]>,
        label: &'static str,
    ) -> Self {
        Self {
            upstream,
            buffer: BytesMut::with_capacity(8 * 1024),
            done: false,
            sentinel,
            label,
        }
    }

    /// Pull a complete frame's bytes from `self.buffer` if one is present.
    /// Returns the `data:` payload (without the prefix or trailing newlines),
    /// or `None` if no full frame is buffered.
    ///
    /// A frame ends at the first blank line. Two byte sequences mark that
    /// boundary: `\n\n` (LF) or `\r\n\r\n` (CRLF). Browser fetch surfaces
    /// SSE with CRLF, so we must accept both.
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

    /// Consume the ENTIRE remaining buffer as one final frame, regardless of
    /// whether it ends in a blank line. Called only at upstream EOF (see the
    /// struct docs). Returns `None` once the buffer is empty.
    fn take_remaining(&mut self) -> Option<Vec<u8>> {
        if self.buffer.is_empty() {
            return None;
        }
        let frame = self.buffer.split_to(self.buffer.len());
        Some(extract_data_payload(&frame))
    }
}

impl Stream for SseFrameStream {
    type Item = Result<Vec<u8>>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if self.done {
                // Flush any remaining buffered frame even after upstream EOF.
                // First drain blank-line-terminated frames; once those are
                // gone, treat whatever is left as the final unterminated frame.
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
                if Some(payload.as_slice()) == self.sentinel {
                    continue;
                }
                return Poll::Ready(Some(Ok(payload)));
            }

            if let Some(payload) = self.take_frame() {
                if payload.is_empty() {
                    continue;
                }
                if Some(payload.as_slice()) == self.sentinel {
                    // The sentinel is authoritative: terminate the stream and
                    // drop anything buffered after it, rather than letting the
                    // EOF flush leak post-sentinel frames as payloads.
                    self.done = true;
                    self.buffer.clear();
                    continue;
                }
                return Poll::Ready(Some(Ok(payload)));
            }

            match self.upstream.as_mut().poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(bytes))) => {
                    trace!(len = bytes.len(), "{} sse bytes", self.label);
                    self.buffer.extend_from_slice(&bytes);
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Some(Err(e))),
                Poll::Ready(None) => self.done = true,
            }
        }
    }
}

/// Concatenate every `data:` line in `frame`, trimming the prefix and any
/// single leading space. Other SSE fields (`event:`, `id:`, `retry:`, `:`
/// comments) are ignored.
fn extract_data_payload(frame: &[u8]) -> Vec<u8> {
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
