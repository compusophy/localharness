//! Token types for the bashlite lexer.

/// A single lexed token. Bashlite is line/word oriented (like a shell), so the
/// token set is small: words (which may carry interpolation), operators, and
/// the structural newline/semicolon that separate commands.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// A bare word or quoted string, already split into literal/interpolation
    /// segments (see [`WordPart`]). Adjacent quoted/unquoted runs with no
    /// whitespace lex as ONE word (`a"b"c` → one `Word`), matching the shell.
    Word(Vec<WordPart>),
    /// `|` — pipe stdout of the left command into stdin of the right.
    Pipe,
    /// `;` or a newline — command separator.
    Semi,
    /// `(` — paren grouping (only used by the parser for `[ ... ]`-free test
    /// grouping is NOT supported in v1; kept for clearer diagnostics).
    LParen,
    /// `)`
    RParen,
    /// End of input.
    Eof,
}

/// One segment of a word. A word is a sequence of these concatenated at eval
/// time: `"$x.rl"` → `[Var("x"), Lit(".rl")]`.
#[derive(Debug, Clone, PartialEq)]
pub enum WordPart {
    /// A literal run of characters.
    Lit(String),
    /// `$name` or `${name}` — expands to the variable's value ("" if unset).
    Var(String),
    /// `$(...)` — command substitution; the inner source is re-lexed/parsed and
    /// run, and its trailing-newline-trimmed stdout replaces the segment.
    Subst(String),
}
