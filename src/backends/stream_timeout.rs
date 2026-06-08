//! Idle (stall) timeout for model response streams.
//!
//! A streaming model response is consumed chunk-by-chunk in the backend
//! turn loops (`gemini/loop.rs`, `anthropic/loop.rs`). The UI stop button
//! is *cooperative* — it's only checked between chunks — so a stream parked
//! on a silent socket (a black-holed proxy, a model that opened the
//! connection and then went away) never reaches a cancel boundary and the
//! turn hangs forever, holding the one-turn-at-a-time guard.
//!
//! This module wraps the per-chunk `stream.next().await` in an IDLE-based
//! deadline: each awaited chunk races a fresh [`sleep_ms`] of
//! [`STREAM_IDLE_TIMEOUT_MS`]. Because the sleep is created anew for *every*
//! chunk, the timer resets on each byte that arrives — a steadily streaming
//! response (even one that streams for many minutes) is byte-for-byte
//! unaffected. Only a TRUE stall — zero data for the whole idle window —
//! trips the timeout, which the caller turns into a normal stream error so
//! the turn ends via the existing error path (`TurnOutcome::Error`) and the
//! guard releases. No panic; recoverable.

use std::pin::pin;

use futures_core::Stream;
use futures_util::stream::StreamExt;

/// Idle window for a model response stream, in milliseconds.
///
/// A response that is steadily streaming resets this every chunk, so this is
/// NOT a cap on total response length — it's a "the connection is dead, not
/// slow" detector. 2 minutes of TOTAL silence (no chunk at all) means the
/// socket is black-holed. Generous on purpose: models can pause mid-stream
/// (thinking, server-side tool latency) for a long time without it being a
/// stall. Overridable via the `LH_STREAM_IDLE_TIMEOUT_MS` env var (native
/// only; on wasm the const applies).
pub(crate) const STREAM_IDLE_TIMEOUT_MS: u32 = 120_000;

/// Resolve the effective idle timeout. On native, an `LH_STREAM_IDLE_TIMEOUT_MS`
/// env override (a positive integer of milliseconds) wins; otherwise the
/// const. On wasm there's no env, so the const is used directly.
pub(crate) fn idle_timeout_ms() -> u32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(v) = std::env::var("LH_STREAM_IDLE_TIMEOUT_MS") {
            if let Ok(n) = v.trim().parse::<u32>() {
                if n > 0 {
                    return n;
                }
            }
        }
    }
    STREAM_IDLE_TIMEOUT_MS
}

/// The outcome of awaiting the next chunk under an idle deadline.
pub(crate) enum NextChunk<T> {
    /// A chunk (or a per-chunk stream error) arrived before the deadline.
    Item(T),
    /// The upstream stream ended (EOF).
    End,
    /// No data arrived for the whole idle window — the stream stalled.
    IdleTimeout,
}

/// Await the next item of `stream`, racing it against a freshly-armed
/// [`idle_timeout_ms`] sleep.
///
/// The sleep is constructed inside this call, so each invocation (i.e. each
/// chunk) starts a brand-new timer — that is what makes the timeout
/// IDLE-based: a chunk arriving re-arms the window for the next one. A steady
/// stream never trips it; only `idle_ms` of uninterrupted silence does.
pub(crate) async fn next_with_idle_timeout<S, T>(stream: &mut S, idle_ms: u32) -> NextChunk<T>
where
    S: Stream<Item = T> + Unpin,
{
    let next = stream.next();
    let sleep = sleep_ms(idle_ms);
    // Race the two without cancel-on-first-poll surprises: `select` polls the
    // chunk future first, so a ready chunk always wins over a co-ready timer.
    let next = pin!(next);
    let sleep = pin!(sleep);
    match futures_util::future::select(next, sleep).await {
        // chunk future resolved first
        futures_util::future::Either::Left((Some(item), _sleep)) => NextChunk::Item(item),
        futures_util::future::Either::Left((None, _sleep)) => NextChunk::End,
        // timer fired first — the stream produced nothing for the whole window
        futures_util::future::Either::Right((_elapsed, _next)) => NextChunk::IdleTimeout,
    }
}

/// Sleep for `ms` milliseconds. cfg-gated: tokio timer on native, a
/// `setTimeout`-backed promise on wasm32 (mirrors `registry::sleep_ms` /
/// `app::verify::sleep_ms`, but lives here so the backends don't depend on a
/// feature-gated module).
#[cfg(not(target_arch = "wasm32"))]
async fn sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(target_arch = "wasm32")]
async fn sleep_ms(ms: u32) {
    use wasm_bindgen_futures::JsFuture;
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
                &resolve,
                ms as i32,
            );
        }
    });
    let _ = JsFuture::from(promise).await;
}
