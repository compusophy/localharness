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

        // invalid assignment target (parser): can't assign to a literal.
        // (Indexed array writes `a[0] = 9` ARE now supported — see
        // `arrays_literal_and_index`; this exercises the remaining reject path.)
        let e = compile_err("fn frame(t: i32) { 5 = 9; host::display::present(); }");
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
        // INDEXED WRITES `arr[i] = v` now compile (the stateful-grid primitive):
        // write then read back the SAME element in one frame.
        assert!(compile(
            "fn frame(t: i32) { let mut a = [1, 2, 3]; a[0] = 9; host::display::clear(a[0]); host::display::present(); }"
        )
        .is_ok());
        // a variable index on the write side, mutating from a host value
        assert!(compile(
            "fn frame(t: i32) { let mut a = [0, 0, 0, 0]; a[t % 4] = t; host::display::clear(a[t % 4]); host::display::present(); }"
        )
        .is_ok());
        // writing the wrong element type is still a type error (i32 elems only)
        assert!(compile(
            "fn frame(t: i32) { let mut a = [1, 2, 3]; a[0] = true; host::display::present(); }"
        )
        .is_err());
        // writing into a non-mut array binding is rejected (mutability holds)
        assert!(compile(
            "fn frame(t: i32) { let a = [1, 2, 3]; a[0] = 9; host::display::present(); }"
        )
        .is_err());
        // indexing a non-array on the write side is a clear error
        assert!(compile(
            "fn frame(t: i32) { let mut x = 5; x[0] = 9; host::display::present(); }"
        )
        .is_err());
    }

    #[test]
    fn array_params_and_repeat_init() {
        // `[T; N]` is now a TYPE — an array can be a fn PARAMETER (passed as its
        // i32 base pointer). A helper reads through the array param.
        assert!(compile(
            "fn sum3(a: [i32; 3]) -> i32 { a[0] + a[1] + a[2] } \
             fn frame(t: i32) { let g = [10, 20, 30]; host::display::clear(sum3(g)); host::display::present(); }"
        )
        .is_ok());
        // An array param can be MUTATED in the callee (shared backing — the
        // base pointer aliases the caller's region, C-style).
        assert!(compile(
            "fn set0(a: [i32; 3], v: i32) { a[0] = v; } \
             fn frame(t: i32) { let mut g = [0, 0, 0]; set0(g, 7); host::display::clear(g[0]); host::display::present(); }"
        )
        .is_ok());
        // `[v; N]` sized repeat init typechecks + the result is indexable.
        assert!(compile(
            "fn frame(t: i32) { let mut g = [0; 64]; g[5] = 9; host::display::clear(g[5]); host::display::present(); }"
        )
        .is_ok());
        // The repeat value need not be a literal (any i32 expr).
        assert!(compile(
            "fn frame(t: i32) { let g = [t * 2; 8]; host::display::clear(g[3]); host::display::present(); }"
        )
        .is_ok());
        // `[v; 0]` is rejected (empty arrays unsupported, same as `[]`).
        assert!(compile(
            "fn frame(t: i32) { let g = [0; 0]; host::display::present(); }"
        )
        .is_err());
        // Non-i32 array param element type is rejected (i32-only, v1).
        assert!(compile(
            "fn f(a: [bool; 2]) {} fn frame(t: i32) { host::display::present(); }"
        )
        .is_err());
        // Array repeat with a non-i32 value is rejected.
        assert!(compile(
            "fn frame(t: i32) { let g = [true; 4]; host::display::present(); }"
        )
        .is_err());
    }
}

/// Emit the indexed-array-write cartridges that the node run-proof
/// (`scripts/verify-array-write.mjs`) instantiates + runs. This is the bridge
/// the task asks for: "use a Rust test that compiles + writes the bytes for
/// node to load." Each cartridge ends its `frame` by `clear()`-ing with a value
/// it READ BACK out of an array it just WROTE; node asserts the value matches.
///
/// Run `cargo test emits_wasm_for_node_proof`, then `node
/// scripts/verify-array-write.mjs`. Native-only (writes to the source tree).
#[cfg(all(test, feature = "native"))]
mod array_write_run_proof {
    use super::compile;

    #[test]
    fn emits_wasm_for_node_proof() {
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join(".array-write-proof");
        std::fs::create_dir_all(&out_dir).expect("create proof dir");

        let cases: &[(&str, &str)] = &[
            // 1) single write then read back the SAME element
            (
                "single.wasm",
                "fn frame(t: i32) { let mut a = [0, 0, 0, 0]; a[2] = 42; host::display::clear(a[2]); host::display::present(); }",
            ),
            // 2) loop-fill a[i] = i*10, read a fixed cell (a[3] => 30)
            (
                "loopfill.wasm",
                "fn frame(t: i32) { let mut a = [0, 0, 0, 0, 0]; for i in 0..5 { a[i] = i * 10; } host::display::clear(a[3]); host::display::present(); }",
            ),
            // 2b) same loop-fill, read a[t] so node can pick the cell at runtime
            (
                "loopfill_t.wasm",
                "fn frame(t: i32) { let mut a = [0, 0, 0, 0, 0]; for i in 0..5 { a[i] = i * 10; } host::display::clear(a[t]); host::display::present(); }",
            ),
            // 3) overwrite the same cell twice — later write wins
            (
                "overwrite.wasm",
                "fn frame(t: i32) { let mut a = [0, 0]; a[0] = 7; a[0] = 99; host::display::clear(a[0]); host::display::present(); }",
            ),
            // 4) ARRAY PARAM — read through it in a helper. sum([3,4,5]) = 12.
            //    Proves an array typed `[i32; N]` lowers to its base pointer and
            //    the callee indexes it correctly.
            (
                "param_read.wasm",
                "fn sum(a: [i32; 3]) -> i32 { a[0] + a[1] + a[2] } \
                 fn frame(t: i32) { let g = [3, 4, 5]; host::display::clear(sum(g)); host::display::present(); }",
            ),
            // 5) ARRAY PARAM — SHARED BACKING. A write IN THE CALLEE through the
            //    array param is visible to the CALLER (the pointer aliases the
            //    same static region, C-style). set(g, 77); read g[1] => 77.
            (
                "param_shared_write.wasm",
                "fn set1(a: [i32; 3], v: i32) { a[1] = v; } \
                 fn frame(t: i32) { let mut g = [0, 0, 0]; set1(g, 77); host::display::clear(g[1]); host::display::present(); }",
            ),
            // 6) `[v; N]` SIZED REPEAT INIT — every slot is filled with v.
            //    let g = [9; 16]; read g[7] => 9 (a slot the literal didn't
            //    special-case), proving the fill loop covers the whole region.
            (
                "repeat_fill.wasm",
                "fn frame(t: i32) { let g = [9; 16]; host::display::clear(g[7]); host::display::present(); }",
            ),
            // 7) `[v; N]` then WRITE one cell — the rest stay at the fill value.
            //    let mut g = [5; 8]; g[2] = 88; read g[t] (t picks the cell).
            (
                "repeat_then_write.wasm",
                "fn frame(t: i32) { let mut g = [5; 8]; g[2] = 88; host::display::clear(g[t]); host::display::present(); }",
            ),
        ];

        for (file, src) in cases {
            let wasm = compile(src).unwrap_or_else(|e| panic!("compile {file}: {e}"));
            std::fs::write(out_dir.join(file), &wasm).unwrap_or_else(|e| panic!("write {file}: {e}"));
        }
    }
}
