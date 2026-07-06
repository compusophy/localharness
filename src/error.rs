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
    #[deprecated(
        since = "0.69.0",
        note = "never constructed by the SDK; use Error::http_status (non-2xx with a \
                real status) or Error::transport (request/stream transport failure)"
    )]
    #[error("http: {0}")]
    Http(String),

    /// A network/transport failure reaching a backend: the request POST
    /// failed, or a mid-stream chunk read died. Like [`Error::HttpStatus`],
    /// `Display` prints the message verbatim so surfaced text is byte-identical
    /// to the legacy `Other` path. Constructed via [`Error::transport`].
    #[error("{0}")]
    Transport(String),

    /// A payload failed to decode (a provider JSON body / SSE frame, restored
    /// history bytes). `what` names the codec boundary; `Display` prints
    /// `"{what}: {message}"` — byte-identical to the legacy `Other` strings.
    /// Constructed via [`Error::decode`].
    #[error("{what}: {message}")]
    Decode {
        /// The codec boundary that failed (e.g. `"gemini JSON"`).
        what: String,
        /// The decoder's error text (may embed the offending payload).
        message: String,
    },

    /// A filesystem operation failed (the [`crate::filesystem::Filesystem`]
    /// impls: native, OPFS, encrypted, rooted). `op` names the operation
    /// (e.g. `"read"`, `"removeEntry"`), `path` the target when known;
    /// `message` is the full user-facing text and `Display` prints it
    /// verbatim (the legacy strings are heterogeneous — the [`Error::HttpStatus`]
    /// precedent), so surfaced text is byte-identical to the legacy `Other`
    /// path. Constructed via [`Error::fs`].
    #[error("{message}")]
    Fs {
        /// The filesystem operation that failed (e.g. `"read"`).
        op: String,
        /// The path/name involved (empty when the failure has no target).
        path: String,
        /// The full formatted error message (what the user sees).
        message: String,
    },

    /// A tool rejected its ARGUMENTS before doing any work: the args JSON
    /// failed to deserialize, or an argument value is invalid on its face
    /// (empty `old_string`, a malformed glob/regex). `Display` prints
    /// `message` verbatim; `tool` rides alongside structurally (mirroring
    /// [`Error::ToolFailed`]'s `name`). Constructed via [`Error::bad_args`].
    #[error("{message}")]
    BadArgs {
        /// The tool whose arguments were rejected.
        tool: String,
        /// The full formatted error message (what the model sees).
        message: String,
    },

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

    /// Construct a transport error (request send / chunk read died). `msg` is
    /// surfaced verbatim.
    pub fn transport(msg: impl Into<String>) -> Self {
        Self::Transport(msg.into())
    }

    /// Construct a decode error: `what` names the codec boundary, `message`
    /// the decoder's error text. Surfaces as `"{what}: {message}"`.
    pub fn decode(what: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Decode { what: what.into(), message: message.into() }
    }

    /// Construct a filesystem-operation error: `op` names the operation,
    /// `path` the target (empty when none); `message` is surfaced verbatim.
    pub fn fs(
        op: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self::Fs { op: op.into(), path: path.into(), message: message.into() }
    }

    /// Construct a bad-arguments error for `tool`; `message` is surfaced
    /// verbatim.
    pub fn bad_args(tool: impl Into<String>, message: impl Into<String>) -> Self {
        Self::BadArgs { tool: tool.into(), message: message.into() }
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
    /// resolves to the right `LH3xxx` backend code; [`Error::Transport`] /
    /// [`Error::Decode`] classify the same way but fall back to
    /// `BACKEND_NETWORK` / `CORE_DECODE`; [`Error::Fs`] / [`Error::BadArgs`]
    /// map STRUCTURALLY (no substring pass — see the arms); every other
    /// variant maps to its own `LH4xxx` core code. Always resolves to a
    /// registered code.
    pub fn code(&self) -> u16 {
        use crate::error_codes as ec;
        match self {
            Error::Io(_) => ec::CORE_IO,
            Error::Json(_) => ec::CORE_JSON,
            #[allow(deprecated)]
            Error::Http(s) => ec::classify(s).unwrap_or(ec::CORE_HTTP),
            // Transport failures classify off the message (a "429" body / bare
            // "error sending request" keeps its precise LH3xxx class) and fall
            // back to the network class — an unmatched transport failure IS a
            // network failure, so the #29 stream-open retry treats it as
            // transient instead of failing fast on CORE_OTHER.
            Error::Transport(s) => ec::classify(s).unwrap_or(ec::BACKEND_NETWORK),
            Error::Decode { message, .. } => ec::classify(message).unwrap_or(ec::CORE_DECODE),
            // Fs maps STRAIGHT to CORE_IO — deliberately NO classify() pass:
            // fs messages embed user paths + OS/JS error prose that false-
            // positive the backend patterns (an OPFS "QuotaExceededError"
            // read as a provider rate-limit through the old Other path; a
            // filename containing "429" would too). A filesystem failure is
            // never a provider failure.
            Error::Fs { .. } => ec::CORE_IO,
            // Same rationale: arg text is model-authored (paths/globs echoed
            // back) and must never substring-classify into LH3xxx. Shares
            // CORE_TOOL_FAILED with ToolFailed — no consumer branches on the
            // distinction, so a new code doesn't pay (the LH4013 bar).
            Error::BadArgs { .. } => ec::CORE_TOOL_FAILED,
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
    #[allow(deprecated)]
    fn every_variant_maps_to_a_registered_code() {
        let variants = [
            Error::Io(std::io::Error::other("x")),
            Error::Json(serde_json::from_str::<i32>("nope").unwrap_err()),
            Error::Http("boom".into()),
            Error::http_status(500, "boom"),
            Error::transport("boom"),
            Error::decode("gemini JSON", "boom"),
            Error::fs("read", "x.txt", "read(x.txt): boom"),
            Error::bad_args("edit_file", "edit_file args: boom"),
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
    #[allow(deprecated)]
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

    /// The typed transport/decode variants (slice A of the Error migration):
    /// `Display` stays byte-identical to the legacy `Error::other` strings,
    /// classification keeps the precise LH3xxx class when the message carries
    /// one, and the fallbacks are BACKEND_NETWORK (transport IS a network
    /// failure — retryable) / CORE_DECODE.
    #[test]
    fn transport_and_decode_display_and_classify() {
        use crate::error_codes as ec;
        let t = Error::transport("gemini POST: error sending request");
        assert_eq!(t.to_string(), "gemini POST: error sending request");
        assert_eq!(t.code(), ec::BACKEND_SEND); // #41 retry-once class preserved
        assert_eq!(Error::transport("gemini chunk read: <opaque>").code(), ec::BACKEND_NETWORK);
        assert_eq!(Error::transport("x").http_status_code(), None);

        let d = Error::decode("gemini JSON", "expected value at line 1");
        assert_eq!(d.to_string(), "gemini JSON: expected value at line 1");
        assert_eq!(d.code(), ec::CORE_DECODE);
        // A payload echo that names a backend cause keeps its LH3xxx class.
        assert_eq!(
            Error::decode("gemini sse decode", "eof; payload: exceeded your quota").code(),
            ec::BACKEND_RATE_LIMIT
        );
    }

    /// Slice B of the Error migration: backend client-construction failures
    /// ride the existing `Config` variant (its established `"config: "` Display
    /// prefix is the ONE deliberate text change of the slice), MCP stdio
    /// process/pipe failures + a mid-stream stall are `Transport` (LH3007
    /// network fallback), and MCP response decodes are `Decode`.
    #[test]
    fn slice_b_config_transport_decode_classification() {
        use crate::error_codes as ec;
        let c = Error::config("invalid model url: relative URL without a base");
        assert_eq!(c.to_string(), "config: invalid model url: relative URL without a base");
        assert_eq!(c.code(), ec::CORE_CONFIG);
        assert_eq!(Error::transport("model stream stalled — no data for 90s").code(), ec::BACKEND_NETWORK);
        assert_eq!(Error::transport("mcp spawn 'srv': No such file or directory").code(), ec::BACKEND_NETWORK);
        let d = Error::decode("tools/call decode", "missing field `content`");
        assert_eq!(d.to_string(), "tools/call decode: missing field `content`");
        assert_eq!(d.code(), ec::CORE_DECODE);
    }

    /// Slice C1 of the Error migration: filesystem-op failures and builtin
    /// bad-args are typed, `Display` verbatim, and classify STRUCTURALLY —
    /// deliberately NO substring pass, so OS/JS/model-authored prose can no
    /// longer false-positive into an LH3xxx backend class (an OPFS
    /// QuotaExceededError used to read as a provider rate-limit).
    #[test]
    fn slice_c_fs_and_bad_args_classification() {
        use crate::error_codes as ec;
        let f = Error::fs("write", "a.txt", "write(a.txt): QuotaExceededError: quota exceeded");
        assert_eq!(f.to_string(), "write(a.txt): QuotaExceededError: quota exceeded");
        assert_eq!(f.code(), ec::CORE_IO); // NOT BACKEND_RATE_LIMIT despite "quota"
        assert_eq!(Error::fs("read", "429.log", "read(429.log): denied").code(), ec::CORE_IO);

        let b = Error::bad_args("edit_file", "edit_file args: missing field `path`");
        assert_eq!(b.to_string(), "edit_file args: missing field `path`");
        assert_eq!(b.code(), ec::CORE_TOOL_FAILED);
        // Model-echoed arg text must not classify either.
        assert_eq!(
            Error::bad_args("find_file", "invalid glob 'quota-**': nested `**`").code(),
            ec::CORE_TOOL_FAILED
        );
    }

    /// Slice C2 of the Error migration (src/app chat tools — no new variants):
    /// arg-validation rides `BadArgs` (structural CORE_TOOL_FAILED — a
    /// model-echoed amount/name/script containing "429"/"quota" can no longer
    /// read as a backend failure), and the proxy notify/web_fetch failures ride
    /// `HttpStatus` (the real number decides: a metered 402 IS out-of-credits).
    /// Chain/RPC/tx prose deliberately stays `Other` — its substring pass is
    /// load-bearing ("insufficient … $LH" → the credits class + hint).
    #[test]
    fn slice_c2_app_tool_sites_classification() {
        use crate::error_codes as ec;
        let b = Error::bad_args("send_lh", "could not parse amount \"429 quota\" — pass a decimal $LH figure like \"5\" or \"1.5\"");
        assert_eq!(b.to_string(), "could not parse amount \"429 quota\" — pass a decimal $LH figure like \"5\" or \"1.5\"");
        assert_eq!(b.code(), ec::CORE_TOOL_FAILED); // NOT BACKEND_RATE_LIMIT
        assert_eq!(Error::http_status(402, "notify bob failed (402): no $LH").code(), ec::BACKEND_CREDITS);
        // The chain-prose class kept on Other: the substring pass is correct here.
        assert_eq!(Error::other("send_lh failed: insufficient $LH balance").code(), ec::BACKEND_CREDITS);
    }

    #[test]
    #[allow(deprecated)]
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
