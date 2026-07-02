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
    // LOSSY, not `unwrap_or("")`: invalid UTF-8 (a multi-byte char split across a
    // network chunk, or backend corruption) must not silently DROP the whole SSE
    // frame's `data:` payload (truncating the stream). Replace only the bad bytes
    // (U+FFFD) and keep the rest. `Cow<str>` derefs to `str` for `.split`.
    let text = String::from_utf8_lossy(frame);
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{stream, StreamExt};

    fn byte_stream(parts: &[&[u8]]) -> ByteStream {
        let owned: Vec<Bytes> = parts.iter().map(|b| Bytes::copy_from_slice(b)).collect();
        Box::pin(stream::iter(owned.into_iter().map(Ok)))
    }

    /// Drive the decoder to completion, returning each yielded payload as a
    /// String (every test payload here is valid UTF-8).
    async fn frames(parts: &[&[u8]], sentinel: Option<&'static [u8]>) -> Vec<String> {
        let mut s = SseFrameStream::new(byte_stream(parts), sentinel, "test");
        let mut out = Vec::new();
        while let Some(item) = s.next().await {
            out.push(String::from_utf8(item.unwrap()).unwrap());
        }
        out
    }

    #[tokio::test]
    async fn lf_and_crlf_boundaries_both_split() {
        // The load-bearing CRLF gotcha (wasm fetch): a frame ends at `\n\n` OR
        // `\r\n\r\n`. Mixed in one stream, both must split.
        assert_eq!(frames(&[b"data: a\n\n", b"data: b\r\n\r\n"], None).await, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn multiple_data_lines_in_one_frame_concat_with_newline() {
        // WHATWG: multiple `data:` lines in a frame join with `\n`.
        assert_eq!(frames(&[b"data: line1\ndata: line2\n\n"], None).await, vec!["line1\nline2"]);
    }

    #[tokio::test]
    async fn non_data_fields_and_comments_ignored() {
        let parts: &[&[u8]] = &[b"event: msg\ndata: x\nid: 7\n: a comment\nretry: 100\n\n"];
        assert_eq!(frames(parts, None).await, vec!["x"]);
    }

    #[tokio::test]
    async fn only_one_leading_space_stripped() {
        // `data:  x` (two spaces) keeps the second; `data:x` (no space) is exact.
        assert_eq!(frames(&[b"data:  x\n\n", b"data:y\n\n"], None).await, vec![" x", "y"]);
    }

    #[tokio::test]
    async fn heartbeat_and_empty_frames_are_skipped() {
        // A comment-only frame and a blank frame yield nothing; a real one does.
        assert_eq!(frames(&[b": ping\n\n", b"\n\n", b"data: real\n\n"], None).await, vec!["real"]);
    }

    #[tokio::test]
    async fn eof_flushes_an_unterminated_final_frame() {
        // The last event need not end in a blank line — at EOF the remainder is
        // one final frame (for both backends it carries the stop/finish data).
        assert_eq!(frames(&[b"data: a\n\n", b"data: last"], None).await, vec!["a", "last"]);
    }

    #[tokio::test]
    async fn sentinel_terminates_and_drops_anything_after_it() {
        // `[DONE]` is authoritative: stop, and DON'T leak post-sentinel frames
        // (incl. via the EOF flush).
        let parts: &[&[u8]] = &[b"data: a\n\n", b"data: [DONE]\n\ndata: leaked\n\n"];
        assert_eq!(frames(parts, Some(b"[DONE]")).await, vec!["a"]);
    }

    #[tokio::test]
    async fn frame_split_across_chunks_is_buffered_until_complete() {
        // A frame whose bytes arrive across two upstream chunks decodes once the
        // boundary lands — not before, not duplicated.
        assert_eq!(frames(&[b"data: hel", b"lo\n\n"], None).await, vec!["hello"]);
        // The CRLF boundary split mid-sequence too (`\r\n` then `\r\n`).
        assert_eq!(frames(&[b"data: z\r\n", b"\r\n"], None).await, vec!["z"]);
    }
}
