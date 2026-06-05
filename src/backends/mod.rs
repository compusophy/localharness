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

pub mod gemini;
/// Anthropic (Claude Messages API) backend — a second `ConnectionStrategy`
/// behind the same Layer-3 seam. Gated on the `anthropic` feature so it's
/// purely additive (off by default).
#[cfg(feature = "anthropic")]
pub mod anthropic;
#[cfg(feature = "native")]
pub mod mcp;
