//! The one `LHxxxx` error-code registry — a single source of truth spanning
//! the three failure families an agent (or a user) hits on this platform:
//!
//!   * `LH0xxx` — **rustlite COMPILE errors** (lexer / parser / typecheck /
//!     codegen). The most numerous + most valuable: every compiler diagnostic
//!     carries one (see [`crate::rustlite::CompileError::code`]).
//!   * `LH1xxx` — **cartridge RUNTIME errors** (the Web-Worker cartridge engine:
//!     a hung/trapped `frame()`, a missing entry, an instantiate failure). The
//!     worker reports the code in its `{type:'error', code, detail}` message and
//!     the "CARTRIDGE STOPPED" overlay shows it.
//!   * `LH2xxx` — **on-chain TX REVERTS** (the known facet custom-error
//!     selectors). [`crate::registry`]'s revert decoder maps a 4-byte selector
//!     to its code so a revert surfaces `LH2xxx: <name> — <meaning>` instead of
//!     a bare hash.
//!
//! Numbering scheme (stable — codes are NEVER renumbered, only appended):
//!
//! | Range        | Family                | Sub-range by stage            |
//! |--------------|-----------------------|-------------------------------|
//! | `LH0001`–`LH0099` | compile: lexer   | byte/string/char/number lexing |
//! | `LH0100`–`LH0199` | compile: parser  | unexpected token / structure   |
//! | `LH0200`–`LH0299` | compile: typecheck | types / arity / scope        |
//! | `LH0300`–`LH0399` | compile: codegen | lowering / unsupported emit    |
//! | `LH1000`–`LH1099` | runtime          | cartridge worker failures      |
//! | `LH2000`–`LH2099` | tx revert        | facet custom-error selectors   |
//!
//! A code is a small stable integer + a static category + a one-line meaning +
//! a fix hint. The full human/agent index is `docs/error-codes.md`; a compact
//! list is injected into `self_docs::RUNTIME_SUMMARY` so the agent knows the
//! codes it will see. This module is pure data — no feature gates, no deps — so
//! it compiles on every target and is unit-testable headlessly.

/// The three families a code belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `LH0xxx` — a rustlite compile error.
    Compile,
    /// `LH1xxx` — a cartridge runtime error.
    Runtime,
    /// `LH2xxx` — an on-chain transaction revert.
    TxRevert,
}

impl Family {
    /// The family of a numeric code by its thousands digit.
    pub fn of(code: u16) -> Option<Family> {
        match code {
            1..=999 => Some(Family::Compile),
            1000..=1999 => Some(Family::Runtime),
            2000..=2999 => Some(Family::TxRevert),
            _ => None,
        }
    }

    /// A short label for the index / overlay.
    pub fn label(self) -> &'static str {
        match self {
            Family::Compile => "compile",
            Family::Runtime => "runtime",
            Family::TxRevert => "tx-revert",
        }
    }
}

/// One registry entry: a stable code, its family, a one-line meaning, and a
/// fix hint. `code` is the integer behind the `LHxxxx` label (e.g. `20` →
/// `LH0020`), so the printed form is always four zero-padded digits.
#[derive(Debug, Clone, Copy)]
pub struct ErrorCode {
    /// The stable integer (printed zero-padded to 4 digits after `LH`).
    pub code: u16,
    /// Which of the three families this belongs to.
    pub family: Family,
    /// A short, stable meaning (no trailing period).
    pub meaning: &'static str,
    /// A one-line, actionable fix hint.
    pub hint: &'static str,
}

impl ErrorCode {
    /// The canonical `LHxxxx` label, e.g. `LH0020`.
    pub fn label(&self) -> String {
        format!("LH{:04}", self.code)
    }
}

/// Format an `LHxxxx` label from a bare integer without a registry lookup.
/// (`fmt_label(20)` → `"LH0020"`.)
pub fn fmt_label(code: u16) -> String {
    format!("LH{code:04}")
}

// ── LH0xxx — rustlite COMPILE codes ─────────────────────────────────────────
// Lexer LH00xx.
/// `LH0001` — an unexpected byte the lexer can't begin a token with.
pub const UNEXPECTED_BYTE: u16 = 1;
/// `LH0002` — a string literal with no closing quote (or a newline inside).
pub const UNTERMINATED_STRING: u16 = 2;
/// `LH0003` — an unknown `\x` escape in a string or char literal.
pub const UNKNOWN_ESCAPE: u16 = 3;
/// `LH0004` — a char literal that isn't exactly one byte (empty/multi/unclosed).
pub const BAD_CHAR_LITERAL: u16 = 4;
/// `LH0005` — a malformed numeric literal (bad int/float/hex digits).
pub const BAD_NUMBER: u16 = 5;

// Parser LH01xx.
/// `LH0100` — a token didn't match what the grammar required here.
pub const UNEXPECTED_TOKEN: u16 = 100;
/// `LH0101` — expected an item (fn/struct/enum/const) at the top level.
pub const EXPECTED_ITEM: u16 = 101;
/// `LH0102` — expected a type in a type position.
pub const EXPECTED_TYPE: u16 = 102;
/// `LH0103` — expected the start of an expression.
pub const EXPECTED_EXPRESSION: u16 = 103;
/// `LH0104` — expected a pattern in a `match` arm / `let`.
pub const EXPECTED_PATTERN: u16 = 104;
/// `LH0105` — a statement isn't terminated by `;` or `}`.
pub const MISSING_SEMICOLON: u16 = 105;
/// `LH0106` — the assignment target isn't an assignable place (incl. `arr[i] = v`).
pub const INVALID_ASSIGN_TARGET: u16 = 106;
/// `LH0107` — expression/block nesting exceeded the parser's recursion cap.
pub const NESTING_TOO_DEEP: u16 = 107;

// Typecheck LH02xx.
/// `LH0200` — a use of an unknown type name.
pub const UNKNOWN_TYPE: u16 = 200;
/// `LH0201` — a reference to a variable that isn't in scope.
pub const UNDEFINED_VARIABLE: u16 = 201;
/// `LH0202` — a call to a function rustlite doesn't know.
pub const UNKNOWN_FUNCTION: u16 = 202;
/// `LH0203` — a call with the wrong number of arguments.
pub const ARITY_MISMATCH: u16 = 203;
/// `LH0204` — an operand/argument/binding type didn't match what's required.
pub const TYPE_MISMATCH: u16 = 204;
/// `LH0205` — assignment to a binding declared without `mut`.
pub const NOT_MUTABLE: u16 = 205;
/// `LH0206` — a field access on something that isn't that struct.
pub const BAD_FIELD_ACCESS: u16 = 206;
/// `LH0207` — indexing a non-array, a non-i32 index, or an unsupported array.
pub const BAD_INDEX: u16 = 207;
/// `LH0208` — an `as` cast between non-numeric types.
pub const BAD_CAST: u16 = 208;
/// `LH0209` — an unknown struct in a struct-literal.
pub const UNKNOWN_STRUCT: u16 = 209;

// Codegen LH03xx.
/// `LH0300` — codegen hit a construct it can't lower to wasm.
pub const UNSUPPORTED_FEATURE: u16 = 300;
/// `LH0301` — a host import the codegen tables don't know (wrong `host::` path).
pub const UNKNOWN_HOST_IMPORT: u16 = 301;
/// `LH0302` — the compiled cartridge has no `frame`/`render` entry export.
pub const NO_ENTRY: u16 = 302;
/// `LH0303` — the compiled cartridge exceeds the on-chain publish size cap.
pub const OVERSIZE: u16 = 303;

// ── LH1xxx — cartridge RUNTIME codes ────────────────────────────────────────
/// `LH1001` — a frame stopped posting; the watchdog terminated a hung cartridge.
pub const FRAME_TIMEOUT: u16 = 1001;
/// `LH1002` — the cartridge trapped during `frame()`/`render()` (unreachable / OOB).
pub const WASM_TRAP: u16 = 1002;
/// `LH1003` — `WebAssembly.instantiate` failed (a bad/incompatible module).
pub const INSTANTIATE_FAILED: u16 = 1003;
/// `LH1004` — the loaded module exports neither `frame` nor `render`.
pub const NO_ENTRY_RUNTIME: u16 = 1004;

// ── LH2xxx — on-chain TX-REVERT codes ───────────────────────────────────────
// Each maps to a facet custom-error selector in `registry::decode_known_revert`.
/// `LH2001` — ScheduleFacet `NotDue()`.
pub const TX_NOT_DUE: u16 = 2001;
/// `LH2002` — ScheduleFacet `StaleNextRun()`.
pub const TX_STALE_NEXT_RUN: u16 = 2002;
/// `LH2003` — ScheduleFacet `SpendExceedsBudget()`.
pub const TX_SPEND_EXCEEDS_BUDGET: u16 = 2003;
/// `LH2004` — ScheduleFacet `NotScheduler()`.
pub const TX_NOT_SCHEDULER: u16 = 2004;
/// `LH2005` — ScheduleFacet `NotJobOwner()`.
pub const TX_NOT_JOB_OWNER: u16 = 2005;
/// `LH2006` — ScheduleFacet `UnknownJob()`.
pub const TX_UNKNOWN_JOB: u16 = 2006;
/// `LH2007` — ScheduleFacet `JobNotActive()`.
pub const TX_JOB_NOT_ACTIVE: u16 = 2007;
/// `LH2008` — ScheduleFacet `JobNotPaused()`.
pub const TX_JOB_NOT_PAUSED: u16 = 2008;
/// `LH2009` — ScheduleFacet `UnregisteredTarget()`.
pub const TX_UNREGISTERED_TARGET: u16 = 2009;
/// `LH2010` — ScheduleFacet `ZeroInterval()`.
pub const TX_ZERO_INTERVAL: u16 = 2010;
/// `LH2011` — ScheduleFacet `ZeroRuns()`.
pub const TX_ZERO_RUNS: u16 = 2011;
/// `LH2012` — InviteFacet `CodeTaken()`.
pub const TX_CODE_TAKEN: u16 = 2012;
/// `LH2013` — InviteFacet `BadTtl()`.
pub const TX_BAD_TTL: u16 = 2013;
/// `LH2014` — InviteFacet `EscrowCapExceeded()`.
pub const TX_ESCROW_CAP_EXCEEDED: u16 = 2014;
/// `LH2015` — InviteFacet `UnknownInvite()`.
pub const TX_UNKNOWN_INVITE: u16 = 2015;
/// `LH2016` — InviteFacet `NotOpen()`.
pub const TX_NOT_OPEN: u16 = 2016;
/// `LH2017` — InviteFacet `Expired()`.
pub const TX_EXPIRED: u16 = 2017;
/// `LH2018` — InviteFacet `NotYetExpired()`.
pub const TX_NOT_YET_EXPIRED: u16 = 2018;
/// `LH2019` — shared `ZeroBudget()`.
pub const TX_ZERO_BUDGET: u16 = 2019;
/// `LH2020` — shared `ZeroAmount()`.
pub const TX_ZERO_AMOUNT: u16 = 2020;
/// `LH2021` — shared `NotConfigured()`.
pub const TX_NOT_CONFIGURED: u16 = 2021;
/// `LH2022` — a `require(reason)` / `Error(string)` revert (reason decoded inline).
pub const TX_REASON_STRING: u16 = 2022;
/// `LH2023` — a `Panic(uint256)` (internal assert) revert — a platform bug.
pub const TX_PANIC: u16 = 2023;
/// `LH2024` — CreditMeterFacet `InsufficientCredits()` on `withdrawCredits` —
/// the chat-meter credits being pulled out are LOCKED (fiat-minted $LH must be
/// spent on inference, not transferred/bridged to the wallet) or simply short.
pub const TX_INSUFFICIENT_CREDITS: u16 = 2024;

/// The full registry — the SINGLE source of truth. `docs/error-codes.md` is
/// generated/checked against this (the `index_doc_lists_every_code` test pins
/// the count), and `self_docs` injects a compact slice into the system prompt.
pub const REGISTRY: &[ErrorCode] = &[
    // LH0xxx compile — lexer
    ec(UNEXPECTED_BYTE, Family::Compile, "unexpected byte in source",
       "remove the stray character; rustlite only accepts ASCII Rust-subset source"),
    ec(UNTERMINATED_STRING, Family::Compile, "unterminated string literal",
       "add the closing \" on the same line (strings can't span newlines)"),
    ec(UNKNOWN_ESCAPE, Family::Compile, "unknown string/char escape",
       "use a supported escape: \\n \\t \\\\ \\\" \\0"),
    ec(BAD_CHAR_LITERAL, Family::Compile, "malformed char literal",
       "a 'x' char is exactly one byte; use a \"string\" for text"),
    ec(BAD_NUMBER, Family::Compile, "malformed numeric literal",
       "check the digits/suffix; hex is 0xFF, floats need a fractional digit"),
    // LH0xxx compile — parser
    ec(UNEXPECTED_TOKEN, Family::Compile, "unexpected token",
       "the grammar expected a different token here — read the [start..end] span"),
    ec(EXPECTED_ITEM, Family::Compile, "expected a top-level item",
       "only fn/struct/enum/const are allowed at the top level"),
    ec(EXPECTED_TYPE, Family::Compile, "expected a type",
       "supply a known type (i32/i64/f32/f64/bool or a declared struct/enum)"),
    ec(EXPECTED_EXPRESSION, Family::Compile, "expected an expression",
       "an expression is required here; check for a dangling operator"),
    ec(EXPECTED_PATTERN, Family::Compile, "expected a pattern",
       "a match arm / let needs a pattern (binding, literal, path, or range)"),
    ec(MISSING_SEMICOLON, Family::Compile, "missing ';' after a statement",
       "terminate the statement with ';' (or close the block with '}')"),
    ec(INVALID_ASSIGN_TARGET, Family::Compile, "invalid assignment target",
       "assign to a variable, struct field, or arr[i]; non-places (5 = 9) and indexed writes through struct fields (s.arr[i] = v) are unsupported"),
    ec(NESTING_TOO_DEEP, Family::Compile, "nesting too deep",
       "flatten deeply-nested expressions/blocks; the parser caps recursion depth"),
    // LH0xxx compile — typecheck
    ec(UNKNOWN_TYPE, Family::Compile, "unknown type name",
       "declare the struct/enum, or use a primitive (i32/i64/f32/f64/bool)"),
    ec(UNDEFINED_VARIABLE, Family::Compile, "undefined variable",
       "declare it with let before use, or fix the spelling"),
    ec(UNKNOWN_FUNCTION, Family::Compile, "unknown function",
       "define the fn, or use a valid host fn (host::display::*, host::net::*, …)"),
    ec(ARITY_MISMATCH, Family::Compile, "wrong number of arguments",
       "match the function's parameter count exactly"),
    ec(TYPE_MISMATCH, Family::Compile, "type mismatch",
       "convert with an `as` cast or fix the operand types so they agree"),
    ec(NOT_MUTABLE, Family::Compile, "assignment to a non-mut binding",
       "declare it `let mut` to reassign"),
    ec(BAD_FIELD_ACCESS, Family::Compile, "field access on a non-struct / missing field",
       "access a real field of a struct value"),
    ec(BAD_INDEX, Family::Compile, "invalid index expression",
       "index an array with an i32; only arrays of i32 are indexable"),
    ec(BAD_CAST, Family::Compile, "invalid `as` cast",
       "`as` only converts between numbers (i32/i64/f32/f64)"),
    ec(UNKNOWN_STRUCT, Family::Compile, "unknown struct in a literal",
       "declare the struct before constructing it"),
    // LH0xxx compile — codegen
    ec(UNSUPPORTED_FEATURE, Family::Compile, "unsupported language feature",
       "rustlite lacks traits/generics/references/heap types (Vec/String/Box)/globals"),
    ec(UNKNOWN_HOST_IMPORT, Family::Compile, "unknown host import",
       "use a registered host fn — check the host::display / host::net / host::audio names + arity"),
    ec(NO_ENTRY, Family::Compile, "no frame/render entry export",
       "add `fn frame(t: i32)` (animated) or `fn render()` (one-shot) — the loader calls one of these"),
    ec(OVERSIZE, Family::Compile, "cartridge exceeds the publish size cap",
       "shrink the cartridge below the on-chain publish cap before publishing"),
    // LH1xxx runtime
    ec(FRAME_TIMEOUT, Family::Runtime, "cartridge hung (watchdog terminated it)",
       "a frame() ran too long / looped unbounded — bound your loops; reload to retry"),
    ec(WASM_TRAP, Family::Runtime, "cartridge trapped during a frame",
       "a wasm trap (unreachable / out-of-bounds) — check array indices + arithmetic"),
    ec(INSTANTIATE_FAILED, Family::Runtime, "cartridge failed to instantiate",
       "the wasm module is invalid/incompatible — recompile with compile_rustlite"),
    ec(NO_ENTRY_RUNTIME, Family::Runtime, "cartridge exports neither frame nor render",
       "export `fn frame(t: i32)` or `fn render()` so the engine has an entry to call"),
    // LH2xxx tx reverts
    ec(TX_NOT_DUE, Family::TxRevert, "NotDue — job not due yet",
       "the scheduler only fires on the interval; check `localharness jobs`"),
    ec(TX_STALE_NEXT_RUN, Family::TxRevert, "StaleNextRun — run already fired",
       "the on-chain clock already advanced; nothing to do"),
    ec(TX_SPEND_EXCEEDS_BUDGET, Family::TxRevert, "SpendExceedsBudget — over the job budget",
       "top up the job or it will be marked exhausted"),
    ec(TX_NOT_SCHEDULER, Family::TxRevert, "NotScheduler — scheduler-only call",
       "only the scheduler worker can record a run; not a user action"),
    ec(TX_NOT_JOB_OWNER, Family::TxRevert, "NotJobOwner — you don't own this job",
       "use the right `--as` identity; check `localharness jobs`"),
    ec(TX_UNKNOWN_JOB, Family::TxRevert, "UnknownJob — no job with that id",
       "list yours with `localharness jobs` (the id is the #N)"),
    ec(TX_JOB_NOT_ACTIVE, Family::TxRevert, "JobNotActive — already cancelled/exhausted",
       "nothing to cancel; see `localharness jobs`"),
    ec(TX_JOB_NOT_PAUSED, Family::TxRevert, "JobNotPaused — can't resume a running job",
       "only a paused job can be resumed"),
    ec(TX_UNREGISTERED_TARGET, Family::TxRevert, "UnregisteredTarget — target isn't an agent",
       "confirm it exists first (`localharness whoami <target>`)"),
    ec(TX_ZERO_INTERVAL, Family::TxRevert, "ZeroInterval — interval below the 60s minimum",
       "use `--every 60s` or more"),
    ec(TX_ZERO_RUNS, Family::TxRevert, "ZeroRuns — max-runs must be >= 1",
       "drop `--runs 0`"),
    ec(TX_CODE_TAKEN, Family::TxRevert, "CodeTaken — invite code already exists",
       "generate a fresh code (`invite create` makes a new one each time)"),
    ec(TX_BAD_TTL, Family::TxRevert, "BadTtl — TTL outside 1h..90d",
       "use e.g. `--ttl 7d`"),
    ec(TX_ESCROW_CAP_EXCEEDED, Family::TxRevert, "EscrowCapExceeded — past the per-funder cap",
       "reclaim an expired invite or use a smaller amount"),
    ec(TX_UNKNOWN_INVITE, Family::TxRevert, "UnknownInvite — no invite for that code",
       "double-check you copied the full code (incl. the inv- prefix)"),
    ec(TX_NOT_OPEN, Family::TxRevert, "NotOpen — invite already accepted/reclaimed",
       "it's spent; ask for a fresh invite"),
    ec(TX_EXPIRED, Family::TxRevert, "Expired — invite past its TTL",
       "it can only be reclaimed by its funder now (`invite reclaim <code>`)"),
    ec(TX_NOT_YET_EXPIRED, Family::TxRevert, "NotYetExpired — reclaim only after the TTL",
       "until then it can still be accepted"),
    ec(TX_ZERO_BUDGET, Family::TxRevert, "ZeroBudget — budget must be > 0",
       "supply a positive budget"),
    ec(TX_ZERO_AMOUNT, Family::TxRevert, "ZeroAmount — amount must be > 0",
       "supply a positive amount"),
    ec(TX_NOT_CONFIGURED, Family::TxRevert, "NotConfigured — credits token unset",
       "a platform-side misconfiguration; report it via `localharness feedback`"),
    ec(TX_REASON_STRING, Family::TxRevert, "Error(string) — reverted with a reason",
       "the decoded reason is shown inline; an escrow/balance reason means you need more $LH"),
    ec(TX_PANIC, Family::TxRevert, "Panic — internal assertion failed",
       "a platform bug, not your input; please `localharness feedback` it"),
    ec(TX_INSUFFICIENT_CREDITS, Family::TxRevert, "InsufficientCredits — chat-meter credits locked or short",
       "fiat-minted $LH is locked for spending on inference, not withdraw/transfer; check_balances shows the withdrawable amount + unlock time"),
];

/// `const`-friendly constructor for a [`ErrorCode`] table entry.
const fn ec(code: u16, family: Family, meaning: &'static str, hint: &'static str) -> ErrorCode {
    ErrorCode { code, family, meaning, hint }
}

/// Look up an entry by its numeric code.
pub fn lookup(code: u16) -> Option<&'static ErrorCode> {
    REGISTRY.iter().find(|e| e.code == code)
}

/// The cartridge-lifecycle phase a runtime (`LH1xxx`) failure happened in:
/// `"instantiate"` (the module never came up — bad wasm or no entry export)
/// or `"run"` (it instantiated, then trapped or hung). Tool results carry
/// this so an agent knows whether to recompile (instantiate) or fix its
/// frame logic (run) without decoding the numeric code first.
pub fn runtime_phase(code: u16) -> &'static str {
    match code {
        INSTANTIATE_FAILED | NO_ENTRY_RUNTIME => "instantiate",
        _ => "run",
    }
}

/// "LH0204: type mismatch" — the label + meaning, for prefixing a message.
pub fn describe(code: u16) -> String {
    match lookup(code) {
        Some(e) => format!("{}: {}", e.label(), e.meaning),
        None => fmt_label(code),
    }
}

/// A compact, agent-facing list of every code (label + meaning), grouped by
/// family. Injected into the system prompt via `self_docs` so the agent learns
/// the codes once. Newline-separated, no trailing newline.
pub fn compact_index() -> String {
    let mut out = String::new();
    for fam in [Family::Compile, Family::Runtime, Family::TxRevert] {
        out.push_str(fam.label());
        out.push_str(":\n");
        for e in REGISTRY.iter().filter(|e| e.family == fam) {
            out.push_str(&format!("  {} {}\n", e.label(), e.meaning));
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_in_family_range() {
        let mut seen = std::collections::HashSet::new();
        for e in REGISTRY {
            assert!(seen.insert(e.code), "duplicate code LH{:04}", e.code);
            assert_eq!(
                Family::of(e.code),
                Some(e.family),
                "LH{:04} family {:?} doesn't match its numeric range",
                e.code,
                e.family
            );
            // Every entry must have a non-empty meaning + hint.
            assert!(!e.meaning.is_empty() && !e.hint.is_empty(), "LH{:04} blank text", e.code);
        }
    }

    #[test]
    fn label_is_zero_padded() {
        assert_eq!(fmt_label(1), "LH0001");
        assert_eq!(fmt_label(204), "LH0204");
        assert_eq!(fmt_label(2001), "LH2001");
        assert_eq!(lookup(TYPE_MISMATCH).unwrap().label(), "LH0204");
    }

    #[test]
    fn runtime_phase_maps_every_lh1xxx_code() {
        assert_eq!(runtime_phase(INSTANTIATE_FAILED), "instantiate");
        assert_eq!(runtime_phase(NO_ENTRY_RUNTIME), "instantiate");
        assert_eq!(runtime_phase(WASM_TRAP), "run");
        assert_eq!(runtime_phase(FRAME_TIMEOUT), "run");
        // every registered runtime code yields one of the two phases
        for e in REGISTRY.iter().filter(|e| e.family == Family::Runtime) {
            assert!(matches!(runtime_phase(e.code), "instantiate" | "run"));
        }
    }

    #[test]
    fn describe_falls_back_for_unknown() {
        assert_eq!(describe(TYPE_MISMATCH), "LH0204: type mismatch");
        assert_eq!(describe(9999), "LH9999");
    }

    #[test]
    fn index_doc_lists_every_code() {
        // The human/agent index `docs/error-codes.md` must mention every
        // registry code's label, so the doc can't silently drift from the
        // source-of-truth table. (Run from the crate root via CARGO_MANIFEST_DIR.)
        let doc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/docs/error-codes.md"
        ))
        .expect("docs/error-codes.md must exist");
        for e in REGISTRY {
            let label = e.label();
            assert!(
                doc.contains(&label),
                "docs/error-codes.md is missing {label} ({})",
                e.meaning
            );
        }
    }

    #[test]
    fn compact_index_covers_all_three_families() {
        let idx = compact_index();
        assert!(idx.contains("compile:"));
        assert!(idx.contains("runtime:"));
        assert!(idx.contains("tx-revert:"));
        assert!(idx.contains("LH0204"));
        assert!(idx.contains("LH1001"));
        assert!(idx.contains("LH2003"));
    }
}
