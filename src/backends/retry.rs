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
//! stream once, while the subagent already retried. Telemetry #41 added the
//! [`BACKEND_SEND`] class (bare "error sending request" — flaky mobile): ONE
//! retry after [`SEND_RETRY_BACKOFF_MS`], tighter than the other transient
//! classes because the wording can't prove the request never reached the proxy
//! (which floor-debits after a 2xx upstream — retrying a lost RESPONSE bills
//! twice; one retry bounds that to one message).

use crate::error_codes::{BACKEND_NETWORK, BACKEND_SEND, BACKEND_SERVER, BACKEND_TIMEOUT};

/// Total tries (1 initial + retries) for opening a model stream.
pub const MAX_STREAM_ATTEMPTS: u32 = 3;
/// Base backoff between stream-open retries; ×attempt for a small linear ramp
/// (300ms, then 600ms).
pub const STREAM_RETRY_BACKOFF_MS: u32 = 300;
/// The bare transport-send class ([`BACKEND_SEND`], telemetry #41 — "error
/// sending request" with no detail, flaky mobile networks) is retried ONCE
/// only: the wording can't prove the request never reached the proxy, and the
/// proxy floor-debits after a 2xx upstream — so a retry after a lost RESPONSE
/// could double-bill. One retry bounds that worst case to one message.
pub const SEND_MAX_ATTEMPTS: u32 = 2;
/// Backoff before the single [`BACKEND_SEND`] retry (~500ms lets a radio
/// blip / network handoff settle).
pub const SEND_RETRY_BACKOFF_MS: u32 = 500;

/// Whether an error code is a transient class worth retrying a stream-open for
/// (transport / 5xx / timeout / bare send failure). Everything else — auth,
/// credits, rate-limit, any 4xx, a non-backend code — fails fast.
pub fn is_transient(code: u16) -> bool {
    matches!(code, BACKEND_NETWORK | BACKEND_SERVER | BACKEND_TIMEOUT | BACKEND_SEND)
}

/// Total tries allowed for error `code`: 1 (fail fast) for non-transient,
/// [`SEND_MAX_ATTEMPTS`] for the ambiguous send class, [`MAX_STREAM_ATTEMPTS`]
/// for the other transient classes (5xx never billed — the proxy only debits
/// after a 2xx upstream — and named network causes mean the request died
/// before reaching the server; both are safe to retry harder).
pub fn max_attempts(code: u16) -> u32 {
    if code == BACKEND_SEND {
        SEND_MAX_ATTEMPTS
    } else if is_transient(code) {
        MAX_STREAM_ATTEMPTS
    } else {
        1
    }
}

/// Whether the just-failed attempt `attempt` (1-based) for error `code` should be
/// retried: a transient class AND still under its [`max_attempts`].
pub fn should_retry(code: u16, attempt: u32) -> bool {
    attempt < max_attempts(code)
}

/// Backoff (ms) to wait before the next attempt after the 1-based `attempt`
/// failed with error `code`.
pub fn backoff_ms(code: u16, attempt: u32) -> u32 {
    if code == BACKEND_SEND {
        SEND_RETRY_BACKOFF_MS
    } else {
        STREAM_RETRY_BACKOFF_MS * attempt
    }
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
                crate::runtime::sleep_ms(backoff_ms(e.code(), attempt)).await;
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
        for c in [BACKEND_NETWORK, BACKEND_SERVER, BACKEND_TIMEOUT, BACKEND_SEND] {
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
        assert_eq!(backoff_ms(BACKEND_SERVER, 2), STREAM_RETRY_BACKOFF_MS * 2);
    }

    /// Telemetry #41: the bare transport-send class retries exactly ONCE
    /// (2 attempts total — the wording can't prove the request never reached
    /// the proxy, so it's capped tighter than the other transient classes)
    /// with a flat ~500ms backoff.
    #[test]
    fn send_class_retries_once_with_flat_backoff() {
        assert_eq!(max_attempts(BACKEND_SEND), SEND_MAX_ATTEMPTS);
        assert!(should_retry(BACKEND_SEND, 1));
        assert!(!should_retry(BACKEND_SEND, SEND_MAX_ATTEMPTS));
        assert_eq!(backoff_ms(BACKEND_SEND, 1), SEND_RETRY_BACKOFF_MS);
        // Other transient classes keep the 3-attempt cap; non-transient 1.
        assert_eq!(max_attempts(BACKEND_NETWORK), MAX_STREAM_ATTEMPTS);
        assert_eq!(max_attempts(BACKEND_AUTH), 1);
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

    /// Telemetry #41 E2E through the wrapper: the exact reported error string
    /// ("gemini POST: error sending request") is retried exactly once — a
    /// recovered network on the 2nd attempt succeeds; a still-dead network
    /// surfaces the original error after 2 attempts, not 3.
    #[tokio::test]
    async fn open_stream_with_retry_retries_bare_send_failure_once() {
        use std::sync::atomic::{AtomicU32, Ordering};
        let calls = AtomicU32::new(0);
        let out = open_stream_with_retry(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                if n < 2 {
                    Err(crate::error::Error::other("gemini POST: error sending request"))
                } else {
                    Ok("stream")
                }
            }
        })
        .await
        .expect("2nd attempt succeeds");
        assert_eq!(out, "stream");
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let calls = AtomicU32::new(0);
        let err = open_stream_with_retry(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Err::<(), _>(crate::error::Error::other("gemini POST: error sending request")) }
        })
        .await
        .expect_err("still-dead network surfaces the error");
        assert_eq!(err.code(), crate::error_codes::BACKEND_SEND);
        assert_eq!(calls.load(Ordering::SeqCst), SEND_MAX_ATTEMPTS, "exactly one retry");
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
