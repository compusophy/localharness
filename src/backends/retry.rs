//! Shared policy for retrying a model stream-OPEN against transient failures.
//!
//! Only the stream OPEN is retried — it is idempotent (no bytes emitted yet). A
//! MID-STREAM failure is NOT retried (a partial response already went out). Auth /
//! credits / rate-limit fail FAST: retrying them just burns time and quota.
//!
//! Both turn loops (gemini / anthropic) and the subagent loop
//! (`builtins::start_subagent`) share this so the policy lives in ONE place
//! (backends spec: a fix that would be copy-pasted into two backends belongs in
//! the shared core). Telemetry #29 was a Gemini HTTP 503 aborting a whole turn
//! because the MAIN loop opened its stream once, while the subagent already retried.

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
}
