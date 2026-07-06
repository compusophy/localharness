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
//!   * `LH3xxx` — **BACKEND / agent-runtime** failures (the chat-facing ones):
//!     a model provider rate-limit / quota, a rejected API key, out-of-credits,
//!     a request timeout, an empty/truncated response, a transport failure. The
//!     `.turn-error` chat line shows the code; [`classify`](crate::error_codes::classify) maps a raw error
//!     string to one of these.
//!   * `LH4xxx` — **SDK CORE** errors — one per [`crate::Error`] variant, so
//!     `Error::code()` always resolves to a stable code (the CLI prints it).
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
//! | `LH3000`–`LH3099` | backend          | provider/transport/agent runtime |
//! | `LH4000`–`LH4099` | core             | one per `Error` enum variant   |
//!
//! A code is a small stable integer + a static category + a one-line meaning +
//! a fix hint. The full human/agent index is `docs/error-codes.md`; a compact
//! list is injected into `self_docs::RUNTIME_SUMMARY` so the agent knows the
//! codes it will see. This module is pure data — no feature gates, no deps — so
//! it compiles on every target and is unit-testable headlessly.

/// The families a code belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `LH0xxx` — a rustlite compile error.
    Compile,
    /// `LH1xxx` — a cartridge runtime error.
    Runtime,
    /// `LH2xxx` — an on-chain transaction revert.
    TxRevert,
    /// `LH3xxx` — a backend / agent-runtime failure (provider/transport/chat).
    Backend,
    /// `LH4xxx` — an SDK core error (one per [`crate::Error`] variant).
    Core,
}

/// The order families are listed in the compact agent-facing index.
pub const FAMILIES: [Family; 5] = [
    Family::Compile,
    Family::Runtime,
    Family::TxRevert,
    Family::Backend,
    Family::Core,
];

impl Family {
    /// The family of a numeric code by its thousands digit.
    pub fn of(code: u16) -> Option<Family> {
        match code {
            1..=999 => Some(Family::Compile),
            1000..=1999 => Some(Family::Runtime),
            2000..=2999 => Some(Family::TxRevert),
            3000..=3999 => Some(Family::Backend),
            4000..=4999 => Some(Family::Core),
            _ => None,
        }
    }

    /// A short label for the index / overlay.
    pub fn label(self) -> &'static str {
        match self {
            Family::Compile => "compile",
            Family::Runtime => "runtime",
            Family::TxRevert => "tx-revert",
            Family::Backend => "backend",
            Family::Core => "core",
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

// ── LH3xxx — BACKEND / agent-runtime codes ──────────────────────────────────
// The chat-facing failures. [`classify`] maps a raw error string to one of
// these; the `.turn-error` line and the telemetry signature carry the label.
/// `LH3001` — the model provider rate-limited the request or the project quota
/// / spending cap is exhausted (HTTP 429 / `RESOURCE_EXHAUSTED`).
pub const BACKEND_RATE_LIMIT: u16 = 3001;
/// `LH3002` — the model rejected the API key / the request was unauthorized
/// (HTTP 401/403, `PERMISSION_DENIED`, `UNAUTHENTICATED`).
pub const BACKEND_AUTH: u16 = 3002;
/// `LH3003` — out of platform credits: the proxy 402'd (no $LH / no session).
pub const BACKEND_CREDITS: u16 = 3003;
/// `LH3004` — the model request timed out / produced no response in time.
pub const BACKEND_TIMEOUT: u16 = 3004;
/// `LH3005` — the model returned an empty or truncated response.
pub const BACKEND_EMPTY: u16 = 3005;
/// `LH3006` — the model backend errored (HTTP 5xx / internal server error).
pub const BACKEND_SERVER: u16 = 3006;
/// `LH3007` — a network / transport failure reaching the backend or proxy.
pub const BACKEND_NETWORK: u16 = 3007;
/// `LH3008` — request auth went stale: the device clock is off by more than the
/// proxy's freshness window (a `stale or future timestamp` rejection).
pub const BACKEND_STALE_AUTH: u16 = 3008;
/// `LH3009` — the request POST failed at the transport layer with NO response
/// (reqwest's bare "error sending request" — on wasm a rejected `fetch()`,
/// flaky mobile networks; telemetry #41). Unlike `LH3007`'s named causes, this
/// wording is ambiguous about whether the request reached the server, so the
/// stream-open retry treats it more conservatively (ONE retry).
pub const BACKEND_SEND: u16 = 3009;

// ── LH4xxx — SDK CORE codes (one per `Error` variant) ───────────────────────
/// `LH4001` — `Error::Io`: an OS-level I/O error.
pub const CORE_IO: u16 = 4001;
/// `LH4002` — `Error::Json`: a (de)serialization error.
pub const CORE_JSON: u16 = 4002;
/// `LH4003` — `Error::Http`: an HTTP transport error not matched by
/// [`classify`](crate::error_codes::classify).
pub const CORE_HTTP: u16 = 4003;
/// `LH4004` — `Error::Closed`: the connection closed unexpectedly.
pub const CORE_CLOSED: u16 = 4004;
/// `LH4005` — `Error::NotStarted`: the operation needs a started agent.
pub const CORE_NOT_STARTED: u16 = 4005;
/// `LH4006` — `Error::AlreadyStarted`: `start()` was called more than once.
pub const CORE_ALREADY_STARTED: u16 = 4006;
/// `LH4007` — `Error::Config`: invalid configuration.
pub const CORE_CONFIG: u16 = 4007;
/// `LH4008` — `Error::ToolNotFound`: no tool registered under that name.
pub const CORE_TOOL_NOT_FOUND: u16 = 4008;
/// `LH4009` — `Error::ToolFailed`: a tool errored during execution.
pub const CORE_TOOL_FAILED: u16 = 4009;
/// `LH4010` — `Error::PolicyDenied`: a policy blocked the operation.
pub const CORE_POLICY_DENIED: u16 = 4010;
/// `LH4011` — `Error::Timeout`: an operation exceeded its deadline.
pub const CORE_TIMEOUT: u16 = 4011;
/// `LH4012` — `Error::Other`: a catch-all not matched by
/// [`classify`](crate::error_codes::classify).
pub const CORE_OTHER: u16 = 4012;
/// `LH4013` — `Error::Decode`: a payload failed to decode (provider JSON/SSE
/// frame, restored history bytes) and [`classify`](crate::error_codes::classify)
/// matched nothing. (`Error::Transport` has no own code — an unmatched
/// transport failure falls back to [`BACKEND_NETWORK`].)
pub const CORE_DECODE: u16 = 4013;

/// The full registry — the SINGLE source of truth. `docs/error-codes.md` is a
/// hand-maintained index checked against this table (the
/// `index_doc_lists_every_code` test asserts the doc lists every code's label),
/// and `self_docs` injects a compact slice into the system prompt.
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
    // LH3xxx backend / agent runtime
    ec(BACKEND_RATE_LIMIT, Family::Backend, "model provider rate-limited / over quota",
       "the platform's model provider is throttled or over its spend cap — wait a moment and retry; not a problem with your account"),
    ec(BACKEND_AUTH, Family::Backend, "model API key rejected",
       "check the Gemini/model API key (BYOK); on the platform path this is a server-side key issue to report"),
    ec(BACKEND_CREDITS, Family::Backend, "out of platform credits ($LH)",
       "redeem a code or top up — this signing address has no active session / no $LH"),
    ec(BACKEND_TIMEOUT, Family::Backend, "the model request timed out",
       "the backend didn't respond in time — retry; if it persists the provider may be degraded"),
    ec(BACKEND_EMPTY, Family::Backend, "empty or truncated model response",
       "the model returned nothing usable — retry; shortening the input can help"),
    ec(BACKEND_SERVER, Family::Backend, "model backend error (5xx)",
       "the provider returned a server error — transient; retry shortly"),
    ec(BACKEND_NETWORK, Family::Backend, "network / transport failure",
       "couldn't reach the backend or proxy — check connectivity and retry"),
    ec(BACKEND_STALE_AUTH, Family::Backend, "request auth went stale (device clock skew)",
       "your device clock is off by more than ~5 minutes — sync it and retry"),
    ec(BACKEND_SEND, Family::Backend, "request POST failed in transit (no response)",
       "the network dropped the request before a response arrived — usually a flaky connection; retry"),
    // LH4xxx SDK core
    ec(CORE_IO, Family::Core, "I/O error",
       "an OS-level read/write failed — check paths and permissions"),
    ec(CORE_JSON, Family::Core, "JSON (de)serialization error",
       "malformed or unexpected JSON — verify the payload shape"),
    ec(CORE_HTTP, Family::Core, "HTTP transport error",
       "the request failed at the transport layer — retry; check the endpoint"),
    ec(CORE_CLOSED, Family::Core, "connection closed unexpectedly",
       "the stream/connection dropped — restart the operation"),
    ec(CORE_NOT_STARTED, Family::Core, "agent not started",
       "call start() before using the agent"),
    ec(CORE_ALREADY_STARTED, Family::Core, "agent already started",
       "start() was called more than once — reuse the running agent"),
    ec(CORE_CONFIG, Family::Core, "invalid configuration",
       "fix the configuration value named in the message"),
    ec(CORE_TOOL_NOT_FOUND, Family::Core, "tool not found",
       "no tool is registered under that name — register it or fix the name"),
    ec(CORE_TOOL_FAILED, Family::Core, "tool execution failed",
       "the tool returned an error — see the inline message for the cause"),
    ec(CORE_POLICY_DENIED, Family::Core, "policy denied the operation",
       "a policy blocked this action — adjust the request or the policy"),
    ec(CORE_TIMEOUT, Family::Core, "operation timed out",
       "the operation exceeded its deadline — raise the timeout or retry"),
    ec(CORE_OTHER, Family::Core, "unspecified error",
       "a catch-all error — see the inline message for details"),
    ec(CORE_DECODE, Family::Core, "payload decode error",
       "the bytes didn't match the expected shape — the message names the codec boundary"),
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
    for fam in FAMILIES {
        out.push_str(fam.label());
        out.push_str(":\n");
        for e in REGISTRY.iter().filter(|e| e.family == fam) {
            out.push_str(&format!("  {} {}\n", e.label(), e.meaning));
        }
    }
    out.trim_end().to_string()
}

/// Map a REAL HTTP status code to a stable `LH3xxx` backend code — the
/// structured twin of [`classify`], which has to substring-match "429"/"503"
/// out of prose. Used by [`classify_http`] for [`crate::Error::HttpStatus`];
/// `None` for statuses that carry no backend meaning on their own (e.g. 400).
pub fn classify_status(status: u16) -> Option<u16> {
    match status {
        429 => Some(BACKEND_RATE_LIMIT),
        401 | 403 => Some(BACKEND_AUTH),
        402 => Some(BACKEND_CREDITS),
        408 => Some(BACKEND_TIMEOUT),
        500..=599 => Some(BACKEND_SERVER),
        _ => None,
    }
}

/// Structured classification for an HTTP failure with a KNOWN status code
/// (`Error::HttpStatus`): the one body-borne semantic override that must win
/// regardless of status runs first (a stale device clock arrives as a 401 but
/// is NOT an auth-key problem), then the real status decides
/// ([`classify_status`]), then full string classification of the body is the
/// fallback (a provider 400 whose body says "API key not valid" is still an
/// auth failure). Legacy string-only errors keep using [`classify`] directly.
pub fn classify_http(status: u16, body: &str) -> Option<u16> {
    let l = body.to_lowercase();
    if l.contains("stale or future timestamp") || l.contains("clock") {
        return Some(BACKEND_STALE_AUTH);
    }
    classify_status(status).or_else(|| classify(body))
}

/// Map a raw error string to a stable `LH3xxx` backend/runtime code — the SINGLE
/// source of truth for turning an opaque provider/proxy/transport message into a
/// code. Used by both the chat `.turn-error` surface and [`crate::Error::code`]
/// (for the string-wrapping `Http`/`Other`/`ToolFailed` variants). Returns
/// `None` when nothing matches, so the caller can fall back to a core code.
/// When the real numeric status is known, prefer the structured
/// [`classify_http`] over substring-matching the digits out of prose.
///
/// Order matters — most specific first. Pure + case-insensitive; no deps, so it
/// is unit-tested headlessly.
pub fn classify(s: &str) -> Option<u16> {
    let l = s.to_lowercase();
    // Stale device clock first — the proxy phrases it distinctively and it must
    // NOT be mistaken for an auth-key problem.
    if l.contains("stale or future timestamp") || l.contains("clock") {
        return Some(BACKEND_STALE_AUTH);
    }
    // Rate-limit / quota before credits: a provider 429 / spend-cap is NOT the
    // user being out of $LH (the historic conflation that showed a "redeem" card
    // for a provider quota error).
    if l.contains("429")
        || l.contains("rate limit")
        || l.contains("rate-limit")
        || l.contains("resource_exhausted")
        || l.contains("spending cap")
        || l.contains("spend cap")
        || l.contains("too many requests")
        || l.contains("quota")
        || l.contains("overloaded")
    {
        return Some(BACKEND_RATE_LIMIT);
    }
    if l.contains("401")
        || l.contains("403")
        || l.contains("api key")
        || l.contains("api_key")
        || l.contains("permission_denied")
        || l.contains("unauthenticated")
        || l.contains("unauthorized")
    {
        return Some(BACKEND_AUTH);
    }
    if l.contains("402")
        || l.contains("payment required")
        || l.contains("no $lh")
        || l.contains("no credit")
        || (l.contains("insufficient")
            && (l.contains("credit")
                || l.contains("balance")
                || l.contains("funds")
                || l.contains("$lh")))
        || l.contains("no active session")
    {
        return Some(BACKEND_CREDITS);
    }
    if l.contains("timed out") || l.contains("timeout") || l.contains("deadline") {
        return Some(BACKEND_TIMEOUT);
    }
    if l.contains("empty response")
        || l.contains("response truncated")
        || l.contains("output truncated")
        || l.contains("truncated response")
        || l.contains("no response")
    {
        return Some(BACKEND_EMPTY);
    }
    if l.contains("500")
        || l.contains("502")
        || l.contains("503")
        || l.contains("504")
        || l.contains("internal server")
    {
        return Some(BACKEND_SERVER);
    }
    if l.contains("network")
        || l.contains("connection")
        || l.contains("failed to fetch")
        || l.contains("dns")
    {
        return Some(BACKEND_NETWORK);
    }
    // reqwest's opaque transport wording for a POST that produced NO response
    // (telemetry #41: "gemini POST: error sending request" on mobile — on wasm
    // a rejected fetch() carries no detail). Checked AFTER the named-cause
    // classes above: a message that also says connection/dns/tls names a
    // definitive pre-send failure and stays LH3007; the bare form can't prove
    // the request never reached the server, hence its own code.
    if l.contains("error sending request") {
        return Some(BACKEND_SEND);
    }
    None
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
    fn compact_index_covers_all_families() {
        let idx = compact_index();
        for fam in FAMILIES {
            assert!(idx.contains(&format!("{}:", fam.label())), "missing family {}", fam.label());
        }
        assert!(idx.contains("LH0204"));
        assert!(idx.contains("LH1001"));
        assert!(idx.contains("LH2003"));
        assert!(idx.contains("LH3001"));
        assert!(idx.contains("LH4001"));
    }

    #[test]
    fn classify_maps_common_backend_errors() {
        assert_eq!(classify("gemini HTTP 429 Too Many Requests"), Some(BACKEND_RATE_LIMIT));
        assert_eq!(classify("status: RESOURCE_EXHAUSTED, spending cap"), Some(BACKEND_RATE_LIMIT));
        assert_eq!(classify("exceeded your quota"), Some(BACKEND_RATE_LIMIT));
        assert_eq!(classify("the model is overloaded"), Some(BACKEND_RATE_LIMIT));
        assert_eq!(classify("HTTP 401 Unauthorized: bad API key"), Some(BACKEND_AUTH));
        assert_eq!(classify("PERMISSION_DENIED"), Some(BACKEND_AUTH));
        assert_eq!(classify("402 Payment Required: no $LH"), Some(BACKEND_CREDITS));
        assert_eq!(classify("the request timed out"), Some(BACKEND_TIMEOUT));
        assert_eq!(classify("empty response from model"), Some(BACKEND_EMPTY));
        assert_eq!(classify("model output truncated at max_tokens"), Some(BACKEND_EMPTY));
        assert_eq!(classify("the connection was truncated mid-stream"), Some(BACKEND_NETWORK));
        assert_eq!(classify("HTTP 503 internal server error"), Some(BACKEND_SERVER));
        assert_eq!(classify("failed to fetch: network down"), Some(BACKEND_NETWORK));
        assert_eq!(classify("stale or future timestamp"), Some(BACKEND_STALE_AUTH));
        assert_eq!(classify("a perfectly ordinary message"), None);
    }

    /// Telemetry #41: reqwest's bare transport failure ("error sending
    /// request" — a rejected fetch() on wasm/mobile) must classify as the
    /// retryable send class, NOT fall through to CORE_OTHER (which made the
    /// stream-open retry fail fast and surfaced a hard turn error).
    #[test]
    fn classify_maps_bare_send_failure_to_backend_send() {
        assert_eq!(classify("gemini POST: error sending request"), Some(BACKEND_SEND));
        assert_eq!(classify("anthropic POST: error sending request"), Some(BACKEND_SEND));
        assert_eq!(classify("openai POST: error sending request"), Some(BACKEND_SEND));
        // A named cause wins over the bare wording: still LH3007 (network).
        assert_eq!(
            classify("error sending request: tcp connect error: Connection refused"),
            Some(BACKEND_NETWORK)
        );
        assert_eq!(classify("error sending request: dns error"), Some(BACKEND_NETWORK));
        // A send timeout stays a timeout.
        assert_eq!(classify("error sending request: operation timed out"), Some(BACKEND_TIMEOUT));
    }

    #[test]
    fn classify_prefers_rate_limit_over_credits() {
        // A provider 429 / spend-cap must NOT be classified as out-of-credits
        // (the historic conflation that showed a "redeem" card for a quota error).
        assert_eq!(
            classify("429 RESOURCE_EXHAUSTED: project exceeded its monthly spending cap"),
            Some(BACKEND_RATE_LIMIT)
        );
    }

    #[test]
    fn classify_status_reads_the_real_number() {
        assert_eq!(classify_status(429), Some(BACKEND_RATE_LIMIT));
        assert_eq!(classify_status(401), Some(BACKEND_AUTH));
        assert_eq!(classify_status(403), Some(BACKEND_AUTH));
        assert_eq!(classify_status(402), Some(BACKEND_CREDITS));
        assert_eq!(classify_status(408), Some(BACKEND_TIMEOUT));
        for s in [500, 502, 503, 504, 529] {
            assert_eq!(classify_status(s), Some(BACKEND_SERVER), "status {s}");
        }
        // Statuses with no backend meaning of their own stay unclassified.
        assert_eq!(classify_status(400), None);
        assert_eq!(classify_status(404), None);
        assert_eq!(classify_status(200), None);
    }

    #[test]
    fn classify_http_status_first_with_overrides_and_fallback() {
        // Structured: the status decides even with an opaque body.
        assert_eq!(classify_http(429, "<opaque provider body>"), Some(BACKEND_RATE_LIMIT));
        assert_eq!(classify_http(503, "x"), Some(BACKEND_SERVER));
        // Stale device clock overrides the 401 it arrives under.
        assert_eq!(classify_http(401, "stale or future timestamp"), Some(BACKEND_STALE_AUTH));
        // Unmapped status falls back to the body string.
        assert_eq!(classify_http(400, "API key not valid"), Some(BACKEND_AUTH));
        assert_eq!(classify_http(400, "exceeded your quota"), Some(BACKEND_RATE_LIMIT));
        assert_eq!(classify_http(418, "a perfectly ordinary message"), None);
    }

    #[test]
    fn classify_narrows_bare_insufficient() {
        // Bare "insufficient" (e.g. a provider "insufficient storage") must NOT
        // be treated as out-of-credits — that showed a spurious redeem card.
        assert_ne!(classify("insufficient storage"), Some(BACKEND_CREDITS));
        // Money-shaped "insufficient" still maps to credits.
        assert_eq!(classify("insufficient credit balance"), Some(BACKEND_CREDITS));
        assert_eq!(classify("402 payment required"), Some(BACKEND_CREDITS));
        // "insufficient quota" is caught earlier as rate-limit, not credits.
        assert_eq!(classify("insufficient quota"), Some(BACKEND_RATE_LIMIT));
    }
}
