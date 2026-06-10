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
