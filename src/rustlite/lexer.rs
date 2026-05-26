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
            b'.' => TokenKind::Dot,
            b'+' => TokenKind::Plus,
            b'*' => TokenKind::Star,
            b'/' => TokenKind::Slash,
            b'%' => TokenKind::Percent,

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
                if self.peek() == Some(b'=') {
                    self.advance();
                    TokenKind::LtEq
                } else {
                    TokenKind::Lt
                }
            }

            b'>' => {
                if self.peek() == Some(b'=') {
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
                    return Err(CompileError::at("unexpected '&' (no references in rustlite)", Span { start, end: self.pos }));
                }
            }

            b'|' => {
                if self.peek() == Some(b'|') {
                    self.advance();
                    TokenKind::PipePipe
                } else {
                    return Err(CompileError::at("unexpected '|' (no closures in rustlite)", Span { start, end: self.pos }));
                }
            }

            b'"' => self.lex_string(start)?,

            b'0'..=b'9' => self.lex_number(start)?,

            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_ident(start)?,

            _ => return Err(CompileError::at(format!("unexpected byte 0x{b:02x}"), Span { start, end: self.pos })),
        };

        Ok(Token { kind, span: Span { start, end: self.pos } })
    }

    fn lex_string(&mut self, start: usize) -> Result<TokenKind, CompileError> {
        let mut s = String::new();
        loop {
            match self.peek() {
                None | Some(b'\n') => {
                    return Err(CompileError::at("unterminated string", Span { start, end: self.pos }));
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
                            return Err(CompileError::at(
                                format!("unknown escape \\{}", c as char),
                                Span { start: self.pos - 1, end: self.pos + 1 },
                            ));
                        }
                        None => {
                            return Err(CompileError::at("unterminated escape", Span { start, end: self.pos }));
                        }
                    }
                }
                Some(c) => {
                    self.advance();
                    s.push(c as char);
                }
            }
        }
        Ok(TokenKind::StringLit(s))
    }

    fn lex_number(&mut self, _start: usize) -> Result<TokenKind, CompileError> {
        // We already consumed the first digit
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }

        let is_float = self.peek() == Some(b'.') && self.peek2().is_some_and(|c| c.is_ascii_digit());
        if is_float {
            self.advance(); // '.'
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
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
            let text = text.trim_end_matches("f32").trim_end_matches("f64");
            let val: f64 = text.parse().map_err(|e| CompileError::at(format!("bad float: {e}"), Span { start: _start, end: self.pos }))?;
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
            let text = text.trim_end_matches("i32").trim_end_matches("i64");
            let val: i64 = text.parse().map_err(|e| CompileError::at(format!("bad int: {e}"), Span { start: _start, end: self.pos }))?;
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
    fn lex_float() {
        let tokens = lex("2.75f32").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::FloatLit(2.75));
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
}
