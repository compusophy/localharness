//! Idle (stall) timeout for model response streams.
//!
//! A streaming model response is consumed chunk-by-chunk in the shared turn
//! engine (`turn_engine.rs`). The UI stop button is *cooperative*, and a
//! stream parked on a silent socket (a black-holed proxy, a model that opened
//! the connection and then went away) produces no chunks — so this module
//! provides both an idle deadline AND a cancel-aware wrapper
//! ([`next_with_idle_timeout_or_cancel`]) that re-checks the stop flag every
//! [`CANCEL_POLL_MS`] while the stream is silent (telemetry #33: Stop used to
//! wait for the next chunk).
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
use std::sync::atomic::{AtomicBool, Ordering};

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
    /// The cancel flag flipped while the stream was silent (the stop button).
    Cancelled,
}

/// How often a SILENT stream re-checks the cancel flag, in milliseconds.
///
/// This bounds the stop button's worst-case latency while the model is between
/// chunks (thinking, network stall): before this existed, cancel was only
/// observed when the NEXT chunk arrived — a long pre-answer think meant Stop
/// did nothing for seconds (telemetry #33). A chunk that IS ready still wins
/// instantly ([`next_with_idle_timeout`] polls the stream first), so a steady
/// stream pays nothing for the polling.
pub(crate) const CANCEL_POLL_MS: u32 = 100;

/// Await the next item of `stream` under the idle deadline, ALSO honouring a
/// cooperative `cancel` flag while the stream is silent.
///
/// Implementation: the idle window is consumed in [`CANCEL_POLL_MS`] slices;
/// between slices the flag is re-read. Dropping the intermediate `next()`
/// future between slices is safe — the stream itself retains all progress
/// (an item is only moved out on `Poll::Ready`). The idle semantics are
/// unchanged: only `idle_ms` of TOTAL silence returns `IdleTimeout`, and any
/// arriving item re-arms the window (the caller re-invokes per chunk).
pub(crate) async fn next_with_idle_timeout_or_cancel<S, T>(
    stream: &mut S,
    idle_ms: u32,
    cancel: &AtomicBool,
) -> NextChunk<T>
where
    S: Stream<Item = T> + Unpin,
{
    let mut waited: u32 = 0;
    loop {
        // Checked BEFORE each poll slice: a stop pressed while the stream is
        // silent is seen within one slice, and a stop pressed between chunks
        // is seen on the next invocation's first check.
        if cancel.load(Ordering::Acquire) {
            return NextChunk::Cancelled;
        }
        let slice = CANCEL_POLL_MS.min(idle_ms - waited).max(1);
        match next_with_idle_timeout(stream, slice).await {
            NextChunk::IdleTimeout => {
                waited = waited.saturating_add(slice);
                if waited >= idle_ms {
                    return NextChunk::IdleTimeout;
                }
            }
            other => return other,
        }
    }
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
    let sleep = crate::runtime::sleep_ms(idle_ms);
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::stream;

    /// A chunk available NOW wins over the idle timer — `select` polls the chunk
    /// future first, so a steadily-streaming response is never falsely stalled.
    #[tokio::test]
    async fn ready_item_beats_the_timer_then_end() {
        let mut s = stream::iter(vec![42i32]);
        match next_with_idle_timeout(&mut s, 60_000).await {
            NextChunk::Item(v) => assert_eq!(v, 42),
            _ => panic!("a ready chunk must win over a 60s timer"),
        }
        // Drained → upstream EOF → End (NOT a timeout).
        assert!(matches!(next_with_idle_timeout(&mut s, 60_000).await, NextChunk::End));
    }

    /// An already-finished stream is `End`, never an `IdleTimeout`.
    #[tokio::test]
    async fn empty_stream_is_end_not_timeout() {
        let mut s = stream::iter(Vec::<i32>::new());
        assert!(matches!(next_with_idle_timeout(&mut s, 60_000).await, NextChunk::End));
    }

    /// A black-holed stream (never produces a chunk) trips the idle timeout
    /// instead of hanging forever — the whole reason this wrapper exists.
    #[tokio::test]
    async fn a_silent_stream_trips_the_idle_timeout() {
        let mut s = stream::pending::<i32>();
        assert!(matches!(next_with_idle_timeout(&mut s, 5).await, NextChunk::IdleTimeout));
    }

    /// The idle window is a generous TOTAL-silence detector (2 min), not a cap on
    /// response length — a guard so it can't be trimmed into a length cap by
    /// accident.
    #[test]
    fn idle_window_is_two_minutes() {
        assert_eq!(STREAM_IDLE_TIMEOUT_MS, 120_000);
    }

    /// A cancel flag that is ALREADY set returns `Cancelled` immediately — even
    /// with a ready item queued. Cancel means "drop everything", not "finish
    /// reading first".
    #[tokio::test]
    async fn preset_cancel_wins_over_a_ready_item() {
        let cancel = AtomicBool::new(true);
        let mut s = stream::iter(vec![42i32]);
        assert!(matches!(
            next_with_idle_timeout_or_cancel(&mut s, 60_000, &cancel).await,
            NextChunk::Cancelled
        ));
    }

    /// The stop button pressed while the stream is SILENT (mid-think / stalled
    /// socket) is observed within roughly one poll slice — the telemetry #33
    /// fix: cancel no longer waits for the next chunk to arrive.
    #[tokio::test]
    async fn cancel_mid_silence_interrupts_promptly() {
        use std::sync::Arc;
        let cancel = Arc::new(AtomicBool::new(false));
        let flip = cancel.clone();
        tokio::spawn(async move {
            crate::runtime::sleep_ms(30).await;
            flip.store(true, Ordering::Release);
        });
        let mut s = stream::pending::<i32>();
        let t0 = std::time::Instant::now();
        let out = next_with_idle_timeout_or_cancel(&mut s, 60_000, &cancel).await;
        assert!(matches!(out, NextChunk::Cancelled), "cancel must break a silent stream");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(5),
            "cancel latency is bounded by the poll slice, not the 60s idle window"
        );
    }

    /// Uncancelled behavior is unchanged: a ready item still wins, EOF is
    /// still `End`, and total silence still trips `IdleTimeout` (the window
    /// accumulates across poll slices).
    #[tokio::test]
    async fn uncancelled_semantics_match_the_plain_helper() {
        let cancel = AtomicBool::new(false);
        let mut s = stream::iter(vec![7i32]);
        assert!(matches!(
            next_with_idle_timeout_or_cancel(&mut s, 60_000, &cancel).await,
            NextChunk::Item(7)
        ));
        assert!(matches!(
            next_with_idle_timeout_or_cancel(&mut s, 60_000, &cancel).await,
            NextChunk::End
        ));
        // idle_ms smaller than one poll slice: the slice clamps to it.
        let mut p = stream::pending::<i32>();
        assert!(matches!(
            next_with_idle_timeout_or_cancel(&mut p, 25, &cancel).await,
            NextChunk::IdleTimeout
        ));
        // idle_ms spanning multiple slices still times out (accumulation).
        let mut p2 = stream::pending::<i32>();
        assert!(matches!(
            next_with_idle_timeout_or_cancel(&mut p2, 220, &cancel).await,
            NextChunk::IdleTimeout
        ));
    }
}
