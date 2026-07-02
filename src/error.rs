//! Typed error hierarchy for the SDK.
//!
//! Every fallible boundary returns `Result<T>` aliased to this crate's
//! `Error`. Wrapping inner errors via `#[from]` keeps call sites terse
//! (`foo()?`) while preserving the underlying source for `tracing` and
//! `std::error::Error::source` walks.

use std::time::Duration;

use thiserror::Error;

/// All errors the SDK can produce.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// OS-level I/O error.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// HTTP transport error.
    #[error("http: {0}")]
    Http(String),

    /// An HTTP failure carrying its REAL status code (structured — consumers
    /// read [`Error::http_status_code`] instead of substring-parsing "429"
    /// out of the message). `message` is the full user-facing text; `Display`
    /// prints it verbatim, so surfaced messages are identical to the legacy
    /// string-only path. Constructed via [`Error::http_status`].
    #[error("{message}")]
    HttpStatus {
        /// The HTTP status code (e.g. `429`).
        status: u16,
        /// The full formatted error message (what the user sees).
        message: String,
    },

    /// The connection was closed unexpectedly.
    #[error("connection closed")]
    Closed,

    /// Operation requires a started agent.
    #[error("not started")]
    NotStarted,

    /// `start()` was called more than once.
    #[error("already started")]
    AlreadyStarted,

    /// Invalid configuration.
    #[error("config: {0}")]
    Config(String),

    /// No tool registered under this name.
    #[error("tool '{name}' not found")]
    ToolNotFound {
        /// The requested tool name.
        name: String,
    },

    /// A tool returned an error during execution.
    #[error("tool '{name}' failed: {message}")]
    ToolFailed {
        /// The tool that failed.
        name: String,
        /// The error message from the tool.
        message: String,
    },

    /// A policy blocked the operation.
    #[error("policy denied: {0}")]
    PolicyDenied(String),

    /// An operation exceeded its deadline.
    #[error("timed out after {0:?}")]
    Timeout(Duration),

    /// Catch-all for errors that don't fit other variants.
    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Construct a catch-all `Other` error.
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }

    /// Construct a configuration error.
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Construct a structured HTTP error: `msg` is what users see (unchanged
    /// from the legacy string path); `status` rides alongside so
    /// [`Error::code`] classifies off the real number instead of substring
    /// matching. Backends route their non-2xx responses here.
    pub fn http_status(status: u16, msg: impl Into<String>) -> Self {
        Self::HttpStatus { status, message: msg.into() }
    }

    /// The HTTP status code, when this error carries one structurally
    /// (i.e. it is [`Error::HttpStatus`]). Legacy string-wrapping variants
    /// return `None` — they only ever knew the status as prose.
    pub fn http_status_code(&self) -> Option<u16> {
        match self {
            Error::HttpStatus { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The stable `LHxxxx` code for this error (see [`crate::error_codes`]).
    /// [`Error::HttpStatus`] classifies STRUCTURALLY off its real status code
    /// ([`crate::error_codes::classify_http`]); the legacy string-wrapping
    /// variants (`Http`/`ToolFailed`/`Other`) fall back to substring parsing
    /// via [`crate::error_codes::classify`] so an upstream "429"/"401"/… body
    /// resolves to the right `LH3xxx` backend code; every other variant maps to
    /// its own `LH4xxx` core code. Always resolves to a registered code.
    pub fn code(&self) -> u16 {
        use crate::error_codes as ec;
        match self {
            Error::Io(_) => ec::CORE_IO,
            Error::Json(_) => ec::CORE_JSON,
            Error::Http(s) => ec::classify(s).unwrap_or(ec::CORE_HTTP),
            Error::HttpStatus { status, message } => {
                ec::classify_http(*status, message).unwrap_or(ec::CORE_HTTP)
            }
            Error::Closed => ec::CORE_CLOSED,
            Error::NotStarted => ec::CORE_NOT_STARTED,
            Error::AlreadyStarted => ec::CORE_ALREADY_STARTED,
            Error::Config(_) => ec::CORE_CONFIG,
            Error::ToolNotFound { .. } => ec::CORE_TOOL_NOT_FOUND,
            Error::ToolFailed { message, .. } => {
                ec::classify(message).unwrap_or(ec::CORE_TOOL_FAILED)
            }
            Error::PolicyDenied(_) => ec::CORE_POLICY_DENIED,
            Error::Timeout(_) => ec::CORE_TIMEOUT,
            Error::Other(s) => ec::classify(s).unwrap_or(ec::CORE_OTHER),
        }
    }

    /// The canonical `LHxxxx` label for this error, e.g. `"LH4009"`.
    pub fn code_label(&self) -> String {
        crate::error_codes::fmt_label(self.code())
    }
}

/// Convenience alias for `std::result::Result<T, localharness::Error>`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_maps_to_a_registered_code() {
        let variants = [
            Error::Io(std::io::Error::other("x")),
            Error::Json(serde_json::from_str::<i32>("nope").unwrap_err()),
            Error::Http("boom".into()),
            Error::http_status(500, "boom"),
            Error::Closed,
            Error::NotStarted,
            Error::AlreadyStarted,
            Error::Config("c".into()),
            Error::ToolNotFound { name: "t".into() },
            Error::ToolFailed { name: "t".into(), message: "m".into() },
            Error::PolicyDenied("p".into()),
            Error::Timeout(Duration::from_secs(1)),
            Error::other("o"),
        ];
        for e in &variants {
            assert!(
                crate::error_codes::lookup(e.code()).is_some(),
                "{:?} -> {} is not in the registry",
                e,
                e.code_label()
            );
        }
    }

    #[test]
    fn string_wrapping_variants_classify_to_backend_codes() {
        use crate::error_codes as ec;
        assert_eq!(Error::Http("HTTP 429 too many requests".into()).code(), ec::BACKEND_RATE_LIMIT);
        assert_eq!(Error::other("402 no $LH").code(), ec::BACKEND_CREDITS);
        // An unmatched body falls back to the variant's own core code.
        assert_eq!(Error::Http("plain".into()).code(), ec::CORE_HTTP);
        assert_eq!(Error::other("plain").code(), ec::CORE_OTHER);
    }

    #[test]
    fn http_status_classifies_off_the_real_status() {
        use crate::error_codes as ec;
        // The status FIELD decides — no "429" substring needed in the body.
        assert_eq!(Error::http_status(429, "gemini HTTP <opaque>").code(), ec::BACKEND_RATE_LIMIT);
        assert_eq!(Error::http_status(401, "nope").code(), ec::BACKEND_AUTH);
        assert_eq!(Error::http_status(402, "x").code(), ec::BACKEND_CREDITS);
        assert_eq!(Error::http_status(503, "x").code(), ec::BACKEND_SERVER);
        // A stale-device-clock body overrides the 401 (must NOT read as bad key).
        assert_eq!(
            Error::http_status(401, "stale or future timestamp").code(),
            ec::BACKEND_STALE_AUTH
        );
        // Unmapped status → body string classification → core fallback.
        assert_eq!(Error::http_status(400, "API key not valid").code(), ec::BACKEND_AUTH);
        assert_eq!(Error::http_status(418, "teapot").code(), ec::CORE_HTTP);
    }

    #[test]
    fn http_status_display_and_accessor() {
        let e = Error::http_status(429, "gemini HTTP 429 Too Many Requests: quota");
        // Display prints the message VERBATIM — byte-identical to the legacy
        // `Error::other(...)` surface these errors used to ride.
        assert_eq!(e.to_string(), "gemini HTTP 429 Too Many Requests: quota");
        assert_eq!(e.http_status_code(), Some(429));
        assert_eq!(Error::other("x").http_status_code(), None);
        assert_eq!(Error::Http("x".into()).http_status_code(), None);
    }
}
