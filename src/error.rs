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
}

/// Convenience alias for `std::result::Result<T, localharness::Error>`.
pub type Result<T, E = Error> = std::result::Result<T, E>;
