# localharness error-code index (`LHxxxx`)

Every failure on this platform carries a **stable `LHxxxx` code**: a small integer
behind an `LH`-prefixed, zero-padded label (e.g. `LH0204`). Learn the index once;
the code never changes meaning and is never renumbered (codes are only appended).

This is the human/agent index — modeled on
[rustc's error index](https://rustc-dev-guide.rust-lang.org/diagnostics/error-codes.html).
The single source of truth is the Rust registry `src/error_codes.rs` (`REGISTRY`);
this file is kept in lockstep (a test pins the count + every code's presence). A
compact slice is injected into the agent's system prompt via `self_docs`.

## Numbering scheme

| Range             | Family               | Stage / source                         |
|-------------------|----------------------|----------------------------------------|
| `LH0001`–`LH0099` | compile: **lexer**   | byte/string/char/number lexing         |
| `LH0100`–`LH0199` | compile: **parser**  | unexpected token / structure           |
| `LH0200`–`LH0299` | compile: **typecheck** | types / arity / scope                |
| `LH0300`–`LH0399` | compile: **codegen** | lowering / unsupported emit            |
| `LH1000`–`LH1099` | **runtime**          | cartridge Web-Worker failures          |
| `LH2000`–`LH2099` | **tx revert**        | facet custom-error selectors           |
| `LH3000`–`LH3099` | **backend**          | model provider / transport / agent runtime |
| `LH4000`–`LH4099` | **core**             | one per SDK `Error` enum variant       |

The thousands digit is the family. `LH0xxx` = a rustlite **compile** error (carried
on `CompileError`, surfaced by the `compile_rustlite` tool). `LH1xxx` = a cartridge
**runtime** error (reported by the Web-Worker engine; shown in the "CARTRIDGE
STOPPED" overlay). `LH2xxx` = an on-chain **transaction revert** (mapped from a
known facet custom-error selector by the registry's revert decoder). `LH3xxx` = a
**backend / agent-runtime** failure (a model provider rate-limit, a rejected key,
out-of-credits, a timeout — shown on the `.turn-error` chat line). `LH4xxx` = an
**SDK core** error, one per `Error` enum variant, so `Error::code()` always
resolves and the CLI can print a code.

---

## `LH0xxx` — rustlite compile errors

These are the most numerous and the most valuable: the `compile_rustlite` tool
returns the code plus a fix hint, and the compiler message reads
`LH0204: type mismatch: ... [12..18]` — the code, the message, and the
`[start..end]` source byte span.

### Lexer (`LH00xx`)

| Code | Meaning | Common cause | Fix |
|------|---------|--------------|-----|
| `LH0001` | unexpected byte in source | a stray non-Rust-subset character | remove the stray character; rustlite accepts ASCII Rust-subset source |
| `LH0002` | unterminated string literal | missing closing `"`, or a newline inside the string | add the closing `"` on the same line (strings can't span newlines) |
| `LH0003` | unknown string/char escape | a `\x` escape rustlite doesn't support | use a supported escape: `\n` `\t` `\\` `\"` `\0` |
| `LH0004` | malformed char literal | `''` (empty), `'AB'` (multi-byte), or unclosed | a `'x'` char is exactly one byte; use a `"string"` for text |
| `LH0005` | malformed numeric literal | bad int/float/hex digits or suffix | check the digits/suffix; hex is `0xFF`, floats need a fractional digit |

### Parser (`LH01xx`)

| Code | Meaning | Common cause | Fix |
|------|---------|--------------|-----|
| `LH0100` | unexpected token | a token didn't match the grammar | read the `[start..end]` span; supply the expected token |
| `LH0101` | expected a top-level item | a non-item at the top level | only `fn`/`struct`/`enum`/`const` are allowed at the top level |
| `LH0102` | expected a type | a missing/invalid type in a type position | supply a known type (`i32`/`i64`/`f32`/`f64`/`bool` or a declared struct/enum) |
| `LH0103` | expected an expression | a dangling operator or empty position | an expression is required here |
| `LH0104` | expected a pattern | a malformed `match` arm / `let` pattern (incl. a range with no upper bound) | use a binding, literal, path, or range pattern |
| `LH0105` | missing `;` after a statement | two statements run together | terminate the statement with `;` (or close the block with `}`) |
| `LH0106` | invalid assignment target | assigning to a non-place (`5 = 9`), or an indexed write through a struct field (`s.arr[i] = v`) | assign to a variable, struct field, or `arr[i]` |
| `LH0107` | nesting too deep | deeply-nested expressions/blocks past the recursion cap | flatten the nesting |

### Typecheck (`LH02xx`)

| Code | Meaning | Common cause | Fix |
|------|---------|--------------|-----|
| `LH0200` | unknown type name | a type that isn't declared / primitive | declare the struct/enum, or use a primitive |
| `LH0201` | undefined variable | use before `let`, or a typo | declare it with `let` before use, or fix the spelling |
| `LH0202` | unknown function | a call to a fn rustlite doesn't know | define the fn, or use a valid host fn (`host::display::*`, `host::net::*`, …) |
| `LH0203` | wrong number of arguments | arity mismatch on a fn / host fn call | match the parameter count exactly |
| `LH0204` | type mismatch | operand/argument/binding types disagree | convert with an `as` cast or fix the operand types |
| `LH0205` | assignment to a non-`mut` binding | reassigning a `let` without `mut` | declare it `let mut` to reassign |
| `LH0206` | field access on a non-struct / missing field | `.field` on a non-struct or unknown field | access a real field of a struct value |
| `LH0207` | invalid index expression | indexing a non-array, a non-i32 index, or an empty/non-i32 array | index an array with an i32; only arrays of i32 are indexable |
| `LH0208` | invalid `as` cast | `as` between non-numeric types | `as` only converts between numbers (`i32`/`i64`/`f32`/`f64`) |
| `LH0209` | unknown struct in a literal | constructing an undeclared struct | declare the struct before constructing it |

### Codegen (`LH03xx`)

| Code | Meaning | Common cause | Fix |
|------|---------|--------------|-----|
| `LH0300` | unsupported language feature | a construct codegen can't lower | rustlite lacks traits/generics/references/heap types (`Vec`/`String`/`Box`)/globals |
| `LH0301` | unknown host import | a wrong `host::` path/name | use a registered host fn — check the `host::display` / `host::net` / `host::audio` names + arity |
| `LH0302` | no `frame`/`render` entry export | the cartridge defines no entry point | add `fn frame(t: i32)` (animated) or `fn render()` (one-shot) |
| `LH0303` | cartridge exceeds the publish size cap | the compiled wasm is too big to publish on-chain | shrink the cartridge below the publish cap |

> Note: `LH0302`/`LH0303` are enforced at the **publish/CLI** boundary
> (the `src/bin/localharness/` CLI, the loader) rather than inside `compile()` — a bare
> `compile()` succeeds for an entry-less or oversize module; the codes label the
> check that rejects it on the way to a public face.

---

## `LH1xxx` — cartridge runtime errors

The single-cartridge path runs the cartridge in a Web Worker (`web/cartridge-worker.js`)
so a hung or trapping `frame()` can't freeze the app. Each fatal path reports a code
in its `{type:'error', code, detail}` message; the main thread paints the code + its
meaning into the "CARTRIDGE STOPPED" overlay. Containment is unchanged — these codes
only **label** it.

| Code | Meaning | Common cause | Fix |
|------|---------|--------------|-----|
| `LH1001` | cartridge hung (watchdog terminated it) | a `frame()` ran too long / looped unbounded | bound your loops; reload to retry. (Assigned on the main thread — a hung worker can't post.) |
| `LH1002` | cartridge trapped during a frame | a wasm trap: `unreachable`, out-of-bounds, divide-by-zero | check array indices + arithmetic |
| `LH1003` | cartridge failed to instantiate | an invalid/incompatible wasm module | recompile with `compile_rustlite` |
| `LH1004` | cartridge exports neither `frame` nor `render` | no entry point in the loaded module | export `fn frame(t: i32)` or `fn render()` |

> Surfacing in the on-canvas overlay is **browser-confirmable only** (the worker +
> watchdog need a real `Worker`/`Window`). The code-assignment and message wiring are
> proven headlessly (Rust + the worker-host-parity harness).

---

## `LH2xxx` — on-chain transaction reverts

A sponsored Tempo write that reverts is re-run read-only and its revert data decoded.
A known facet custom-error selector maps to a code + the facet error name + what to do,
e.g. `LH2003: SpendExceedsBudget — the run would spend more $LH than the job's
remaining budget …`, instead of a bare 4-byte selector.

| Code | Facet error | Meaning / fix |
|------|-------------|---------------|
| `LH2001` | `NotDue()` | the scheduled job isn't due yet; the scheduler only fires on the interval |
| `LH2002` | `StaleNextRun()` | this run already fired; the on-chain clock advanced |
| `LH2003` | `SpendExceedsBudget()` | over the job's remaining budget — top it up |
| `LH2004` | `NotScheduler()` | scheduler-worker-only call; not a user action |
| `LH2005` | `NotJobOwner()` | you don't own this job — use the right `--as` identity |
| `LH2006` | `UnknownJob()` | no job with that id — `localharness jobs` |
| `LH2007` | `JobNotActive()` | already cancelled/exhausted |
| `LH2008` | `JobNotPaused()` | only a paused job can be resumed |
| `LH2009` | `UnregisteredTarget()` | the target isn't a registered agent |
| `LH2010` | `ZeroInterval()` | interval below the 60s minimum |
| `LH2011` | `ZeroRuns()` | max-runs must be ≥ 1 |
| `LH2012` | `CodeTaken()` | that invite code already exists — generate a fresh one |
| `LH2013` | `BadTtl()` | invite TTL outside 1h..90d |
| `LH2014` | `EscrowCapExceeded()` | past the per-funder escrow cap |
| `LH2015` | `UnknownInvite()` | no invite for that code |
| `LH2016` | `NotOpen()` | invite already accepted/reclaimed |
| `LH2017` | `Expired()` | invite past its TTL — reclaimable by its funder |
| `LH2018` | `NotYetExpired()` | reclaim only works after the TTL |
| `LH2019` | `ZeroBudget()` | budget must be > 0 |
| `LH2020` | `ZeroAmount()` | amount must be > 0 |
| `LH2021` | `NotConfigured()` | credits token unset — a platform misconfiguration |
| `LH2022` | `Error(string)` | a `require(reason)` revert — the reason is decoded inline (a balance/escrow reason means you need more `$LH`) |
| `LH2023` | `Panic(uint256)` | an internal assertion — a platform bug, not your input |
| `LH2024` | `InsufficientCredits()` | chat-meter credits being withdrawn/bridged are locked (fiat-minted `$LH` is spend-only) or short — `check_balances` shows the withdrawable amount |

---

## `LH3xxx` — backend / agent-runtime errors

The chat-facing failures. `error_codes::classify()` maps a raw provider/proxy/transport
error string to one of these; the `.turn-error` chat line shows `LH3xxx · <meaning> —
<hint>`, and the telemetry report groups by the code. A provider **429 / spend-cap is
`LH3001`, not `LH3003`** — a quota error is the platform's, not the user being out of `$LH`.

| Code | Meaning | Common cause | Fix |
|------|---------|--------------|-----|
| `LH3001` | model provider rate-limited / over quota | HTTP 429 / `RESOURCE_EXHAUSTED` / spend cap | wait and retry; not an account problem |
| `LH3002` | model API key rejected | HTTP 401/403, `PERMISSION_DENIED` | check the BYOK key; on the platform path, a server-side key issue |
| `LH3003` | out of platform credits (`$LH`) | the proxy 402'd (no $LH / no session) | redeem or top up |
| `LH3004` | the model request timed out | no response in time | retry; the provider may be degraded |
| `LH3005` | empty or truncated model response | the model returned nothing usable | retry; shorten the input |
| `LH3006` | model backend error (5xx) | a provider server error | transient — retry shortly |
| `LH3007` | network / transport failure | couldn't reach the backend/proxy | check connectivity and retry |
| `LH3008` | request auth went stale (device clock skew) | clock off by > ~5 min | sync the device clock and retry |
| `LH3009` | request POST failed in transit (no response) | a flaky connection dropped the request (mobile radio blip) | auto-retried once; retry if it persists |

---

## `LH4xxx` — SDK core errors

One code per [`Error`](https://docs.rs/localharness) enum variant, so `Error::code()`
always resolves to a stable code (the `src/bin/localharness/` CLI prints it on failure).
The string-wrapping variants (`Http`/`ToolFailed`/`Other`) first defer to `classify()`,
so they surface a `LH3xxx` backend code when the message matches one; so do the typed
`Transport` (falls back to `LH3007` — an unmatched transport failure IS a network
failure) and `Decode { what, message }` (falls back to `LH4013`) variants. The structured
`HttpStatus { status, message }` variant (what the model backends raise on a non-2xx
response) classifies off its **real status code** via `classify_http()` /
`classify_status()` — no substring parsing — falling back to the message string for
unmapped statuses, and to `LH4003` when nothing matches. Consumers can read the raw
status via `Error::http_status_code()`. The `Fs { op, path, message }` and
`BadArgs { tool, message }` variants map STRUCTURALLY (`LH4001` / `LH4009`) with
**no** `classify()` pass — their messages embed user paths / model-authored args
that must not false-positive into a backend class.

| Code | `Error` variant | Meaning |
|------|-----------------|---------|
| `LH4001` | `Io` / `Fs` | an OS-level I/O error / a filesystem-operation failure |
| `LH4002` | `Json` | a JSON (de)serialization error |
| `LH4003` | `Http` / `HttpStatus` | an HTTP transport error not matched by classification |
| `LH4004` | `Closed` | the connection closed unexpectedly |
| `LH4005` | `NotStarted` | the operation needs a started agent |
| `LH4006` | `AlreadyStarted` | `start()` was called more than once |
| `LH4007` | `Config` | invalid configuration |
| `LH4008` | `ToolNotFound` | no tool registered under that name |
| `LH4009` | `ToolFailed` / `BadArgs` | a tool errored during execution / rejected its arguments |
| `LH4010` | `PolicyDenied` | a policy blocked the operation |
| `LH4011` | `Timeout` | an operation exceeded its deadline |
| `LH4012` | `Other` | a catch-all not matched by `classify()` |
| `LH4013` | `Decode` | a payload failed to decode (provider JSON/SSE frame, restored history) |

---

*Generated from `src/error_codes.rs` — keep this file in sync when adding a code
(the `index_doc_lists_every_code` test enforces it).*
