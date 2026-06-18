//! Recursive-descent parser: [`Token`]s → a list of [`Stmt`]s.
//!
//! Keywords (`if`, `then`, `for`, `while`, …) are not lexed specially — they
//! arrive as plain single-literal words, and the parser recognizes them by
//! their literal text at a statement boundary. A keyword only acts as a keyword
//! in command-NAME position; quoted (`"if"`) or mid-word it stays a normal arg,
//! exactly like the shell.

use super::ast::{Command, Stmt, Word};
use super::token::{Token, WordPart};
use super::BashError;

/// Parse a full token stream into a script body.
pub fn parse(tokens: &[Token]) -> Result<Vec<Stmt>, BashError> {
    let mut p = Parser { toks: tokens, pos: 0 };
    let body = p.block(&[])?;
    p.expect_eof()?;
    Ok(body)
}

struct Parser<'a> {
    toks: &'a [Token],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> &Token {
        self.toks.get(self.pos).unwrap_or(&Token::Eof)
    }

    fn bump(&mut self) -> &'a Token {
        let t = self.toks.get(self.pos).unwrap_or(&Token::Eof);
        if self.pos < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn expect_eof(&self) -> Result<(), BashError> {
        match self.peek() {
            Token::Eof => Ok(()),
            other => Err(BashError::parse(format!("unexpected trailing token: {other:?}"))),
        }
    }

    /// Skip any run of `Semi` separators.
    fn skip_semis(&mut self) {
        while matches!(self.peek(), Token::Semi) {
            self.pos += 1;
        }
    }

    /// If the next token is a bare keyword word equal to `kw`, return true
    /// WITHOUT consuming it. A word is a keyword only if it's a single literal
    /// segment exactly equal to `kw` (so `"if"` quoted — still one Lit — also
    /// matches; that's acceptable for v1 since these words are reserved).
    fn at_keyword(&self, kw: &str) -> bool {
        matches!(self.peek(), Token::Word(parts) if is_kw(parts, kw))
    }

    /// Consume a required keyword.
    fn eat_keyword(&mut self, kw: &str) -> Result<(), BashError> {
        if self.at_keyword(kw) {
            self.pos += 1;
            Ok(())
        } else {
            Err(BashError::parse(format!("expected `{kw}`, found {:?}", self.peek())))
        }
    }

    /// Parse statements until a token whose keyword is in `terminators` (or
    /// EOF). Does NOT consume the terminator. Statements are separated by one or
    /// more `Semi`; a missing separator between two commands is an error.
    fn block(&mut self, terminators: &[&str]) -> Result<Vec<Stmt>, BashError> {
        let mut stmts = Vec::new();
        loop {
            self.skip_semis();
            if matches!(self.peek(), Token::Eof) {
                break;
            }
            if terminators.iter().any(|kw| self.at_keyword(kw)) {
                break;
            }
            stmts.push(self.statement()?);
            // After a statement, the next token must be a separator, a block
            // terminator, or EOF — otherwise two commands ran together.
            match self.peek() {
                Token::Semi | Token::Eof => {}
                _ if terminators.iter().any(|kw| self.at_keyword(kw)) => {}
                other => {
                    return Err(BashError::parse(format!(
                        "expected `;` or newline between commands, found {other:?}"
                    )))
                }
            }
        }
        Ok(stmts)
    }

    fn statement(&mut self) -> Result<Stmt, BashError> {
        if self.at_keyword("if") {
            return self.if_stmt();
        }
        if self.at_keyword("for") {
            return self.for_stmt();
        }
        if self.at_keyword("while") {
            return self.while_stmt();
        }
        // An assignment is a leading word of the exact shape `NAME=...` with a
        // valid identifier NAME, in command-name position, with no following
        // args. (`x=1 cmd` env-prefix form is NOT supported in v1.)
        if let Token::Word(parts) = self.peek() {
            if let Some((name, value)) = split_assignment(parts) {
                self.pos += 1;
                // A following word would be the unsupported env-prefix form.
                if let Token::Word(_) = self.peek() {
                    return Err(BashError::parse(
                        "env-prefix assignment (`X=v cmd`) is not supported; put the assignment on its own line",
                    ));
                }
                return Ok(Stmt::Assign { name, value });
            }
        }
        self.pipeline()
    }

    fn pipeline(&mut self) -> Result<Stmt, BashError> {
        let mut cmds = vec![self.command()?];
        while matches!(self.peek(), Token::Pipe) {
            self.pos += 1;
            cmds.push(self.command()?);
        }
        Ok(Stmt::Pipeline(cmds))
    }

    /// Parse one simple command: a name word followed by argument words, until a
    /// `Pipe`, `Semi`, `Eof`, or a block keyword. `[ ... ]` needs no special
    /// case — `[` is just a command name and `]` a final arg.
    fn command(&mut self) -> Result<Command, BashError> {
        let name = match self.bump() {
            Token::Word(parts) => parts.clone(),
            other => return Err(BashError::parse(format!("expected a command, found {other:?}"))),
        };
        // Args run until a separator/pipe/EOF. Reserved words are NOT special in
        // ARGUMENT position (shell semantics: `echo done` is echo with arg
        // "done"); a block terminator only ends a command via the leading `;`
        // that must precede it, which `block` consumes before re-checking the
        // terminator at statement-start. So `… ; done` works without `done`
        // being treated as an arg.
        let mut args = Vec::new();
        while let Token::Word(parts) = self.peek() {
            args.push(parts.clone());
            self.pos += 1;
        }
        Ok(Command { name, args })
    }

    fn if_stmt(&mut self) -> Result<Stmt, BashError> {
        self.eat_keyword("if")?;
        let mut arms = Vec::new();
        // first arm
        let cond = self.block(&["then"])?;
        self.eat_keyword("then")?;
        let body = self.block(&["elif", "else", "fi"])?;
        arms.push((cond, body));
        // elif arms
        while self.at_keyword("elif") {
            self.eat_keyword("elif")?;
            let cond = self.block(&["then"])?;
            self.eat_keyword("then")?;
            let body = self.block(&["elif", "else", "fi"])?;
            arms.push((cond, body));
        }
        let otherwise = if self.at_keyword("else") {
            self.eat_keyword("else")?;
            Some(self.block(&["fi"])?)
        } else {
            None
        };
        self.eat_keyword("fi")?;
        Ok(Stmt::If { arms, otherwise })
    }

    fn for_stmt(&mut self) -> Result<Stmt, BashError> {
        self.eat_keyword("for")?;
        let var = match self.bump() {
            Token::Word(parts) => single_ident(parts)
                .ok_or_else(|| BashError::parse("for: expected a variable name"))?,
            other => return Err(BashError::parse(format!("for: expected a name, found {other:?}"))),
        };
        self.eat_keyword("in")?;
        // Collect item words until `;`/newline or `do`.
        let mut items = Vec::new();
        loop {
            match self.peek() {
                Token::Word(parts) if !is_kw(parts, "do") => {
                    items.push(parts.clone());
                    self.pos += 1;
                }
                _ => break,
            }
        }
        self.skip_semis();
        self.eat_keyword("do")?;
        let body = self.block(&["done"])?;
        self.eat_keyword("done")?;
        Ok(Stmt::For { var, items, body })
    }

    fn while_stmt(&mut self) -> Result<Stmt, BashError> {
        self.eat_keyword("while")?;
        let cond = self.block(&["do"])?;
        self.eat_keyword("do")?;
        let body = self.block(&["done"])?;
        self.eat_keyword("done")?;
        Ok(Stmt::While { cond, body })
    }
}

/// True if `parts` is a single literal segment equal to `kw`.
fn is_kw(parts: &[WordPart], kw: &str) -> bool {
    matches!(parts, [WordPart::Lit(s)] if s == kw)
}

/// If `parts` is a single literal of the form `NAME=REST` where NAME is a valid
/// identifier, return `(NAME, value-word)`. The value keeps interpolation: an
/// assignment like `p=$(...)` lexes the RHS as a `Subst` part, so we split only
/// when the FIRST literal segment contains an `=` after a valid name prefix.
fn split_assignment(parts: &[WordPart]) -> Option<(String, Word)> {
    let WordPart::Lit(first) = parts.first()? else {
        return None;
    };
    let eq = first.find('=')?;
    let name = &first[..eq];
    if name.is_empty() || !is_ident(name) {
        return None;
    }
    // Build the value word: the remainder of the first literal after `=`, then
    // the rest of the parts verbatim.
    let mut value: Word = Vec::new();
    let rest = &first[eq + 1..];
    if !rest.is_empty() {
        value.push(WordPart::Lit(rest.to_string()));
    }
    value.extend_from_slice(&parts[1..]);
    Some((name.to_string(), value))
}

/// A single bare identifier word (no interpolation) — used for `for VAR`.
fn single_ident(parts: &[WordPart]) -> Option<String> {
    match parts {
        [WordPart::Lit(s)] if is_ident(s) => Some(s.clone()),
        _ => None,
    }
}

/// A valid shell variable identifier: `[A-Za-z_][A-Za-z0-9_]*`.
pub(crate) fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}
