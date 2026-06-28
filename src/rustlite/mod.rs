/// Token types (keywords, operators, literals).
pub(crate) mod token;
/// Byte-level lexer with string escapes.
pub(crate) mod lexer;
/// Full AST (structs, enums, functions, match, etc.).
#[allow(dead_code)] // internal compiler IR; not every field is read in every build/target
pub(crate) mod ast;
/// Recursive-descent parser with precedence climbing.
pub(crate) mod parser;
/// Scope-based type resolution and mutability checking.
#[allow(dead_code)] // internal compiler pass; some helpers are target/test-only
pub(crate) mod typecheck;
/// Wasm binary emitter (sections, opcodes, LEB128).
#[allow(dead_code)] // internal wasm emitter; some helpers are target/test-only
pub(crate) mod codegen;
/// Wasm32-only cartridge instantiation via `WebAssembly`.
#[allow(dead_code)] // wasm32-only cartridge runtime; methods unused in the native lib build
pub(crate) mod loader;

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

/// 1-based `(line, column)` of a byte offset in `source`.
///
/// The column counts CHARACTERS from the start of the line (so a caret row
/// of single-width spaces lines up). Offsets past the end of `source` clamp
/// to its last position; an offset inside a multi-byte char floors to that
/// char's start. Pure + native-testable — this is what turns the raw
/// `[start..end]` byte span every [`CompileError`] carries into something a
/// human (or an agent reading a tool result) can act on.
pub fn line_col(source: &str, offset: usize) -> (usize, usize) {
    let offset = offset.min(source.len());
    let mut line = 1usize;
    let mut col = 1usize;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Render a `line N, col M` + offending-source-line + caret-marker snippet
/// for `span`, e.g.:
///
/// ```text
/// line 2, col 11
///   let x = true + 1;
///           ^^^^^^^^
/// ```
///
/// The caret row underlines the span where it intersects its FIRST line
/// (multi-line spans clamp to that line; a zero-width or line-end span still
/// gets one `^` so there is always a visible marker). Tabs in the shown line
/// are widened to a single space so the caret row stays aligned. Returns
/// `None` only when `source` is empty (nothing to point into).
/// Largest char boundary `<= i`, clamped to `s.len()`. A span byte-offset can land
/// INSIDE a multi-byte char (e.g. an em-dash in a string literal), and slicing
/// there panics; `str::floor_char_boundary` is still unstable, so roll the
/// two-liner. (Without this, one non-ASCII source byte turned a clean CompileError
/// into a compiler PANIC — caught by the cartridge corpus.)
fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub fn render_snippet(source: &str, span: Span) -> Option<String> {
    if source.is_empty() {
        return None;
    }
    let start = floor_char_boundary(source, span.start);
    let (line, col) = line_col(source, start);
    // The full text of the line containing `start`.
    let line_start = source[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = source[line_start..]
        .find('\n')
        .map(|i| line_start + i)
        .unwrap_or(source.len());
    let line_text: String = source[line_start..line_end]
        .chars()
        .map(|c| if c == '\t' { ' ' } else { c })
        .collect();
    // Caret coverage: the span's char-width on this line, at least 1.
    let span_end = floor_char_boundary(source, span.end.clamp(start, line_end.max(start)));
    let width = source[start..span_end].chars().count().max(1);
    let line_chars = line_text.chars().count();
    let pad = (col - 1).min(line_chars);
    let carets = width.min((line_chars + 1).saturating_sub(pad)).max(1);
    Some(format!(
        "line {line}, col {col}\n  {line_text}\n  {}{}",
        " ".repeat(pad),
        "^".repeat(carets)
    ))
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

    /// `"line N, col M"` of this error's span in `source`, when it has one.
    /// The human-readable counterpart of the raw `[start..end]` byte span.
    pub fn location(&self, source: &str) -> Option<String> {
        let span = self.span?;
        let (line, col) = line_col(source, span.start.min(source.len()));
        Some(format!("line {line}, col {col}"))
    }

    /// The full agent/user-facing rendering: the `Display` form (LHxxxx code +
    /// message + byte span) plus, when the error carries a span, a
    /// line/column locator with the offending source line and a caret marker
    /// underneath. Every surface that has the source at hand (tool results,
    /// the CLI compile-check, the studio publish flow) should prefer this
    /// over bare `to_string()` — a byte offset alone makes the agent hunt.
    pub fn render(&self, source: &str) -> String {
        match self.span.and_then(|s| render_snippet(source, s)) {
            Some(snippet) => format!("{self}\n{snippet}"),
            None => self.to_string(),
        }
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
    fn line_col_is_one_based_and_clamped() {
        let src = "ab\ncde\nf";
        assert_eq!(super::line_col(src, 0), (1, 1));
        assert_eq!(super::line_col(src, 1), (1, 2));
        assert_eq!(super::line_col(src, 3), (2, 1)); // first char after \n
        assert_eq!(super::line_col(src, 5), (2, 3));
        assert_eq!(super::line_col(src, 7), (3, 1));
        // past-the-end clamps to the final position instead of panicking
        assert_eq!(super::line_col(src, 999), (3, 2));
        assert_eq!(super::line_col("", 0), (1, 1));
    }

    #[test]
    fn render_snippet_places_the_caret_under_the_span() {
        let src = "fn frame(t: i32) {\n  let x = true + 1;\n}";
        // span covering `true + 1` on line 2 (col 11)
        let start = src.find("true").unwrap();
        let snip = super::render_snippet(src, super::Span { start, end: start + 8 }).unwrap();
        let lines: Vec<&str> = snip.lines().collect();
        assert_eq!(lines[0], "line 2, col 11", "{snip}");
        // the shown line keeps its own leading whitespace under a 2-space indent
        assert_eq!(lines[1], "    let x = true + 1;", "{snip}");
        assert_eq!(lines[2], format!("  {}{}", " ".repeat(10), "^".repeat(8)), "{snip}");
    }

    #[test]
    fn render_snippet_edge_cases_never_panic() {
        // zero-width span still draws one caret
        let snip = super::render_snippet("let x;", super::Span { start: 4, end: 4 }).unwrap();
        assert!(snip.ends_with("^"), "{snip}");
        // a multi-line span clamps its carets to the FIRST line
        let src = "a\nbb\ncc";
        let snip = super::render_snippet(src, super::Span { start: 2, end: 7 }).unwrap();
        assert!(snip.contains("line 2, col 1"), "{snip}");
        assert_eq!(snip.lines().last().unwrap().matches('^').count(), 2, "{snip}");
        // span at EOF (the unterminated-string / unexpected-EOF shape)
        let src = "fn f() {";
        let snip = super::render_snippet(src, super::Span { start: 8, end: 8 }).unwrap();
        assert!(snip.contains("line 1, col 9"), "{snip}");
        // out-of-range span clamps
        assert!(super::render_snippet("x", super::Span { start: 50, end: 60 }).is_some());
        // empty source yields no snippet (nothing to point into)
        assert!(super::render_snippet("", super::Span { start: 0, end: 1 }).is_none());
        // tabs are widened to spaces so the caret row aligns
        let snip = super::render_snippet("\tlet q = ;", super::Span { start: 9, end: 10 }).unwrap();
        assert!(!snip.contains('\t'), "{snip}");
        // a span offset landing INSIDE a multi-byte char (an em-dash in a string
        // literal) must clamp to a char boundary, not panic the slicer.
        let src = "let s = \"a \u{2014} b\";"; // \u{2014} (em-dash) is 3 bytes
        let dash = src.find('\u{2014}').unwrap();
        assert!(super::render_snippet(src, super::Span { start: dash + 1, end: dash + 2 }).is_some());
    }

    #[test]
    fn compile_error_render_carries_code_location_and_caret() {
        // Full pipeline: a type mismatch on line 2 renders the LH code, the
        // line/col locator, the offending source line, and a caret row.
        let src = "fn frame(t: i32) {\n  let x = true + 1;\n  host::display::present();\n}";
        let err = compile(src).expect_err("type mismatch must fail");
        let rendered = err.render(src);
        assert!(rendered.starts_with("LH0204:"), "{rendered}");
        assert!(rendered.contains("line 2, col"), "{rendered}");
        assert!(rendered.contains("let x = true + 1;"), "{rendered}");
        assert!(rendered.lines().last().unwrap().trim_start().starts_with('^'), "{rendered}");
        // The exact column depends on which subexpression the checker pins;
        // what matters is that the locator names LINE 2 (where the bug is).
        assert!(err.location(src).expect("typed errors carry a span").starts_with("line 2, col "));
        // A span-less error renders as its plain Display form.
        let plain = super::CompileError::new("internal");
        assert_eq!(plain.render(src), "internal");
        assert_eq!(plain.location(src), None);
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

    #[test]
    fn else_less_if_in_value_position_is_rejected() {
        // GitHub #80 (1): an `if` WITHOUT an `else` is a statement, never a value
        // (Rust semantics). Using its value (here as a `let` init) must be a type
        // error — the previous typechecker typed the `if` as the then-block's
        // type even with no else, and codegen then emitted an `(if (result T))`
        // frame with no else branch (stack-imbalanced, invalid wasm).
        let e = compile(
            "fn frame(t: i32) { let x = if t > 0 { 5 }; host::display::clear(x); host::display::present(); }"
        )
        .expect_err("else-less if used as a value must be rejected");
        assert_eq!(e.code, Some(codes::TYPE_MISMATCH), "{e}");
        // An else-less `if` as a STATEMENT (its value discarded) is still fine.
        assert!(compile(
            "fn frame(t: i32) { let mut x = 0; if t > 0 { x = 5; } host::display::clear(x); host::display::present(); }"
        )
        .is_ok());
        // With an `else`, using the value is fine (both branches yield i32).
        assert!(compile(
            "fn frame(t: i32) { let x = if t > 0 { 5 } else { 9 }; host::display::clear(x); host::display::present(); }"
        )
        .is_ok());
    }

    #[test]
    fn short_circuit_with_break_in_rhs_compiles() {
        // GitHub #80 (2): the short-circuit `&&`/`||` arm opens an OP_IF frame for
        // the rhs but never bumped `extra_depth`, so a `break`/`continue` in the
        // rhs (e.g. `cond || continue`) branched to the wrong frame (invalid
        // wasm). These compile + must validate (see the node proof). `&&` with a
        // `break` in the rhs — the break runs only when the lhs is true:
        assert!(compile(
            "fn frame(t: i32) { let mut i = 0; loop { let go = t > 0 && break; i = i + 1; } host::display::clear(i); host::display::present(); }"
        )
        .is_ok());
        // `||` with a `continue` in the rhs — the continue runs only when the lhs
        // is false:
        assert!(compile(
            "fn frame(t: i32) { let mut i = 0; while i < 3 { i = i + 1; let keep = i > 9 || continue; } host::display::clear(i); host::display::present(); }"
        )
        .is_ok());
    }

    #[test]
    fn non_last_irrefutable_match_arm_is_rejected() {
        // GitHub #80 (3): a `_`/binding arm matches everything; codegen lowers the
        // terminal catch-all to a plain `else`, so a non-last irrefutable arm
        // emitted stack-imbalanced wasm. Reject it (move it last).
        let e = compile(
            "fn frame(t: i32) { let v = match t { _ => 1, 0 => 2 }; host::display::clear(v); host::display::present(); }"
        )
        .expect_err("a non-last wildcard arm must be rejected");
        assert_eq!(e.code, Some(codes::TYPE_MISMATCH), "{e}");
        // A non-last BINDING arm is equally irrefutable → rejected.
        let e = compile(
            "fn frame(t: i32) { let v = match t { n => n, 0 => 2 }; host::display::clear(v); host::display::present(); }"
        )
        .expect_err("a non-last binding arm must be rejected");
        assert_eq!(e.code, Some(codes::TYPE_MISMATCH), "{e}");
        // The catch-all LAST is the correct shape and still compiles.
        assert!(compile(
            "fn frame(t: i32) { let v = match t { 0 => 2, _ => 1 }; host::display::clear(v); host::display::present(); }"
        )
        .is_ok());
    }

    #[test]
    fn match_binding_arm_binds_the_scrutinee() {
        // A binding arm (`n => n`) binds the whole scrutinee to `n`. Before the
        // fix codegen never mapped the name to the scrutinee local, so this
        // failed with "undefined local 'n'" (LH0201) — accepted source the
        // compiler then refused to lower. The LAST-arm binding is the supported
        // shape (a non-last one is rejected, see the test above).
        assert!(compile(
            "fn frame(t: i32) { let v = match t { 0 => 100, n => n }; host::display::clear(v); host::display::present(); }"
        )
        .is_ok());
        // A binding that SHADOWS an outer local still lowers (and now reads the
        // scrutinee, not the outer 7 — value correctness verified via wasm exec).
        assert!(compile(
            "fn frame(t: i32) { let x = 7; let v = match t { 0 => 1, x => x }; host::display::clear(v); host::display::present(); }"
        )
        .is_ok());
    }

    #[test]
    fn struct_literals_are_rejected_not_miscompiled() {
        // The struct codegen path was a stub that pushed every field value and
        // aggregated none → INVALID (stack-imbalanced) wasm that failed
        // instantiation. Until structs are materialised in memory, reject them
        // with a clear diagnostic rather than emit a broken cartridge.
        let e = compile(
            "struct P { x: i32, y: i32 } fn frame(t: i32) { let p = P { x: 11, y: 22 }; host::display::clear(p.x); host::display::present(); }"
        )
        .expect_err("struct literals must be rejected, not miscompiled");
        assert_eq!(e.code, Some(codes::UNSUPPORTED_FEATURE), "{e}");
    }

    #[test]
    fn array_return_type_is_rejected() {
        // RETURNING an array is unsound under the static-region model: the region
        // a returned array points into is reused on every call, so two live
        // results of one array-returning fn would alias and the second call
        // silently clobbers the first (proven via node:
        //   fn mk(v:i32)->[i32;3]{[v,v,v]}  let a=mk(1); let b=mk(2);
        // made `a[0]` read back as 2, not 1). The compiler must reject it rather
        // than emit corrupting code — the supported pattern is a mutable array
        // PARAM the callee fills in place (C-style shared backing).
        let e = compile("fn mk(v: i32) -> [i32; 3] { [v, v, v] } fn frame(t: i32) { let a = mk(1); host::display::clear(a[0]); host::display::present(); }")
            .expect_err("array return must be rejected");
        assert_eq!(e.code, Some(codes::UNSUPPORTED_FEATURE), "{e}");
        // Forward reference (fn declared after frame) is rejected too — the guard
        // lives in the signature-resolution pass, which runs before any body.
        assert!(compile(
            "fn frame(t: i32) { host::display::present(); } fn mk() -> [i32; 2] { [1, 2] }"
        )
        .is_err());
        // An array PARAM with an i32 return is still fine (the real pattern).
        assert!(compile(
            "fn fill(a: [i32; 3], v: i32) -> i32 { a[0] = v; a[0] } \
             fn frame(t: i32) { let mut g = [0, 0, 0]; host::display::clear(fill(g, 9)); host::display::present(); }"
        )
        .is_ok());
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

/// Emit the GitHub-#80 codegen-fix cartridges for the node validate-proof
/// (`scripts/verify-codegen-valid.mjs`). Each snippet is ACCEPTED source that
/// the three #80 bugs used to lower to STACK-IMBALANCED wasm — a `WebAssembly.
/// validate` over the emitted bytes is the only honest proof the fix holds (the
/// in-process `compile().is_ok()` tests only prove the bytes were produced, not
/// that they're a valid module). Run `cargo test emits_codegen_valid_proof`,
/// then `node scripts/verify-codegen-valid.mjs`. Native-only (writes the tree).
#[cfg(all(test, feature = "native"))]
mod codegen_valid_run_proof {
    use super::compile;

    #[test]
    fn emits_codegen_valid_proof() {
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join(".codegen-valid-proof");
        std::fs::create_dir_all(&out_dir).expect("create proof dir");

        let cases: &[(&str, &str)] = &[
            // #80 (1): an else-less `if` as a STATEMENT (value discarded) stays on
            // the void path. Used to be fine, but pin it so the void-frame lowering
            // keeps validating after the value-position reject.
            (
                "elseless_if_stmt.wasm",
                "fn frame(t: i32) { let mut x = 0; if t > 0 { x = 5; } host::display::clear(x); host::display::present(); }",
            ),
            // #80 (1): an `if`/`else` USED AS A VALUE emits an `(if (result i32))`
            // frame with both branches — must validate (balanced stack).
            (
                "value_if_else.wasm",
                "fn frame(t: i32) { let x = if t > 0 { 5 } else { 9 }; host::display::clear(x); host::display::present(); }",
            ),
            // #80 (2): short-circuit `&&` with a `break` in the rhs — the rhs runs
            // inside the `&&` if-frame, so its br target must step one frame extra.
            (
                "and_break_rhs.wasm",
                "fn frame(t: i32) { let mut i = 0; loop { let go = t > 0 && break; i = i + 1; } host::display::clear(i); host::display::present(); }",
            ),
            // #80 (2): short-circuit `||` with a `continue` in the rhs.
            (
                "or_continue_rhs.wasm",
                "fn frame(t: i32) { let mut i = 0; while i < 3 { i = i + 1; let keep = i > 9 || continue; } host::display::clear(i); host::display::present(); }",
            ),
            // #80 (3): a `match` with the catch-all LAST (the only legal shape) —
            // the chained if/else closes cleanly. A non-last catch-all is rejected
            // in the typechecker, so only the valid shape reaches codegen.
            (
                "match_wildcard_last.wasm",
                "fn frame(t: i32) { let v = match t { 0 => 2, 1 => 7, _ => 1 }; host::display::clear(v); host::display::present(); }",
            ),
        ];

        for (file, src) in cases {
            let wasm = compile(src).unwrap_or_else(|e| panic!("compile {file}: {e}"));
            std::fs::write(out_dir.join(file), &wasm).unwrap_or_else(|e| panic!("write {file}: {e}"));
        }
    }
}
