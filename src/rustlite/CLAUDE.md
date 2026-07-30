# src/rustlite — Rust-subset → wasm compiler subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/rustlite/`). This
> is the cartridge language (compiles to `wasm32` cartridges the display framebuffer
> runs). PURE, no deps, native+wasm unit-testable. Pipeline:
> `lexer → token → parser → ast → typecheck → codegen` (wasm emitter) `→ loader`
> (wasm32 cartridge runtime). `soliditylite` is the EVM analog — same shape, separate.

## Cartridge contract
A cartridge exports `fn frame(t: i32)` (animated) or `fn render()` (one-shot) — the
loader calls one. No entry export → `LH0302` (compile) / `LH1004` (runtime).
The DEFAULT framebuffer is **512x512** — `loader::DEFAULT_FB_*` is the one source
(`app::display` re-uses it; `cartridge-worker.js` hand-ports it as
`FB_W_DEFAULT`/`FB_H_DEFAULT`, guard `tests/framebuffer_default_parity.rs`).
It is NOT 320x240; that stale figure survived in tool descriptions + docs long
after the code moved, and agents laid out for the wrong surface then drew
off-screen, where every primitive silently clips (telemetry #73). A cartridge
overrides via `dims() -> i32` (packed `(w<<16)|h`, each clamped 16..=1024).

## Host imports are INTEGER-ONLY — do NOT add a string-passing import
The host ABI crossing the wasm boundary passes only integers: `host::display::*`
(`draw_char`, `draw_number`, …), `host::net::*`, `host::audio::*`, `host::agent::*`.
Strings can't cross directly. The host tables in `loader.rs` are HAND-PORTED to JS
in `web/cartridge-worker.js` and PARITY-TESTED (`test-compose-wiring.mjs`, verify.sh).
**Adding a host fn means editing `loader.rs` AND `cartridge-worker.js` in lockstep**
— a missing JS binding fails instantiation ("module is not an object or function").
Prefer a CODEGEN DESUGAR over a new import when the data is compile-time-known.

## A cartridge is also a LIBRARY (host::compose::call, telemetry #70)
Every rustlite `fn` is emitted as a wasm export (`codegen::emit_module` exports
ALL functions, not just the entry), so a published cartridge is already a callable
library. `compose::spawn_lib(name)` mounts one HEADLESS and
`compose::call(h,"fn",a0..a3)` invokes it; `compose::call_ok()` carries the
outcome because `call` returns the export's own i32. Consequences when you touch
this: the ABI passes at most `compose::MAX_CALL_ARGS`=4 ints (a wider export is
REFUSED, not called with garbage — arity is `fn.length` on the JS side); status
codes + caps are SSOT in `src/compose.rs`; and a library still needs a
`frame`/`render` (publish gates on it — `publish::cartridge_has_entry`), which is
its landing card and never runs while mounted headless.
`examples/cartridges/lib_physics.rl` + `uses_lib.rl` are the pair.

## `draw_string` + `host::math` are desugars, not host imports (the pattern to copy)
`host::display::draw_string(x,y,"LIT",color,scale)` lowers at the PARSER stage to
one `draw_char` per glyph (6px stride, matching `raster::draw_number`) — NO new host
import, integer-only ABI intact. Validated at compile time: literal-only 3rd arg
(`EXPECTED_EXPRESSION`), printable-ASCII (`UNEXPECTED_BYTE`), ≤256 bytes
(`OVERSIZE`), arity (`ARITY_MISMATCH`). When you need a "host fn that takes text,"
do THIS instead of a new import. `host::math::sin/cos` (telemetry #83) is the
second desugar: a baked 512-byte sine table in the data segment + an inline
load16_s — angle = 1/256 turn (wraps via & 255), result x256. No import, no
worker edit, identical in every runtime including compose children.

## Error codes: every diagnostic carries an `LH0xxx`
`CompileError::code` → lexer `LH00xx` · parser `LH01xx` · typecheck `LH02xx` ·
codegen `LH03xx` (full registry: `crate::error_codes`). Unsupported language
features error CLEANLY as `LH0300` (no traits/generics/references/heap types
[Vec/String/Box]/globals — rustlite is a small subset). Don't silently miscompile;
emit a coded error. `compile_rustlite` STUBS host imports so a compile-only check
doesn't need `run_cartridge` (on-chain feedback #15).

## Scoping gotcha (FIXED — don't regress)
`alloc_local` is MONOTONIC and inits before binding, so shadowing `let` is safe; the
residual is flat last-wins (NO block-scope pop). If you touch local allocation, keep
the monotonic-alloc + init-before-bind invariant or shadowing breaks (the old bug).

## wasm emit
Codegen emits post-MVP wasm features modern rustc emits — `wasm-opt` is DISABLED in
`build-web.sh` because the bundled wasm-opt rejects them. Don't re-enable it.

## E2E proofs live in `examples/` (e.g. cartridges/*.rl); run them, don't trust a
green unit test alone for a codegen change — compile AND run a cartridge.
