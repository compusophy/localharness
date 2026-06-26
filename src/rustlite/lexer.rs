use crate::error_codes as codes;
use crate::rustlite::{CompileError, Span};
use crate::rustlite::token::{Token, TokenKind};

pub fn lex(source: &str) -> Result<Vec<Token>, CompileError> {
    let mut lexer = Lexer::new(source);
    let mut tokens = Vec::new();
    loop {
        let tok = lexer.next_token()?;
        let is_eof = tok.kind == TokenKind::Eof;
        tokens.push(tok);
        if is_eof { break; }
    }
    Ok(tokens)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str) -> Self {
        Self { src: source.as_bytes(), pos: 0 }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.src.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> u8 {
        let b = self.src[self.pos];
        self.pos += 1;
        b
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // whitespace
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            // line comment
            if self.pos + 1 < self.src.len() && self.src[self.pos] == b'/' && self.src[self.pos + 1] == b'/' {
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            // block comment `/* … */` — Rust allows nesting, so track depth.
            // (Without this the leading `/` lexed as `Slash` → "got Slash".)
            if self.pos + 1 < self.src.len() && self.src[self.pos] == b'/' && self.src[self.pos + 1] == b'*' {
                let mut depth = 1usize;
                self.pos += 2;
                while self.pos < self.src.len() && depth > 0 {
                    if self.pos + 1 < self.src.len() && self.src[self.pos] == b'/' && self.src[self.pos + 1] == b'*' {
                        depth += 1;
                        self.pos += 2;
                    } else if self.pos + 1 < self.src.len() && self.src[self.pos] == b'*' && self.src[self.pos + 1] == b'/' {
                        depth -= 1;
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                continue;
            }
            // attribute: `#[...]` (outer) or `#![...]` (inner) — accepted and
            // ignored, treated as trivia like a comment, so standard Rust
            // attributes (`#[no_mangle]`, `#[derive(Clone)]`, …) that agent-
            // authored source emits don't trip the lexer on the `#` byte.
            if self.pos < self.src.len() && self.src[self.pos] == b'#' {
                let mut i = self.pos + 1;
                if i < self.src.len() && self.src[i] == b'!' {
                    i += 1;
                }
                if i < self.src.len() && self.src[i] == b'[' {
                    // Skip to the matching `]`, tracking `[` nesting depth.
                    let mut depth = 0usize;
                    self.pos = i;
                    while self.pos < self.src.len() {
                        match self.src[self.pos] {
                            b'[' => depth += 1,
                            b']' => {
                                depth -= 1;
                                self.pos += 1;
                                if depth == 0 {
                                    break;
                                }
                                continue;
                            }
                            _ => {}
                        }
                        self.pos += 1;
                    }
                    continue;
                }
            }
            break;
        }
    }

    fn next_token(&mut self) -> Result<Token, CompileError> {
        self.skip_whitespace_and_comments();

        if self.pos >= self.src.len() {
            return Ok(Token { kind: TokenKind::Eof, span: Span { start: self.pos, end: self.pos } });
        }

        let start = self.pos;
        let b = self.advance();

        let kind = match b {
            b'(' => TokenKind::LParen,
            b')' => TokenKind::RParen,
            b'{' => TokenKind::LBrace,
            b'}' => TokenKind::RBrace,
            b'[' => TokenKind::LBracket,
            b']' => TokenKind::RBracket,
            b',' => TokenKind::Comma,
            b';' => TokenKind::Semi,
            b'.' => {
                if self.peek() == Some(b'.') {
                    self.advance();
                    if self.peek() == Some(b'=') {
                        self.advance();
                        TokenKind::DotDotEq
                    } else {
                        TokenKind::DotDot
                    }
                } else {
                    TokenKind::Dot
                }
            }
            b'+' => self.maybe_eq(TokenKind::PlusEq, TokenKind::Plus),
            b'*' => self.maybe_eq(TokenKind::StarEq, TokenKind::Star),
            b'/' => self.maybe_eq(TokenKind::SlashEq, TokenKind::Slash),
            b'%' => self.maybe_eq(TokenKind::PercentEq, TokenKind::Percent),

            b':' => {
                if self.peek() == Some(b':') {
                    self.advance();
                    TokenKind::ColonColon
                } else {
                    TokenKind::Colon
                }
            }

            b'-' => {
                if self.peek() == Some(b'>') {
                    self.advance();
                    TokenKind::Arrow
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }

            b'=' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::EqEq
                } else if self.peek() == Some(b'>') {
                    self.advance();
                    TokenKind::FatArrow
                } else {
                    TokenKind::Eq
                }
            }

            b'!' => {
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }

            b'<' => {
                if self.peek() == Some(b'<') {
                    self.advance();
                    TokenKind::Shl
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }

            b'>' => {
                if self.peek() == Some(b'>') {
                    self.advance();
                    TokenKind::Shr
                } else if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::GtEq
                } else {
                    TokenKind::Gt
                }
            }

            b'&' => {
                if self.peek() == Some(b'&') {
                    self.advance();
                    TokenKind::AmpAmp
                } else {
                    TokenKind::Amp
                }
            }

            b'|' => {
                if self.peek() == Some(b'|') {
                    self.advance();
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }

            b'^' => TokenKind::Caret,

            b'"' => self.lex_string(start)?,

            b'0'..=b'9' => self.lex_number(start)?,

            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident(start)?,

            b'\'' => self.lex_char(start)?,

            _ => return Err(CompileError::at_code(codes::UNEXPECTED_BYTE, format!("unexpected byte 0x{b:02x}"), Span { start, end: self.pos })),
        };

        Ok(Token { kind, span: Span { start, end: self.pos } })
    }

    fn lex_string(&mut self, start: usize) -> Result<TokenKind, CompileError> {
        let mut s = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => {
                    return Err(CompileError::at_code(codes::UNTERMINATED_STRING, "unterminated string", Span { start, end: self.pos }));
                }
                Some(b'"') => {
                    self.advance();
                    break;
                }
                Some(b'\\') => {
                    self.advance();
                    match self.peek() {
                        Some(b'n') => { self.advance(); s.push('\n'); }
                        Some(b't') => { self.advance(); s.push('\t'); }
                        Some(b'\\') => { self.advance(); s.push('\\'); }
                        Some(b'"') => { self.advance(); s.push('"'); }
                        Some(b'0') => { self.advance(); s.push('\0'); }
                        Some(c) => {
                            return Err(CompileError::at_code(
                                codes::UNKNOWN_ESCAPE,
                                format!("unknown escape \\{}", c as char),
                                Span { start: self.pos - 1, end: self.pos + 1 },
                            ));
                        }
                        None => {
                            return Err(CompileError::at_code(codes::UNTERMINATED_STRING, "unterminated escape", Span { start, end: self.pos }));
                        }
                    }
                }
                Some(c) if c >= 0x80 => {
                    // A raw byte >= 0x80 is part of a multi-byte UTF-8 sequence;
                    // pushing it as `c as char` would split it into a Latin-1
                    // codepoint and silently garble the literal (intern_string
                    // re-encodes it wrong). rustlite string literals are ASCII
                    // only — reject the byte cleanly rather than corrupt it.
                    return Err(CompileError::at_code(
                        codes::UNEXPECTED_BYTE,
                        format!("non-ASCII byte 0x{c:02x} in string literal (ASCII only)"),
                        Span { start: self.pos, end: self.pos + 1 },
                    ));
                }
                Some(c) => {
                    self.advance();
                    s.push(c as char);
                }
            }
        }
        Ok(TokenKind::StringLit(s))
    }

    /// A char literal `'A'` → its byte value as an `IntLit` (chars are `i32`
    /// glyph codes in rustlite, e.g. `host::display::draw_char(x, y, 'A', …)`).
    /// Same escapes as strings; the opening quote is already consumed. Empty
    /// (`''`) and multi-byte literals are clear errors.
    fn lex_char(&mut self, start: usize) -> Result<TokenKind, CompileError> {
        let byte: u8 = match self.peek() {
            Some(b'\\') => {
                self.advance();
                let e = match self.peek() {
                    Some(b'n') => b'\n',
                    Some(b't') => b'\t',
                    Some(b'\\') => b'\\',
                    Some(b'\'') => b'\'',
                    Some(b'0') => 0,
                    Some(c) => {
                        return Err(CompileError::at_code(
                            codes::UNKNOWN_ESCAPE,
                            format!("unknown escape \\{}", c as char),
                            Span { start, end: self.pos + 1 },
                        ));
                    }
                    None => {
                        return Err(CompileError::at_code(
                            codes::BAD_CHAR_LITERAL,
                            "unterminated char literal",
                            Span { start, end: self.pos },
                        ));
                    }
                };
                self.advance();
                e
            }
            Some(b'\'') => {
                return Err(CompileError::at_code(
                    codes::BAD_CHAR_LITERAL,
                    "empty char literal",
                    Span { start, end: self.pos },
                ));
            }
            Some(c) => {
                self.advance();
                c
            }
            None => {
                return Err(CompileError::at_code(
                    codes::BAD_CHAR_LITERAL,
                    "unterminated char literal",
                    Span { start, end: self.pos },
                ));
            }
        };
        if self.peek() != Some(b'\'') {
            return Err(CompileError::at_code(
                codes::BAD_CHAR_LITERAL,
                "char literal must be a single byte (use a \"string\" for text)",
                Span { start, end: self.pos },
            ));
        }
        self.advance(); // closing quote
        Ok(TokenKind::IntLit(byte as i64))
    }

    /// An operator (`+ * / %`) followed by `=` → the compound-assign token,
    /// else the plain operator.
    fn maybe_eq(&mut self, if_eq: TokenKind, otherwise: TokenKind) -> TokenKind {
        if self.peek() == Some(b'=') {
            self.advance();
            if_eq
        } else {
            otherwise
        }
    }

    fn lex_number(&mut self, _start: usize) -> Result<TokenKind, CompileError> {
        // We already consumed the first digit.
        // Hex literal: a leading `0` followed by `x`/`X` (e.g. colours like
        // `0xFF0000`). Consume hex digits (underscores allowed) + an optional
        // i32/i64 suffix and parse base-16 — via u64 so a full 32-bit value
        // fits, then cast to i64. Without this branch the `0` lexes alone and
        // `xFF0000` becomes an Ident → "expected Semi, got Ident" (feedback #15/#16).
        if self.src.get(_start) == Some(&b'0') && matches!(self.peek(), Some(b'x') | Some(b'X')) {
            self.advance(); // 'x'
            let digits_start = self.pos;
            while self.peek().is_some_and(|c| c.is_ascii_hexdigit() || c == b'_') {
                self.advance();
            }
            if self.pos == digits_start {
                return Err(CompileError::at_code(
                    codes::BAD_NUMBER,
                    "hex literal `0x` has no digits".to_string(),
                    Span { start: _start, end: self.pos },
                ));
            }
            let digits_end = self.pos;
            if self.peek() == Some(b'i') {
                let suffix_start = self.pos;
                self.advance();
                if self.peek() == Some(b'3') { self.advance(); if self.peek() == Some(b'2') { self.advance(); } else { self.pos = suffix_start; } }
                else if self.peek() == Some(b'6') { self.advance(); if self.peek() == Some(b'4') { self.advance(); } else { self.pos = suffix_start; } }
                else { self.pos = suffix_start; }
            }
            let raw = std::str::from_utf8(&self.src[digits_start..digits_end])
                .unwrap()
                .replace('_', "");
            let val = u64::from_str_radix(&raw, 16).map_err(|e| {
                CompileError::at_code(codes::BAD_NUMBER, format!("bad hex int: {e}"), Span { start: _start, end: self.pos })
            })? as i64;
            return Ok(TokenKind::IntLit(val));
        }
        // `_` digit separators are valid in decimal too (the hex branch above
        // already allows them) — `1_000` / `16_777_215` are valid Rust and a
        // natural way to write the large color/coordinate constants cartridges
        // use. They're stripped before `parse` below.
        while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'_') {
            self.advance();
        }

        let is_float = self.peek() == Some(b'.') && self.peek2().is_some_and(|c| c.is_ascii_digit());
        if is_float {
            self.advance(); // '.'
            while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'_') {
                self.advance();
            }
            // optional f32/f64 suffix
            if self.peek() == Some(b'f') {
                let suffix_start = self.pos;
                self.advance();
                if self.peek() == Some(b'3') { self.advance(); if self.peek() == Some(b'2') { self.advance(); } }
                else if self.peek() == Some(b'6') { self.advance(); if self.peek() == Some(b'4') { self.advance(); } }
                else { self.pos = suffix_start; }
            }
            let text = std::str::from_utf8(&self.src[_start..self.pos]).unwrap();
            let text = text.trim_end_matches("f32").trim_end_matches("f64").replace('_', "");
            let val: f64 = text.parse().map_err(|e| CompileError::at_code(codes::BAD_NUMBER, format!("bad float: {e}"), Span { start: _start, end: self.pos }))?;
            Ok(TokenKind::FloatLit(val))
        } else {
            // optional i32/i64 suffix
            if self.peek() == Some(b'i') {
                let suffix_start = self.pos;
                self.advance();
                if self.peek() == Some(b'3') { self.advance(); if self.peek() == Some(b'2') { self.advance(); } else { self.pos = suffix_start; } }
                else if self.peek() == Some(b'6') { self.advance(); if self.peek() == Some(b'4') { self.advance(); } else { self.pos = suffix_start; } }
                else { self.pos = suffix_start; }
            }
            let text = std::str::from_utf8(&self.src[_start..self.pos]).unwrap();
            let text = text.trim_end_matches("i32").trim_end_matches("i64").replace('_', "");
            let val: i64 = text.parse().map_err(|e| CompileError::at_code(codes::BAD_NUMBER, format!("bad int: {e}"), Span { start: _start, end: self.pos }))?;
            Ok(TokenKind::IntLit(val))
        }
    }

    fn lex_ident(&mut self, _start: usize) -> Result<TokenKind, CompileError> {
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.advance();
        }
        let text = std::str::from_utf8(&self.src[_start..self.pos]).unwrap();
        Ok(TokenKind::keyword(text).unwrap_or_else(|| TokenKind::Ident(text.to_string())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lex_simple_fn() {
        let tokens = lex("fn main() { 42 }").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Fn);
        assert_eq!(tokens[1].kind, TokenKind::Ident("main".into()));
        assert_eq!(tokens[2].kind, TokenKind::LParen);
        assert_eq!(tokens[3].kind, TokenKind::RParen);
        assert_eq!(tokens[4].kind, TokenKind::LBrace);
        assert_eq!(tokens[5].kind, TokenKind::IntLit(42));
        assert_eq!(tokens[6].kind, TokenKind::RBrace);
        assert_eq!(tokens[7].kind, TokenKind::Eof);
    }

    #[test]
    fn lex_string_escapes() {
        let tokens = lex(r#""hello\nworld""#).unwrap();
        assert_eq!(tokens[0].kind, TokenKind::StringLit("hello\nworld".into()));
    }

    #[test]
    fn lex_string_rejects_non_ascii() {
        // L39: a raw byte >= 0x80 (here the UTF-8 for `é`) was pushed as a
        // Latin-1 char, silently garbling the literal. It is now a clean error.
        let err = lex("\"caf\u{00e9}\"").unwrap_err();
        assert_eq!(err.code, Some(codes::UNEXPECTED_BYTE));
    }

    #[test]
    fn lex_float() {
        let tokens = lex("2.75f32").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::FloatLit(2.75));
    }

    #[test]
    fn lex_hex_literals() {
        // The common case: 24-bit RGB colours.
        assert_eq!(lex("0xFF0000").unwrap()[0].kind, TokenKind::IntLit(0xFF_0000));
        // lowercase + underscore separators.
        assert_eq!(lex("0xff_00ff").unwrap()[0].kind, TokenKind::IntLit(0xFF_00FF));
        // an i32 suffix is consumed (rustlite ints are i64 internally).
        assert_eq!(lex("0x10i32").unwrap()[0].kind, TokenKind::IntLit(16));
        // `0x` with no digits is a clean error, not a silent `0` + `Ident("x…")`.
        assert!(lex("0x").is_err());
        // Regression guard: a bare `0` still lexes as decimal zero.
        assert_eq!(lex("0").unwrap()[0].kind, TokenKind::IntLit(0));
    }

    #[test]
    fn lex_decimal_digit_separators() {
        // `_` separators are valid Rust in DECIMAL too (the hex branch already
        // allowed them). Before the fix `1_000` lexed as `IntLit(1) Ident("_000")`
        // and parsing then failed on valid agent source.
        assert_eq!(lex("1_000").unwrap()[0].kind, TokenKind::IntLit(1000));
        assert_eq!(lex("16_777_215").unwrap()[0].kind, TokenKind::IntLit(16_777_215));
        // fractional part too, with a suffix.
        assert_eq!(lex("3_000.500_5f64").unwrap()[0].kind, TokenKind::FloatLit(3000.5005));
        // a trailing/loose `_` is stripped before parse (matches Rust: `1_` == 1).
        assert_eq!(lex("1_").unwrap()[0].kind, TokenKind::IntLit(1));
    }

    #[test]
    fn lex_char_literals() {
        // `'A'` → its byte value (a `draw_char` glyph code).
        assert_eq!(lex("'A'").unwrap()[0].kind, TokenKind::IntLit(65));
        assert_eq!(lex("' '").unwrap()[0].kind, TokenKind::IntLit(32));
        // escapes
        assert_eq!(lex(r"'\n'").unwrap()[0].kind, TokenKind::IntLit(10));
        assert_eq!(lex(r"'\\'").unwrap()[0].kind, TokenKind::IntLit(92));
        // clear errors, not a lexer crash
        assert!(lex("''").is_err()); // empty
        assert!(lex("'AB'").is_err()); // multi-byte
    }

    #[test]
    fn lex_block_comments() {
        // `/* … */` is skipped (nesting allowed); `/` still lexes as division.
        let toks: Vec<_> = lex("1 /* x */ + 2")
            .unwrap()
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Eof))
            .map(|t| t.kind)
            .collect();
        assert_eq!(toks, vec![TokenKind::IntLit(1), TokenKind::Plus, TokenKind::IntLit(2)]);
        assert_eq!(lex("/* /* nested */ */ 5").unwrap()[0].kind, TokenKind::IntLit(5));
        assert_eq!(lex("6 / 2").unwrap()[1].kind, TokenKind::Slash);
    }

    #[test]
    fn lex_operators() {
        let tokens = lex("== != <= >= && || => ->").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::EqEq);
        assert_eq!(tokens[1].kind, TokenKind::BangEq);
        assert_eq!(tokens[2].kind, TokenKind::LtEq);
        assert_eq!(tokens[3].kind, TokenKind::GtEq);
        assert_eq!(tokens[4].kind, TokenKind::AmpAmp);
        assert_eq!(tokens[5].kind, TokenKind::PipePipe);
        assert_eq!(tokens[6].kind, TokenKind::FatArrow);
        assert_eq!(tokens[7].kind, TokenKind::Arrow);
    }

    #[test]
    fn lex_keywords() {
        let tokens = lex("struct enum fn let mut if else match while loop break continue return const use").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Struct);
        assert_eq!(tokens[1].kind, TokenKind::Enum);
        assert_eq!(tokens[2].kind, TokenKind::Fn);
        assert_eq!(tokens[3].kind, TokenKind::Let);
        assert_eq!(tokens[4].kind, TokenKind::Mut);
    }

    #[test]
    fn lex_comment_skip() {
        let tokens = lex("fn // this is a comment\nmain").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Fn);
        assert_eq!(tokens[1].kind, TokenKind::Ident("main".into()));
    }

    #[test]
    fn lex_attribute_skip() {
        // `#[...]` / `#![...]` are skipped as trivia; `#[derive(...)]` with a
        // nested `(...)` group is fine, and a `[` inside the attr nests.
        let tokens = lex("#![inner]\n#[no_mangle]\n#[derive(Clone, Copy)]\nfn main").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Fn);
        assert_eq!(tokens[1].kind, TokenKind::Ident("main".into()));
    }

    #[test]
    fn lex_bare_hash_still_errors() {
        // A lone `#` not introducing an attribute is still an error — we
        // only treat `#[` / `#![` as trivia, nothing else.
        assert!(lex("fn # main").is_err());
    }
}
