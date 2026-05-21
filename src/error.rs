//! Typed error hierarchy for the SDK.
//!
//! Every fallible boundary returns `Result<T>` aliased to this crate's
//! `Error`. Wrapping inner errors via `#[from]` keeps call sites terse
//! (`foo()?`) while preserving the underlying source for `tracing` and
//! `std::error::Error::source` walks.

use std::time::Duration;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http: {0}")]
    Http(String),

    #[error("connection closed")]
    Closed,

    #[error("not started")]
    NotStarted,

    #[error("already started")]
    AlreadyStarted,

    #[error("config: {0}")]
    Config(String),

    #[error("tool '{name}' not found")]
    ToolNotFound { name: String },

    #[error("tool '{name}' failed: {message}")]
    ToolFailed { name: String, message: String },

    #[error("policy denied: {0}")]
    PolicyDenied(String),

    #[error("timed out after {0:?}")]
    Timeout(Duration),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
