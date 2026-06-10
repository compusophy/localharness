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

/// Shared SSE frame decoder (blank-line frame splitting, `data:` payload
/// extraction, CRLF+LF tolerance, EOF flush) used by the Gemini and Anthropic
/// streaming clients. Backend event parsing stays in each backend's `api.rs`.
pub(crate) mod sse;

/// The shared tool/hook/session runner bundle the Agent injects into every
/// backend strategy ([`BackendRunners`]).
mod runners;
pub use runners::BackendRunners;

/// The shared tool-dispatch pipeline (pre-hook → execute → error-lift →
/// post-hook) every backend funnels its inline tool calls through.
pub(crate) mod dispatch;

/// The generic context-compaction fold engine (rolling summary + recent
/// keep-window) shared by the Gemini and Anthropic backends. Each backend's
/// `compaction.rs` is a thin adapter supplying the wire-message seam
/// ([`compaction::CompactionModel`]) and its summarization request.
pub(crate) mod compaction;

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
#[cfg(any(feature = "anthropic", feature = "local"))]
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
/// `translate_error_steps` selects each backend's CURRENT error-step
/// behavior, preserved exactly as found:
///
/// * `true` (anthropic, local) — a System-sourced, Error-status Step with a
///   non-empty message (a turn failure from `emit_error`) converts into a
///   stream `Err` carrying the real message, so the failure propagates to
///   `chat()`/`text()` instead of being swallowed as an empty success.
/// * `false` (gemini, mock) — such Steps pass through as `Ok`.
///
/// The inconsistency is deliberate-as-found; unifying it is a behavior
/// decision, not plumbing.
pub(crate) fn subscribe_step_stream(
    rx: tokio::sync::broadcast::Receiver<Step>,
    label: &'static str,
    translate_error_steps: bool,
) -> StepStream {
    let mapped = BroadcastStream::new(rx).map(move |r| match r {
        Ok(step)
            if translate_error_steps
                && step.source == StepSource::System
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
