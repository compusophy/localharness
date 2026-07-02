//! Shared policy for retrying a model stream-OPEN against transient failures.
//!
//! Only the stream OPEN is retried — it is idempotent (no bytes emitted yet). A
//! MID-STREAM failure is NOT retried (a partial response already went out). Auth /
//! credits / rate-limit fail FAST: retrying them just burns time and quota.
//!
//! All three turn loops (gemini / anthropic / openai, via
//! [`open_stream_with_retry`]) and the subagent loop (`builtins::start_subagent`)
//! share this so the policy lives in ONE place (backends spec: a fix that would
//! be copy-pasted into two backends belongs in the shared core). Telemetry #29
//! was a Gemini HTTP 503 aborting a whole turn because the MAIN loop opened its
//! stream once, while the subagent already retried.

use crate::error_codes::{BACKEND_NETWORK, BACKEND_SERVER, BACKEND_TIMEOUT};

/// Total tries (1 initial + retries) for opening a model stream.
pub const MAX_STREAM_ATTEMPTS: u32 = 3;
/// Base backoff between stream-open retries; ×attempt for a small linear ramp
/// (300ms, then 600ms).
pub const STREAM_RETRY_BACKOFF_MS: u32 = 300;

/// Whether an error code is a transient class worth retrying a stream-open for
/// (transport / 5xx / timeout). Everything else — auth, credits, rate-limit, any
/// 4xx, a non-backend code — fails fast.
pub fn is_transient(code: u16) -> bool {
    matches!(code, BACKEND_NETWORK | BACKEND_SERVER | BACKEND_TIMEOUT)
}

/// Whether the just-failed attempt `attempt` (1-based) for error `code` should be
/// retried: a transient class AND still under [`MAX_STREAM_ATTEMPTS`].
pub fn should_retry(code: u16, attempt: u32) -> bool {
    is_transient(code) && attempt < MAX_STREAM_ATTEMPTS
}

/// Backoff (ms) to wait before the next attempt after the 1-based `attempt` failed.
pub fn backoff_ms(attempt: u32) -> u32 {
    STREAM_RETRY_BACKOFF_MS * attempt
}

/// Open a model stream under the shared retry policy: call `open` up to
/// [`MAX_STREAM_ATTEMPTS`] times, sleeping [`backoff_ms`] between attempts,
/// retrying only [`is_transient`] error classes. THE one home for the #29
/// retry-wrapped stream-open — all three turn loops (gemini / anthropic /
/// openai) call this, so the policy can't drift per-backend again (openai
/// shipped WITHOUT the retry while the other two had it). Only the OPEN goes
/// through here — it is idempotent; never route a mid-stream failure through it.
pub(crate) async fn open_stream_with_retry<S, F, Fut>(mut open: F) -> crate::error::Result<S>
where
    F: FnMut() -> Fut,
    Fut: core::future::Future<Output = crate::error::Result<S>>,
{
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match open().await {
            Ok(s) => return Ok(s),
            Err(e) if should_retry(e.code(), attempt) => {
                crate::runtime::sleep_ms(backoff_ms(attempt)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error_codes::{BACKEND_AUTH, BACKEND_CREDITS, BACKEND_RATE_LIMIT};

    #[test]
    fn only_transient_classes_retry() {
        for c in [BACKEND_NETWORK, BACKEND_SERVER, BACKEND_TIMEOUT] {
            assert!(is_transient(c), "code {c} should be transient");
        }
        // auth/credits/rate-limit (and a non-backend code) must fail fast.
        for c in [BACKEND_AUTH, BACKEND_CREDITS, BACKEND_RATE_LIMIT, 0] {
            assert!(!is_transient(c), "code {c} must NOT retry");
        }
    }

    #[test]
    fn should_retry_stops_at_the_attempt_cap() {
        assert!(should_retry(BACKEND_SERVER, 1));
        assert!(should_retry(BACKEND_SERVER, MAX_STREAM_ATTEMPTS - 1));
        assert!(!should_retry(BACKEND_SERVER, MAX_STREAM_ATTEMPTS)); // cap reached
        assert!(!should_retry(BACKEND_RATE_LIMIT, 1)); // non-transient never retries
        assert_eq!(backoff_ms(2), STREAM_RETRY_BACKOFF_MS * 2);
    }

    /// The hoisted #29 stream-open wrapper: a transient failure (5xx) is
    /// retried up to the cap and a later success wins; a non-transient
    /// failure (auth) fails FAST on the first attempt.
    #[tokio::test]
    async fn open_stream_with_retry_retries_transient_then_succeeds() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let out = open_stream_with_retry(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if n < MAX_STREAM_ATTEMPTS {
                    Err(crate::error::Error::other("HTTP 503 internal server error"))
                } else {
                    Ok("stream")
                }
            }
        })
        .await
        .expect("succeeds within the attempt cap");
        assert_eq!(out, "stream");
        assert_eq!(calls.load(Ordering::SeqCst), MAX_STREAM_ATTEMPTS);
    }

    #[tokio::test]
    async fn open_stream_with_retry_fails_fast_on_non_transient() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let err = open_stream_with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(crate::error::Error::other("HTTP 401 Unauthorized: bad API key")) }
        })
        .await
        .expect_err("auth must not retry");
        assert_eq!(err.code(), BACKEND_AUTH);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "exactly one attempt");
    }

    /// A persistent transient failure exhausts the cap and returns the error.
    #[tokio::test]
    async fn open_stream_with_retry_gives_up_at_the_cap() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let err = open_stream_with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(crate::error::Error::other("HTTP 503 internal server error")) }
        })
        .await
        .expect_err("all attempts failed");
        assert_eq!(err.code(), BACKEND_SERVER);
        assert_eq!(calls.load(Ordering::SeqCst), MAX_STREAM_ATTEMPTS);
    }
}
