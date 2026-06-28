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

    /// The stable `LHxxxx` code for this error (see [`crate::error_codes`]). The
    /// string-wrapping variants (`Http`/`ToolFailed`/`Other`) defer to
    /// [`crate::error_codes::classify`] so an upstream "429"/"401"/… body
    /// resolves to the right `LH3xxx` backend code; every other variant maps to
    /// its own `LH4xxx` core code. Always resolves to a registered code.
    pub fn code(&self) -> u16 {
        use crate::error_codes as ec;
        match self {
            Error::Io(_) => ec::CORE_IO,
            Error::Json(_) => ec::CORE_JSON,
            Error::Http(s) => ec::classify(s).unwrap_or(ec::CORE_HTTP),
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
}
