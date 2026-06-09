/// Token types (keywords, operators, literals).
pub mod token;
/// Byte-level lexer with string escapes.
pub mod lexer;
/// Full AST (structs, enums, functions, match, etc.).
pub mod ast;
/// Recursive-descent parser with precedence climbing.
pub mod parser;
/// Scope-based type resolution and mutability checking.
pub mod typecheck;
/// Wasm binary emitter (sections, opcodes, LEB128).
pub mod codegen;
/// Wasm32-only cartridge instantiation via `WebAssembly`.
pub mod loader;

/// Compile a Rust-subset source string into wasm bytes.
///
/// Pipeline: lex -> parse -> typecheck -> codegen.
pub fn compile(source: &str) -> Result<Vec<u8>, CompileError> {
    let tokens = lexer::lex(source)?;
    let module = parser::parse(&tokens)?;
    let typed = typecheck::check(&module)?;
    let wasm = codegen::emit(&typed)?;
    Ok(wasm)
}

/// An error produced during compilation (lex, parse, typecheck, or codegen).
#[derive(Debug, Clone)]
pub struct CompileError {
    /// Human-readable error description.
    pub message: String,
    /// Source location, if available.
    pub span: Option<Span>,
    /// Stable `LH0xxx` registry code (see [`crate::error_codes`]). `None` only
    /// for the rare uncoded internal error; `Display` prefixes the `LHxxxx:`
    /// label when present so every surfaced compile error carries its code.
    pub code: Option<u16>,
}

/// A byte-offset range in the source text.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Prefix the stable `LHxxxx:` code when known, so a surfaced compile
        // error reads e.g. "LH0204: type mismatch: ... [12..18]".
        if let Some(code) = self.code {
            write!(f, "{}: ", crate::error_codes::fmt_label(code))?;
        }
        if let Some(span) = self.span {
            write!(f, "{} [{}..{}]", self.message, span.start, span.end)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for CompileError {}

impl CompileError {
    /// Create an error with no source span and no code.
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), span: None, code: None }
    }
    /// Create an error pinned to a source span (no code).
    pub fn at(message: impl Into<String>, span: Span) -> Self {
        Self { message: message.into(), span: Some(span), code: None }
    }
    /// Create a coded error pinned to a source span — the canonical
    /// constructor. `code` is an `LH0xxx` value from [`crate::error_codes`].
    pub fn at_code(code: u16, message: impl Into<String>, span: Span) -> Self {
        Self { message: message.into(), span: Some(span), code: Some(code) }
    }
    /// Create a coded error with no source span.
    pub fn new_code(code: u16, message: impl Into<String>) -> Self {
        Self { message: message.into(), span: None, code: Some(code) }
    }
    /// Attach (or replace) the stable code on an existing error.
    pub fn with_code(mut self, code: u16) -> Self {
        self.code = Some(code);
        self
    }
}

impl From<String> for CompileError {
    fn from(s: String) -> Self { Self::new(s) }
}

#[cfg(test)]
mod tests {
    use super::{compile, lexer, parser, typecheck};
    use crate::error_codes as codes;

    /// Drive the full pipeline and return the `CompileError` (so the test can
    /// inspect its `.code`). Each snippet below is crafted to fail at exactly
    /// one stage.
    fn compile_err(src: &str) -> super::CompileError {
        lexer::lex(src)
            .and_then(|toks| parser::parse(&toks))
            .and_then(|m| typecheck::check(&m))
            .and_then(|t| super::codegen::emit(&t))
            .expect_err("expected a compile error")
    }

    #[test]
    fn compile_errors_carry_their_lh0xxx_code() {
        // A representative bad snippet per stage → its expected LH0xxx code.
        // type mismatch (typecheck): bool + i32.
        let e = compile_err("fn frame(t: i32) { let x = true + 1; host::display::present(); }");
        assert_eq!(e.code, Some(codes::TYPE_MISMATCH), "{e}");
        assert!(e.to_string().starts_with("LH0204:"), "surfaced: {e}");

        // undefined variable (typecheck).
        let e = compile_err("fn frame(t: i32) { host::display::clear(NOPE); host::display::present(); }");
        assert_eq!(e.code, Some(codes::UNDEFINED_VARIABLE), "{e}");

        // unexpected token (parser): missing the fn name.
        let e = compile_err("fn (t: i32) {}");
        assert_eq!(e.code, Some(codes::UNEXPECTED_TOKEN), "{e}");

        // invalid assignment target / array write (parser).
        let e = compile_err("fn frame(t: i32) { let a = [1, 2, 3]; a[0] = 9; host::display::present(); }");
        assert_eq!(e.code, Some(codes::INVALID_ASSIGN_TARGET), "{e}");

        // unknown function (codegen): a host fn path that doesn't resolve —
        // an unknown `host::` name falls through to "undefined function" rather
        // than the registered-but-missing host-import path (LH0301).
        let e = compile_err("fn frame(t: i32) { host::display::nope(1); host::display::present(); }");
        assert_eq!(e.code, Some(codes::UNKNOWN_FUNCTION), "{e}");

        // unexpected byte (lexer).
        let e = compile_err("fn frame(t: i32) { let x = `; }");
        assert_eq!(e.code, Some(codes::UNEXPECTED_BYTE), "{e}");

        // bad cast (typecheck): bool as i32.
        let e = compile_err("fn frame(t: i32) { let x = true as i32; host::display::clear(x); host::display::present(); }");
        assert_eq!(e.code, Some(codes::BAD_CAST), "{e}");

        // Every surfaced compile error string is LHxxxx-prefixed.
        assert!(e.to_string().starts_with("LH0"), "surfaced: {e}");
    }

    #[test]
    fn const_resolves_and_is_order_independent() {
        // const used in a fn declared BEFORE the const — resolution must not
        // depend on source order.
        assert!(compile(
            "fn frame(t: i32) { host::display::clear(W); host::display::present(); } const W: i32 = 256;"
        )
        .is_ok());
        // a const referencing an earlier const
        assert!(compile(
            "const A: i32 = 2; const B: i32 = A * 3; fn frame(t: i32) { host::display::clear(B); host::display::present(); }"
        )
        .is_ok());
        // a genuinely undefined name still errors
        assert!(compile(
            "fn frame(t: i32) { host::display::clear(NOPE); host::display::present(); }"
        )
        .is_err());
    }

    #[test]
    fn casts_between_numbers() {
        // i32 → f64 → i32 round-trip + a float literal truncated to i32 (the
        // common graphics pattern: float math, then cast to a pixel coord).
        assert!(compile(
            "fn frame(t: i32) { let x = t as f64; let y = x as i32; host::display::clear(y + (3.7 as i32)); host::display::present(); }"
        )
        .is_ok());
    }

    #[test]
    fn arrays_literal_and_index() {
        // array literal + variable-index read (the lookup-table pattern)
        assert!(compile(
            "fn frame(t: i32) { let pal = [16711680, 65280, 255]; host::display::clear(pal[t % 3]); host::display::present(); }"
        )
        .is_ok());
        // indexing a non-array is a clear error
        assert!(compile(
            "fn frame(t: i32) { let x = 5; host::display::clear(x[0]); host::display::present(); }"
        )
        .is_err());
        // writes `arr[i] = v` are not yet supported — clean error, not a crash
        assert!(compile(
            "fn frame(t: i32) { let a = [1, 2, 3]; a[0] = 9; host::display::present(); }"
        )
        .is_err());
    }
}
