//! Backend implementations of the [`Connection`] trait.
//!
//! Each backend is the runtime that turns a user prompt into model
//! responses. The Connection trait is the abstraction boundary; backends
//! never leak into Agent/Conversation code.
//!
//! | Backend     | Status   | Notes                                       |
//! |-------------|----------|---------------------------------------------|
//! | `gemini`    | stable   | Rust-native; hits the Gemini REST API       |
//! | `anthropic` | feature  | Rust-native; hits the Anthropic Messages API (feature `anthropic`) |
//! | `mcp`       | native   | stdio bridge to MCP servers                 |
//!
//! [`Connection`]: crate::connections::Connection

/// Idle (stall) timeout shared by the streaming backend turn loops: races
/// each per-chunk `stream.next()` against a freshly-armed sleep so a stream
/// parked on a silent socket ERRORS the turn (recoverable) instead of hanging
/// forever, while a steadily streaming response is unaffected.
mod stream_timeout;

/// Shared stream-OPEN retry policy (transient 5xx / transport / timeout only;
/// auth/credits/rate-limit fail fast). Used by both turn loops + the subagent.
pub mod retry;

/// Shared SSE frame decoder (blank-line frame splitting, `data:` payload
/// extraction, CRLF+LF tolerance, EOF flush) used by the Gemini and Anthropic
/// streaming clients. Backend event parsing stays in each backend's `api.rs`.
pub(crate) mod sse;

/// The shared tool/hook/session runner bundle the Agent injects into every
/// backend strategy ([`BackendRunners`]).
mod runners;
pub use runners::BackendRunners;

/// The generic per-connection `LoopState<M>` container + opaque-history JSON
/// codecs shared by the streaming backends (Gemini/Anthropic/OpenAI). Each
/// backend's `LoopState` is a thin type alias over `state::LoopState<wire msg>`.
pub(crate) mod state;

/// Per-request API-key provider shared by the streaming clients. When set,
/// a client calls it for EVERY HTTP request instead of using its static
/// key — required for credential schemes with a freshness window (the `$LH`
/// credit proxy rejects signed auth tokens older than 5 minutes, so a token
/// baked in at session start goes stale mid-conversation; the provider
/// re-signs per request).
#[cfg(not(target_arch = "wasm32"))]
pub type KeyProvider = std::sync::Arc<dyn Fn() -> String + Send + Sync>;
#[cfg(target_arch = "wasm32")]
pub type KeyProvider = std::sync::Arc<dyn Fn() -> String>;

/// `Debug`-opaque, `Clone`-able wrapper for a [`KeyProvider`] so backend
/// configs (which derive `Debug`) can carry one.
#[derive(Clone)]
pub struct AuthTokenProvider(pub KeyProvider);

impl std::fmt::Debug for AuthTokenProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthTokenProvider(<closure>)")
    }
}

/// The shared tool-dispatch pipeline (pre-hook → execute → error-lift →
/// post-hook) every backend funnels its inline tool calls through.
pub(crate) mod dispatch;

/// Small helpers shared by the streaming-backend turn loops (canonical-path
/// resolution, the malformed-args convention). Unconditional: the always-on
/// turn engine uses `extract_canonical_path`; `resolve_tool_args` gates
/// internally on `any(test, feature = "anthropic", feature = "openai")` —
/// its consumers (gemini's args arrive parsed and never need it).
pub(crate) mod loop_util;

/// The generic context-compaction fold engine (rolling summary + recent
/// keep-window) shared by the Gemini and Anthropic backends. Each backend's
/// `compaction.rs` is a thin adapter supplying the wire-message seam
/// ([`compaction::CompactionModel`]) and its summarization request.
pub(crate) mod compaction;

/// The generic streaming TURN ENGINE (R7, complete): ONE copy of the
/// turn-loop scaffold behind a static-dispatch [`turn_engine::TurnProvider`]
/// seam (the `CompactionModel` pattern; async edges ride in as closures —
/// wasm-safe by construction). ALL THREE streaming loops ride it —
/// gemini (always-on default), anthropic, openai — so it's unconditional;
/// a scaffold fix lands here ONCE.
pub(crate) mod turn_engine;

pub mod gemini;
/// Deterministic, offline mock backend for testing agents — a scripted
/// `ConnectionStrategy` that replays fixed model turns with no network, key,
/// or LLM. Always available (no feature flag): pulls no new deps and compiles
/// on every target, so the crate's own tests and consumers' dev-deps both use it.
pub mod mock;
/// Anthropic (Claude Messages API) backend — a second `ConnectionStrategy`
/// behind the same Layer-3 seam. Gated on the `anthropic` feature so it's
/// purely additive (off by default).
#[cfg(feature = "anthropic")]
pub mod anthropic;
/// OpenAI (Chat Completions API) backend — a `ConnectionStrategy` behind the
/// same Layer-3 seam. Gated on the `openai` feature so it's purely additive
/// (off by default).
#[cfg(feature = "openai")]
pub mod openai;
/// Local in-browser model backend — Gemma 3 270M via Burn's wgpu/WebGPU
/// backend. Gated on the `local` feature (heavy: pulls the Burn framework).
#[cfg(feature = "local")]
pub mod local;
#[cfg(feature = "native")]
pub mod mcp;

use futures_util::stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::connections::StepStream;
use crate::error::Error;
use crate::types::{Step, StepSource, StepStatus};

/// Flatten [`SystemInstructions`](crate::types::SystemInstructions) into a
/// plain system-preamble string. Shared VERBATIM by the Anthropic backend
/// (top-level `system`) and the local backend (prompt preamble); the Gemini
/// backend keeps its own near-variant, which wraps the same flattening in a
/// wire `Content` instead of returning the string.
#[cfg(any(feature = "anthropic", feature = "local", feature = "openai"))]
pub(crate) fn render_system(s: &crate::types::SystemInstructions) -> String {
    use crate::types::SystemInstructions;
    match s {
        SystemInstructions::Custom(c) => c.text.clone(),
        SystemInstructions::Templated(t) => {
            let mut buf = String::new();
            if let Some(id) = &t.identity {
                buf.push_str(id);
                buf.push_str("\n\n");
            }
            for section in &t.sections {
                if !section.title.is_empty() {
                    buf.push_str("## ");
                    buf.push_str(&section.title);
                    buf.push('\n');
                }
                buf.push_str(&section.content);
                buf.push_str("\n\n");
            }
            buf.trim().to_string()
        }
    }
}

/// Shared `subscribe_steps` plumbing: wrap a broadcast receiver as a
/// [`StepStream`] — boxed `Send` on native, boxed local on wasm — with the
/// backend's lag error labelled `"{label} step lag: ..."`.
///
/// A System-sourced, Error-status Step with a non-empty message (a turn
/// failure from `emit_error` / `Step::turn_error`: HTTP non-200, stream
/// decode failure, in-stream error event) converts into a stream `Err`
/// carrying the real message, so the failure propagates to `chat()`/`text()`
/// instead of being swallowed as an empty success — the "(empty response)"
/// bug class. Uniform across ALL backends since the gemini/mock flip;
/// Model-sourced terminal Steps (safety/refusal stops) pass through as `Ok`.
pub(crate) fn subscribe_step_stream(
    rx: tokio::sync::broadcast::Receiver<Step>,
    label: &'static str,
) -> StepStream {
    let mapped = BroadcastStream::new(rx).map(move |r| match r {
        Ok(step)
            if step.source == StepSource::System
                && step.status == StepStatus::Error
                && !step.error.is_empty() =>
        {
            Err(Error::other(step.error))
        }
        Ok(step) => Ok(step),
        Err(e) => Err(Error::other(format!("{label} step lag: {e}"))),
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        mapped.boxed()
    }
    #[cfg(target_arch = "wasm32")]
    {
        mapped.boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    /// REGRESSION (the gemini/mock unification): a System-sourced,
    /// Error-status Step with a message — exactly what `Step::turn_error`
    /// broadcasts on a turn failure — MUST become a stream `Err` for EVERY
    /// backend, or `chat()`/`text()` swallow the failure as an empty
    /// success ("(empty response)" with no cause). Gemini and mock used to
    /// pass these through as `Ok`.
    #[tokio::test]
    async fn turn_error_step_translates_to_stream_err() {
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        let mut stream = subscribe_step_stream(rx, "test");

        tx.send(Step::turn_error(0, "gemini HTTP 500: boom"))
            .expect("subscriber is live");

        match stream.next().await.expect("a stream item") {
            Ok(step) => panic!("error Step leaked as Ok: {step:?}"),
            Err(Error::Other(msg)) => {
                assert!(msg.contains("gemini HTTP 500: boom"), "got: {msg}")
            }
            Err(other) => panic!("unexpected error variant: {other:?}"),
        }
    }

    /// Model-sourced terminal Steps (safety/refusal stops carry
    /// `StepStatus::Error` but `StepSource::Model`) must pass through as
    /// `Ok` — they are answers-with-caveats, not turn failures.
    #[tokio::test]
    async fn model_error_status_step_passes_through() {
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        let mut stream = subscribe_step_stream(rx, "test");

        let step = Step::turn_complete(
            "t",
            0,
            StepStatus::Error,
            "",
            "stopped by safety policy",
            false,
            None,
            None,
        );
        tx.send(step).expect("subscriber is live");

        match stream.next().await.expect("a stream item") {
            Ok(step) => assert_eq!(step.status, StepStatus::Error),
            Err(e) => panic!("Model-sourced step wrongly translated: {e:?}"),
        }
    }
}
