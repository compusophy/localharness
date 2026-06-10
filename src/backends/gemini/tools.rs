//! Re-export shim — the built-in tool registry moved to [`crate::builtins`].
//!
//! The builtins are backend-NEUTRAL (every backend registers from them), so
//! they now live at the crate root instead of inside the Gemini backend
//! (their historical home — Gemini was written first). This shim keeps every
//! existing `crate::backends::gemini::tools::...` import path compiling.

pub use crate::builtins::*;
