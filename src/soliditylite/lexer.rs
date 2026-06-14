//! SolidityLite lexer — source bytes → a flat [`Token`] stream.
//!
//! Mirrors [`crate::rustlite::lexer`]'s byte-cursor discipline (one struct, a
//! `pos` cursor, `//`/`/* */` comment skipping, spans on every token) but its
//! token set is the Solidity-subset surface: the `facet`/`function`/`external`/
//! `view`/`returns`/`return`/`mapping`/`uint256` keywords, the structural
//! punctuation (`{ } ( ) [ ] ; , = => . + > < >= <= ==`), identifiers,
//! decimal/hex integer literals, and double-quoted string literals (the
//! `require(cond, "msg")` message operand). Floats and char literals stay absent
//! from the grammar — an unexpected byte is a clean error.

use crate::rustlite::{CompileError, Span};
use crate::error_codes as codes;

/// A lexed token tagged with its kind and source span.
///
/// Only `PartialEq` (not `Eq`): it carries a [`Span`], and `rustlite::Span`
/// derives `PartialEq` only.
#[derive(Debug, Clone, PartialEq)]
pub struct SolTok {
    /// What kind of token this is.
    pub kind: SolKind,
    /// The token's byte span in the source.
    pub span: Span,
}

/// The SolidityLite token kinds (the floor-grammar + storage-stretch surface).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SolKind {
    /// `facet` (or its `contract` alias) — the top-level container keyword.
    Facet,
    /// `function`.
    Function,
    /// `external`.
    External,
    /// `view`.
    View,
    /// `pure`.
    Pure,
    /// `returns` (the declaration keyword, in `returns (uint256)`).
    Returns,
    /// `return` (the statement keyword).
    Return,
    /// `mapping` (the `mapping(K => V)` state-var keyword).
    Mapping,
    /// A type keyword (`uint256`/`address`/`bool`/`bytes32`).
    TypeName(String),
    /// An identifier (function/facet/variable name).
    Ident(String),
    /// An integer literal, already normalized to a big-endian 32-byte word.
    Int([u8; 32]),
    /// A double-quoted string literal (its decoded contents, without the quotes).
    /// Only ever a `require(cond, "msg")` operand in v1; codegen ignores the text
    /// (a bare `REVERT(0,0)` is enough), but it must lex so the call parses.
    Str(String),
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `;`
    Semi,
    /// `,` (parameter / argument separator).
    Comma,
    /// `=` (assignment).
    Assign,
    /// `=>` (the mapping key→value arrow, in `mapping(K => V)`).
    FatArrow,
    /// `.` (member access, in `msg.sender`).
    Dot,
    /// `+` (addition).
    Plus,
    /// `>` (greater-than comparison).
    Gt,
    /// `<` (less-than comparison).
    Lt,
    /// `>=` (greater-or-equal comparison).
    Ge,
    /// `<=` (less-or-equal comparison).
    Le,
    /// `==` (equality comparison).
    EqEq,
    /// End of input.
    Eof,
}

/// Lex `source` into a [`SolTok`] stream terminated by [`SolKind::Eof`].
///
/// The token type is local to SolidityLite (not rustlite's `TokenKind`) because
/// the keyword/punctuation sets diverge, but it carries the same [`Span`]-bearing
/// convention and errors reuse the shared [`crate::rustlite::CompileError`] + the
/// existing `LH00xx` lexer codes.
pub fn lex(source: &str) -> Result<Vec<SolTok>, CompileError> {
    let mut lx = Lexer { src: source.as_bytes(), pos: 0 };
    let mut out = Vec::new();
    loop {
        let tok = lx.next_token()?;
        let is_eof = tok.kind == SolKind::Eof;
        out.push(tok);
        if is_eof {
            break;
        }
    }
    Ok(out)
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn skip_trivia(&mut self) {
        loop {
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_whitespace() {
                self.pos += 1;
            }
            // `//` line comment.
            if self.pos + 1 < self.src.len() && self.src[self.pos] == b'/' && self.src[self.pos + 1] == b'/' {
                while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
                continue;
            }
            // `/* … */` block comment (non-nesting, Solidity style).
            if self.pos + 1 < self.src.len() && self.src[self.pos] == b'/' && self.src[self.pos + 1] == b'*' {
                self.pos += 2;
                while self.pos + 1 < self.src.len() && !(self.src[self.pos] == b'*' && self.src[self.pos + 1] == b'/') {
                    self.pos += 1;
                }
                // consume the closing `*/` if present
                self.pos = (self.pos + 2).min(self.src.len());
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> Result<SolTok, CompileError> {
        self.skip_trivia();
        let start = self.pos;
        if self.pos >= self.src.len() {
            return Ok(SolTok { kind: SolKind::Eof, span: Span { start, end: start } });
        }
        let b = self.src[self.pos];
        let next = self.src.get(self.pos + 1).copied();
        // Two-byte operators must be checked before their single-byte prefixes.
        // `=>` (mapping arrow) and `==` (equality) both start with `=`; `>=`/`<=`
        // before `>`/`<`.
        let two = match (b, next) {
            (b'=', Some(b'>')) => Some(SolKind::FatArrow),
            (b'=', Some(b'=')) => Some(SolKind::EqEq),
            (b'>', Some(b'=')) => Some(SolKind::Ge),
            (b'<', Some(b'=')) => Some(SolKind::Le),
            _ => None,
        };
        if let Some(kind) = two {
            self.pos += 2;
            return Ok(SolTok { kind, span: Span { start, end: self.pos } });
        }
        // String literal: `"…"` (the `require` message). No escape processing in v1
        // (codegen ignores the text); an unterminated string is a clean error.
        if b == b'"' {
            return self.lex_string(start);
        }
        // Single-char punctuation.
        let punct = match b {
            b'{' => Some(SolKind::LBrace),
            b'}' => Some(SolKind::RBrace),
            b'(' => Some(SolKind::LParen),
            b')' => Some(SolKind::RParen),
            b'[' => Some(SolKind::LBracket),
            b']' => Some(SolKind::RBracket),
            b';' => Some(SolKind::Semi),
            b',' => Some(SolKind::Comma),
            b'=' => Some(SolKind::Assign),
            b'.' => Some(SolKind::Dot),
            b'+' => Some(SolKind::Plus),
            b'>' => Some(SolKind::Gt),
            b'<' => Some(SolKind::Lt),
            _ => None,
        };
        if let Some(kind) = punct {
            self.pos += 1;
            return Ok(SolTok { kind, span: Span { start, end: self.pos } });
        }
        // Identifier / keyword: [A-Za-z_][A-Za-z0-9_]*
        if b.is_ascii_alphabetic() || b == b'_' {
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
            let word = std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("");
            let kind = keyword(word).unwrap_or_else(|| SolKind::Ident(word.to_string()));
            return Ok(SolTok { kind, span: Span { start, end: self.pos } });
        }
        // Integer literal: decimal or 0x-hex, normalized to a 32-byte BE word.
        if b.is_ascii_digit() {
            return self.lex_int(start);
        }
        // Anything else is a byte the floor grammar can't begin a token with.
        Err(CompileError::at_code(
            codes::UNEXPECTED_BYTE,
            format!("unexpected byte {:?} in SolidityLite source", b as char),
            Span { start, end: start + 1 },
        ))
    }

    fn lex_int(&mut self, start: usize) -> Result<SolTok, CompileError> {
        let is_hex = self.src[self.pos] == b'0'
            && self.pos + 1 < self.src.len()
            && (self.src[self.pos + 1] == b'x' || self.src[self.pos + 1] == b'X');
        let word = if is_hex {
            self.pos += 2; // skip 0x
            let digits_start = self.pos;
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            if self.pos == digits_start {
                return Err(CompileError::at_code(
                    codes::BAD_NUMBER,
                    "hex literal `0x` with no digits".to_string(),
                    Span { start, end: self.pos },
                ));
            }
            std::str::from_utf8(&self.src[digits_start..self.pos]).unwrap_or("")
        } else {
            while self.pos < self.src.len() && self.src[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
            std::str::from_utf8(&self.src[start..self.pos]).unwrap_or("")
        };
        // Reject a trailing alphanumeric run (`123abc`) — a malformed literal,
        // not an identifier glued to a number.
        if self.pos < self.src.len()
            && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'_')
        {
            while self.pos < self.src.len()
                && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
            return Err(CompileError::at_code(
                codes::BAD_NUMBER,
                "malformed numeric literal".to_string(),
                Span { start, end: self.pos },
            ));
        }
        let span = Span { start, end: self.pos };
        let word_be32 = if is_hex {
            parse_hex_be32(word, span)?
        } else {
            parse_dec_be32(word, span)?
        };
        Ok(SolTok { kind: SolKind::Int(word_be32), span })
    }

    /// Lex a double-quoted string literal `"…"`. The opening `"` is at `start`.
    /// v1 does NOT process escapes (the contents are only a `require` message,
    /// which codegen discards) — it scans to the next `"` and takes the bytes
    /// between verbatim. An unterminated string (no closing `"`) is a clean error.
    fn lex_string(&mut self, start: usize) -> Result<SolTok, CompileError> {
        self.pos += 1; // consume the opening `"`
        let content_start = self.pos;
        while self.pos < self.src.len() && self.src[self.pos] != b'"' {
            self.pos += 1;
        }
        if self.pos >= self.src.len() {
            return Err(CompileError::at_code(
                codes::UNEXPECTED_BYTE,
                "unterminated string literal".to_string(),
                Span { start, end: self.pos },
            ));
        }
        let text = std::str::from_utf8(&self.src[content_start..self.pos])
            .unwrap_or("")
            .to_string();
        self.pos += 1; // consume the closing `"`
        Ok(SolTok { kind: SolKind::Str(text), span: Span { start, end: self.pos } })
    }
}

/// Map a word to its keyword kind, or `None` if it's a plain identifier.
fn keyword(word: &str) -> Option<SolKind> {
    Some(match word {
        "facet" | "contract" => SolKind::Facet,
        "function" => SolKind::Function,
        "external" => SolKind::External,
        "view" => SolKind::View,
        "pure" => SolKind::Pure,
        "returns" => SolKind::Returns,
        "return" => SolKind::Return,
        "mapping" => SolKind::Mapping,
        "uint256" | "address" | "bool" | "bytes32" => SolKind::TypeName(word.to_string()),
        _ => return None,
    })
}

/// Parse a decimal digit string into a big-endian 32-byte word (rejects overflow
/// past 2^256-1).
fn parse_dec_be32(digits: &str, span: Span) -> Result<[u8; 32], CompileError> {
    let mut word = [0u8; 32]; // accumulator, big-endian
    for ch in digits.bytes() {
        let d = (ch - b'0') as u16;
        // word = word * 10 + d, base-256 long multiply with carry.
        let mut carry = d;
        for byte in word.iter_mut().rev() {
            let v = (*byte as u16) * 10 + carry;
            *byte = (v & 0xFF) as u8;
            carry = v >> 8;
        }
        if carry != 0 {
            return Err(CompileError::at_code(
                codes::BAD_NUMBER,
                "integer literal exceeds uint256 (2^256-1)".to_string(),
                span,
            ));
        }
    }
    Ok(word)
}

/// Parse a hex digit string (no `0x`) into a big-endian 32-byte word (rejects > 64
/// significant hex digits).
fn parse_hex_be32(digits: &str, span: Span) -> Result<[u8; 32], CompileError> {
    if digits.len() > 64 {
        return Err(CompileError::at_code(
            codes::BAD_NUMBER,
            "hex literal exceeds 32 bytes (uint256)".to_string(),
            span,
        ));
    }
    let mut word = [0u8; 32];
    // Decode to nibbles (high → low), then pack two-per-byte from the RIGHT so the
    // value is right-aligned in the 32-byte word.
    let mut nibbles: Vec<u8> = Vec::with_capacity(digits.len());
    for ch in digits.bytes() {
        let v = match ch {
            b'0'..=b'9' => ch - b'0',
            b'a'..=b'f' => ch - b'a' + 10,
            b'A'..=b'F' => ch - b'A' + 10,
            _ => {
                return Err(CompileError::at_code(
                    codes::BAD_NUMBER,
                    "invalid hex digit".to_string(),
                    span,
                ))
            }
        };
        nibbles.push(v);
    }
    // Walk nibbles low-to-high (reversed) in pairs; each pair fills one byte from
    // word[31] leftward. `chunks` over the reversed nibble list = [low, high?].
    let reversed: Vec<u8> = nibbles.into_iter().rev().collect();
    for (byte_off, pair) in reversed.chunks(2).enumerate() {
        let low = pair[0];
        let high = pair.get(1).copied().unwrap_or(0);
        word[31 - byte_off] = (high << 4) | low;
    }
    Ok(word)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<SolKind> {
        lex(src).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_the_floor_grammar() {
        let k = kinds("facet C { function get() external view returns (uint256) { return 42; } }");
        assert_eq!(k[0], SolKind::Facet);
        assert_eq!(k[1], SolKind::Ident("C".into()));
        assert_eq!(k[2], SolKind::LBrace);
        assert_eq!(k[3], SolKind::Function);
        assert_eq!(k[4], SolKind::Ident("get".into()));
        assert!(matches!(k.last(), Some(SolKind::Eof)));
        // the int literal is normalized to BE-32 of 42
        let mut w = [0u8; 32];
        w[31] = 42;
        assert!(k.contains(&SolKind::Int(w)));
    }

    #[test]
    fn decimal_and_hex_agree() {
        // 0xff and 255 produce the same word.
        let dec = lex("255").unwrap()[0].kind.clone();
        let hex = lex("0xff").unwrap()[0].kind.clone();
        assert_eq!(dec, hex);
        let mut w = [0u8; 32];
        w[31] = 0xff;
        assert_eq!(dec, SolKind::Int(w));
        // a 2-byte value
        let dec = lex("256").unwrap()[0].kind.clone();
        let mut w = [0u8; 32];
        w[30] = 0x01;
        assert_eq!(dec, SolKind::Int(w));
    }

    #[test]
    fn lexes_mapping_index_and_msg_sender_tokens() {
        let k = kinds(
            "mapping(address => uint256) bal; bal[msg.sender] = amt; f(uint256 a, address b)",
        );
        assert!(k.contains(&SolKind::Mapping), "`mapping` keyword");
        assert!(k.contains(&SolKind::FatArrow), "`=>` arrow");
        assert!(k.contains(&SolKind::LBracket), "`[`");
        assert!(k.contains(&SolKind::RBracket), "`]`");
        assert!(k.contains(&SolKind::Dot), "`.` (msg.sender)");
        assert!(k.contains(&SolKind::Comma), "`,` param separator");
        // `=>` must NOT lex as a bare `=` followed by `>` (no `>` token exists; `=>`
        // is a single FatArrow). And a standalone `=` still lexes as Assign.
        assert!(k.contains(&SolKind::Assign), "standalone `=` is Assign");
        // `msg` and `sender` are plain identifiers around the Dot.
        assert!(k.contains(&SolKind::Ident("msg".into())));
        assert!(k.contains(&SolKind::Ident("sender".into())));
    }

    #[test]
    fn comments_are_trivia() {
        let k = kinds("// a line\nfacet /* block */ C {}");
        assert_eq!(k[0], SolKind::Facet);
        assert_eq!(k[1], SolKind::Ident("C".into()));
    }

    #[test]
    fn unexpected_byte_is_a_clean_error() {
        let e = lex("facet C { @ }").unwrap_err();
        assert_eq!(e.code, Some(codes::UNEXPECTED_BYTE));
    }

    #[test]
    fn lexes_comparison_operators() {
        let k = kinds("a > b < c >= d <= e == f");
        assert!(k.contains(&SolKind::Gt), "`>`");
        assert!(k.contains(&SolKind::Lt), "`<`");
        assert!(k.contains(&SolKind::Ge), "`>=`");
        assert!(k.contains(&SolKind::Le), "`<=`");
        assert!(k.contains(&SolKind::EqEq), "`==`");
        // `==` must NOT lex as two `=` Assigns.
        assert!(!k.contains(&SolKind::Assign), "`==` is one EqEq, not two Assigns");
    }

    #[test]
    fn ge_le_eqeq_beat_their_single_char_prefixes() {
        // `>=` is ONE token, not `>` then `=`.
        let k = kinds(">=");
        assert_eq!(k[0], SolKind::Ge);
        assert!(matches!(k.get(1), Some(SolKind::Eof)));
        // `<=` likewise.
        let k = kinds("<=");
        assert_eq!(k[0], SolKind::Le);
        // `=>` (the mapping arrow) still beats `==`/`=`.
        let k = kinds("=>");
        assert_eq!(k[0], SolKind::FatArrow);
    }

    #[test]
    fn lexes_a_string_literal() {
        let k = kinds("require(n > 0, \"zero\")");
        assert!(k.contains(&SolKind::Str("zero".into())), "the message string");
        // An empty string is fine too.
        let k = kinds("\"\"");
        assert_eq!(k[0], SolKind::Str(String::new()));
    }

    #[test]
    fn unterminated_string_is_a_clean_error() {
        let e = lex("\"no closing quote").unwrap_err();
        assert_eq!(e.code, Some(codes::UNEXPECTED_BYTE));
    }

    #[test]
    fn overflow_decimal_is_rejected() {
        // 2^256 (one past the max) must error, not wrap.
        let two_256 =
            "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        let e = lex(two_256).unwrap_err();
        assert_eq!(e.code, Some(codes::BAD_NUMBER));
        // 2^256 - 1 is the max and must lex fine.
        let max =
            "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert!(lex(max).is_ok());
    }
}
