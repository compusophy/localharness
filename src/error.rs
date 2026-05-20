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

    #[error("protobuf encode: {0}")]
    ProtoEncode(#[from] prost::EncodeError),

    #[error("protobuf decode: {0}")]
    ProtoDecode(#[from] prost::DecodeError),

    #[error("websocket: {0}")]
    WebSocket(String),

    #[error("connection closed")]
    Closed,

    #[error("not started")]
    NotStarted,

    #[error("already started")]
    AlreadyStarted,

    #[error("harness binary not found (set ANTIGRAVITY_HARNESS_PATH or place 'localharness' on PATH)")]
    BinaryNotFound,

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

impl From<tokio_tungstenite::tungstenite::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::Error) -> Self {
        Self::WebSocket(e.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::http::Error> for Error {
    fn from(e: tokio_tungstenite::tungstenite::http::Error) -> Self {
        Self::WebSocket(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
