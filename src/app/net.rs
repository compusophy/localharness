//! Network-resilience helpers for on-chain reads and remote fetches.
//!
//! **Why this exists.** Every `registry::*` read goes through `reqwest`,
//! which on `wasm32` is a thin wrapper over the browser `fetch` API. The
//! browser gives `fetch` **no default timeout** and `reqwest::Client::timeout`
//! is a documented no-op on wasm — so a TCP-connected-but-silent RPC node (a
//! "black hole", common on flaky mobile networks or a stalled proxy) yields a
//! future that **never resolves**. Awaiting it directly hangs the calling
//! paint/refresh path forever, which on the UI looks like a permanent
//! `loading…` pill or a frozen verify state with no error.
//!
//! [`with_timeout`] caps any such future: it races the work against
//! [`registry::sleep_ms`] (a `setTimeout` Promise on wasm) and returns
//! `Err("timeout")` if the deadline wins. Callers then degrade to a usable
//! fallback (a dash, a hidden pill, the embedded docs) instead of spinning.
//!
//! This lives in `src/app/` rather than `registry.rs` deliberately — the
//! transport-level fix (a real fetch `AbortController`) is a follow-up for the
//! registry layer; until then the call sites that paint UI guard themselves.

use std::future::Future;

use futures_util::future::{select, Either};

use super::registry;

/// Default deadline for a single on-chain read used during a paint/refresh.
/// Long enough to absorb a slow-but-alive RPC round-trip, short enough that a
/// dead node degrades to a fallback within a couple seconds rather than
/// hanging the surface indefinitely.
pub(crate) const READ_TIMEOUT_MS: u32 = 8_000;

/// Race `fut` against a `ms`-millisecond timer. Returns `Ok(fut output)` if
/// the work finishes first, or `Err("timeout")` if the timer wins.
///
/// Pure combinator over [`registry::sleep_ms`] + [`futures_util::future::select`]
/// — no `tokio`, so it compiles + runs on wasm (single-threaded, no `Send`).
/// The losing future is simply dropped (browser `fetch` is cancelled when its
/// future drops).
pub(crate) async fn with_timeout<F, T>(ms: u32, fut: F) -> Result<T, &'static str>
where
    F: Future<Output = T>,
{
    let work = std::pin::pin!(fut);
    let timer = std::pin::pin!(registry::sleep_ms(ms));
    match select(work, timer).await {
        Either::Left((out, _)) => Ok(out),
        Either::Right(((), _)) => Err("timeout"),
    }
}

/// Convenience wrapper at the [`READ_TIMEOUT_MS`] default — the common case for
/// an on-chain read feeding a paint.
pub(crate) async fn read<F, T>(fut: F) -> Result<T, &'static str>
where
    F: Future<Output = T>,
{
    with_timeout(READ_TIMEOUT_MS, fut).await
}
