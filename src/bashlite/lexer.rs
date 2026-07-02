//! Byte-level lexer: source → [`Token`]s. Shell-shaped, not Rust-shaped.
//!
//! Words carry their interpolation structure ([`WordPart`]) so the parser
//! never re-scans for `$`. Quoting rules are the deterministic subset of POSIX
//! sh: single quotes are fully literal; double quotes allow `$var`/`$(...)`
//! interpolation; a backslash escapes the next char outside single quotes.

use super::token::{Token, WordPart};
use super::BashError;

/// Lex `src` into a token stream terminated by [`Token::Eof`].
pub fn lex(src: &str) -> Result<Vec<Token>, BashError> {
    Lexer { src: src.as_bytes(), pos: 0 }.run()
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl Lexer<'_> {
    fn run(&mut self) -> Result<Vec<Token>, BashError> {
        let mut out = Vec::new();
        loop {
            self.skip_blanks_and_comments();
            let Some(b) = self.peek() else {
                out.push(Token::Eof);
                return Ok(out);
            };
            match b {
                b'\n' | b';' => {
                    self.pos += 1;
                    // Collapse runs of separators into one `Semi` so blank lines
                    // and `a;;b` don't produce empty commands.
                    if !matches!(out.last(), Some(Token::Semi) | None) {
                        out.push(Token::Semi);
                    }
                }
                b'|' => {
                    self.pos += 1;
                    if self.peek() == Some(b'|') {
                        self.pos += 1;
                        out.push(Token::OrOr);
                    } else {
                        out.push(Token::Pipe);
                    }
                }
                b'&' => {
                    self.pos += 1;
                    if self.peek() == Some(b'&') {
                        self.pos += 1;
                        out.push(Token::AndAnd);
                    } else {
                        // Lone `&` (background / file-descriptor dup) is unsupported.
                        return Err(BashError::parse(
                            "lone '&' is not supported (no background jobs); use '&&' to chain",
                        ));
                    }
                }
                _ => out.push(self.word()?),
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    /// Skip spaces/tabs/carriage-returns and `#` line comments. Newlines and
    /// `;` are SIGNIFICANT (command separators) so they are NOT skipped here.
    fn skip_blanks_and_comments(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r') => self.pos += 1,
                // `#` starts a comment only at a word boundary (start of a word).
                // Anywhere mid-word it's a literal — but since we only call this
                // between words, a leading `#` is always a comment.
                Some(b'#') => {
                    while !matches!(self.peek(), None | Some(b'\n')) {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    /// Lex one word: a maximal run of non-blank, non-operator bytes, honoring
    /// quotes. Produces the word's interpolation segments.
    fn word(&mut self) -> Result<Token, BashError> {
        let mut parts: Vec<WordPart> = Vec::new();
        let mut lit = String::new();

        // Flush the pending literal run into a `Lit` part (coalescing adjacent
        // literals so `[Lit, Lit]` never happens).
        macro_rules! flush {
            () => {
                if !lit.is_empty() {
                    match parts.last_mut() {
                        Some(WordPart::Lit(s)) => s.push_str(&lit),
                        _ => parts.push(WordPart::Lit(std::mem::take(&mut lit))),
                    }
                    lit.clear();
                }
            };
        }

        while let Some(b) = self.peek() {
            match b {
                // Word terminators (unquoted).
                b' ' | b'\t' | b'\r' | b'\n' | b';' | b'|' | b'&' => break,
                b'\'' => {
                    self.pos += 1;
                    while let Some(c) = self.peek() {
                        if c == b'\'' {
                            break;
                        }
                        lit.push(c as char);
                        self.pos += 1;
                    }
                    if self.peek() != Some(b'\'') {
                        return Err(BashError::parse("unterminated single quote"));
                    }
                    self.pos += 1;
                }
                b'"' => {
                    self.pos += 1;
                    loop {
                        match self.peek() {
                            None => return Err(BashError::parse("unterminated double quote")),
                            Some(b'"') => {
                                self.pos += 1;
                                break;
                            }
                            Some(b'\\') => {
                                // In double quotes, backslash only escapes a few
                                // chars; otherwise it's literal (POSIX). `\<newline>`
                                // is still a line splice inside quotes (as it is
                                // unquoted, line 164), so consume both.
                                self.pos += 1;
                                match self.peek() {
                                    Some(b'\n') => self.pos += 1,
                                    Some(c @ (b'"' | b'\\' | b'$' | b'`')) => {
                                        lit.push(c as char);
                                        self.pos += 1;
                                    }
                                    _ => lit.push('\\'),
                                }
                            }
                            Some(b'$') => {
                                flush!();
                                parts.push(self.dollar()?);
                            }
                            Some(c) => {
                                lit.push(c as char);
                                self.pos += 1;
                            }
                        }
                    }
                }
                b'\\' => {
                    // Outside quotes a backslash escapes the next byte literally
                    // (including a separator), and `\<newline>` is a line splice.
                    self.pos += 1;
                    match self.peek() {
                        None => lit.push('\\'),
                        Some(b'\n') => self.pos += 1,
                        Some(c) => {
                            lit.push(c as char);
                            self.pos += 1;
                        }
                    }
                }
                b'$' => {
                    flush!();
                    parts.push(self.dollar()?);
                }
                c => {
                    lit.push(c as char);
                    self.pos += 1;
                }
            }
        }
        flush!();
        Ok(Token::Word(parts))
    }

    /// Lex a `$`-expansion starting at the `$`: `$name`, `${name}`, or `$(...)`.
    fn dollar(&mut self) -> Result<WordPart, BashError> {
        debug_assert_eq!(self.peek(), Some(b'$'));
        self.pos += 1;
        match self.peek() {
            // `$(...)` command substitution — capture the balanced inner source.
            Some(b'(') => {
                self.pos += 1;
                let start = self.pos;
                let mut depth = 1usize;
                while let Some(c) = self.peek() {
                    match c {
                        b'(' => depth += 1,
                        b')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                    self.pos += 1;
                }
                if self.peek() != Some(b')') {
                    return Err(BashError::parse("unterminated $( ) substitution"));
                }
                let inner = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|_| BashError::parse("non-utf8 in substitution"))?
                    .to_string();
                self.pos += 1; // consume `)`
                Ok(WordPart::Subst(inner))
            }
            // `${name}`
            Some(b'{') => {
                self.pos += 1;
                let start = self.pos;
                while matches!(self.peek(), Some(c) if c != b'}') {
                    self.pos += 1;
                }
                if self.peek() != Some(b'}') {
                    return Err(BashError::parse("unterminated ${ } expansion"));
                }
                let name = std::str::from_utf8(&self.src[start..self.pos])
                    .map_err(|_| BashError::parse("non-utf8 var name"))?
                    .to_string();
                self.pos += 1; // consume `}`
                if name.is_empty() {
                    return Err(BashError::parse("empty ${} variable name"));
                }
                Ok(WordPart::Var(name))
            }
            // `$?` — the last command's exit status (the one special parameter
            // v1 supports). Emitted as a `Var("?")`, expanded by the evaluator.
            Some(b'?') => {
                self.pos += 1;
                Ok(WordPart::Var("?".to_string()))
            }
            // `$name` — a bare identifier (letters, digits, underscore; must not
            // start with a digit). A `$` followed by anything else is literal.
            Some(c) if c == b'_' || c.is_ascii_alphabetic() => {
                let start = self.pos;
                while matches!(self.peek(), Some(c) if c == b'_' || c.is_ascii_alphanumeric()) {
                    self.pos += 1;
                }
                let name = std::str::from_utf8(&self.src[start..self.pos]).unwrap().to_string();
                Ok(WordPart::Var(name))
            }
            // A lone `$` (e.g. `$` at end, or `$5` is unsupported) is a literal `$`.
            _ => Ok(WordPart::Lit("$".to_string())),
        }
    }
}
