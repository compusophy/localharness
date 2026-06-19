//! Abstract syntax for bashlite — a tiny, total shell.
//!
//! A script is a list of [`Stmt`]s. Words ([`Word`]) carry their interpolation
//! structure from the lexer so eval expands them with no re-scanning.

pub use super::token::WordPart;

/// A word = a list of segments concatenated (then field-split on whitespace if
/// the result of an unquoted expansion contains spaces — but v1 keeps it
/// simple: one word expands to exactly one string; no field splitting).
pub type Word = Vec<WordPart>;

/// A statement in a script body.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `name=word` — assign a variable (the value is the expanded word).
    Assign { name: String, value: Word },
    /// A pipeline of one or more simple commands joined by `|`. A bare command
    /// is a one-element pipeline. The whole pipeline yields the LAST command's
    /// exit code (`$?`).
    Pipeline(Vec<Command>),
    /// Pipelines chained by `&&` / `||` with SHORT-CIRCUIT semantics: `a && b`
    /// runs `b` only if `a` exited 0; `a || b` runs `b` only if `a` exited
    /// nonzero. `pipelines.len() == ops.len() + 1`. The statement's exit code is
    /// the LAST pipeline actually run.
    AndOr {
        pipelines: Vec<Vec<Command>>,
        ops: Vec<ChainOp>,
    },
    /// `if COND; then BODY; [elif COND; then BODY;]* [else BODY;] fi`
    If {
        /// `(condition_pipeline, body)` pairs — the first whose condition exits
        /// 0 runs. `elif` arms append here.
        arms: Vec<(Vec<Stmt>, Vec<Stmt>)>,
        /// Optional `else` body.
        otherwise: Option<Vec<Stmt>>,
    },
    /// `for NAME in WORD...; do BODY; done`
    For {
        var: String,
        items: Vec<Word>,
        body: Vec<Stmt>,
    },
    /// `while COND; do BODY; done`
    While {
        cond: Vec<Stmt>,
        body: Vec<Stmt>,
    },
}

/// The operator joining two pipelines in an [`Stmt::AndOr`] chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainOp {
    /// `&&` — run the next pipeline only on the previous's success (exit 0).
    And,
    /// `||` — run the next pipeline only on the previous's failure (nonzero).
    Or,
}

/// A simple command: a command name plus argument words. `[ ... ]` tests parse
/// as a command named `[` with the test words as args (so the eval path is
/// uniform with builtins).
#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    /// The command word (e.g. `echo`, `ls`, `[`). May itself be an expansion.
    pub name: Word,
    /// Argument words.
    pub args: Vec<Word>,
}
