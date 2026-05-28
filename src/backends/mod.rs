//! Backend implementations of the [`Connection`] trait.
//!
//! Each backend is the runtime that turns a user prompt into model
//! responses. The Connection trait is the abstraction boundary; backends
//! never leak into Agent/Conversation code.
//!
//! | Backend  | Status | Notes                                |
//! |----------|--------|--------------------------------------|
//! | `gemini` | stable | Rust-native; hits the Gemini REST API |
//! | `mcp`    | native | stdio bridge to MCP servers          |
//!
//! [`Connection`]: crate::connections::Connection

pub mod gemini;
#[cfg(feature = "native")]
pub mod mcp;
