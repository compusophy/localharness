//! Backend implementations of the [`Connection`] trait.
//!
//! Each backend is the runtime that turns a user prompt into model
//! responses. The Connection trait is the abstraction boundary; backends
//! never leak into Agent/Conversation code.
//!
//! | Backend  | Status | Path  | Notes |
//! |----------|--------|-------|-------|
//! | `gemini` | alpha  | [`gemini`] | Rust-native; hits the Gemini REST API |
//!
//! The 0.1.x `LocalConnection` backend (which proxied to Google's Go
//! `localharness` binary) lives under [`crate::connections::local`] and is
//! kept for source compatibility through 0.2.x.
//!
//! [`Connection`]: crate::connections::Connection

pub mod gemini;
pub mod mcp;
