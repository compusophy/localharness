---
title: "rustlite: compiling a Rust subset to WebAssembly cartridges that run in the browser"
published: false
description: "An agent SDK that lets agents ship apps, not just text. rustlite is a small Rust-subset compiler — hand-written lexer, parser, typechecker, and a direct wasm binary emitter (no LLVM) — that produces tiny WebAssembly 'cartridges' rendered to a pixel framebuffer in a sandboxed Web Worker. A grounded tour of the language, the pipeline, the integer-only host ABI, and recursive composition, inside localharness."
tags: rust, webassembly, compilers, gamedev
canonical_url: https://localharness.xyz/llms.txt
---

> Draft. Flip `published: true` only after a human review. See the disclosure at
> the end — this is first-party content from the project's own automated account.

Most agent frameworks let an agent *say* things. The output is text, maybe a tool
call, maybe a file. But "here's a thing I built, it's live at a URL, open it" is a
different category of output — and it needs a safe, tiny, no-toolchain way to turn
source into something a browser will actually run.

That's what `rustlite` is: a compiler, shipped *inside* the
[`localharness`](https://github.com/compusophy/localharness) crate, that takes a
**Rust subset** as a string and emits **WebAssembly bytes** — a "cartridge." A
cartridge renders to a pixel framebuffer, runs in a sandboxed Web Worker, and once
published, your subdomain (`yourname.localharness.xyz`) serves it 24/7 with no tab
open and no server you run. No `solc`-style external toolchain, no LLVM, no
`wasm-pack` — `localharness::rustlite::compile(src)` returns `Vec<u8>` and that's
the whole build step.

This post is the implementer's tour: the language, the pipeline, the host ABI that
makes it safe to run an *untrusted* cartridge in a visitor's tab, and how cartridges
compose recursively. Everything here is `cargo add`-able today on stable Rust
(1.85+), Apache-2.0.

## 1. What a cartridge is

A cartridge is a wasm module that exports one entry point:

- `fn frame(t: i32)` — animated; the runtime calls it once per frame with a
  monotonically increasing clock `t`.
- `fn render()` — one-shot; called once.

No entry export is a clean compile error (`LH0302`), not a silent dud. Here is a
complete, real one (`examples/cartridges/bouncing_ball.rl` in the repo) — a ball
that bounces off the walls, position computed as a *pure* function of `t`:

```rust
fn triangle(t: i32, span: i32) -> i32 {
    let period: i32 = span * 2;
    let phase: i32 = t % period;
    if phase < span { phase } else { period - phase }
}

fn frame(t: i32) {
    host::display::clear(0);
    // 12x12 white square bouncing inside an inner field
    let bx: i32 = 8 + triangle(t * 2, 232);
    let by: i32 = 8 + triangle(t, 120);
    host::display::fill_rect(bx, by, 12, 12, 16777215);
    host::display::present();
}
```

If you write Rust, you can read that with zero ramp-up. That is the entire point of
choosing a Rust *subset* over inventing a DSL.

## 2. The language: a deliberate subset

rustlite is Rust-shaped, not Rust-complete. What's in:

- `fn` with typed params and returns; `let` / `let mut`; `const` (order-independent).
- Scalar types `i32`, `f64`, `bool`, and `as` casts between numbers (the common
  graphics pattern: do `f64` math, then `as i32` to a pixel coord).
- `if`/`else`, `loop`, `while`, `for i in 0..n`, `break`/`continue`,
  short-circuit `&&`/`||`, and `match` (with range arms and a trailing catch-all).
- Fixed-size arrays `[i32; N]`: literals, indexed reads *and* writes (`a[i] = v`),
  sized repeat-init `[v; N]`, and array parameters — passed as a base pointer, so a
  callee mutates the caller's backing C-style. (Returning an array is rejected: the
  static-region model would alias two live results, so the compiler refuses it
  rather than miscompile.)

What's deliberately *out* — and errors cleanly as `LH0300` rather than silently
miscompiling — is everything that needs a heap or a runtime: no traits, no generics,
no references, no heap types (`Vec`/`String`/`Box`), no mutable globals. A cartridge
is a few kilobytes of integer math over a framebuffer; the language is scoped to
exactly that. When something isn't supported, you get a coded diagnostic, never a
broken module.

## 3. The pipeline: lex → parse → typecheck → codegen → load

The compiler is a classic four-stage front end and a direct back end, all pure and
unit-testable on native *and* wasm:

```
lexer → parser → typecheck → codegen (wasm emitter) → loader (wasm32 runtime)
```

- **lexer** — byte-level, with string escapes.
- **parser** — hand-written recursive descent with precedence climbing.
- **typecheck** — scope-based type resolution and mutability checking.
- **codegen** — a hand-rolled **wasm binary emitter**: sections, opcodes, LEB128.
  No LLVM, no intermediate toolchain. `compile()` is `lex → parse → typecheck →
  emit` and hands you the module bytes.

Every diagnostic carries a stable `LH0xxx` code (lexer `LH00xx`, parser `LH01xx`,
typecheck `LH02xx`, codegen `LH03xx`) and renders a caret snippet — important
because the *author* is often an agent reading a tool result, not a human staring at
a terminal:

```text
LH0204: type mismatch ... [27..35]
line 2, col 11
  let x = true + 1;
          ^^^^^^^^
```

A coded, located error is something an agent can act on and retry; a byte offset
alone makes it hunt. There's also a compile-*only* check that stubs the host imports,
so validating a cartridge never requires standing up a runtime.

## 4. The host ABI is integer-only — on purpose

A cartridge is a sandbox: it gets linear memory and the host functions we hand it,
and **no DOM**. The functions it can call are grouped into host modules, and the
hard rule is that **only integers cross the wasm boundary**:

- `host::display::*` — `clear`, `set_pixel`, `fill_rect`, `draw_char`,
  `draw_number`, `draw_line`, `fill_triangle`, `present`, plus queries `width` /
  `height` / `pointer_x` / `pointer_y` / `pointer_down`, and a tiny
  `state_get(slot)` / `state_set(slot, v)` for persisting a handful of ints between
  frames.
- `host::net::*` (a poll-model WebSocket), `host::mp::*` (multiplayer),
  `host::chat::*`, `host::http::*`, `host::audio::*`, `host::compose::*`.

"Integer-only" sounds limiting until you see what it buys: there's no way for a
cartridge to hand the host a pointer it can dereference into arbitrary memory, and
the ABI is trivially the same on both sides. When a cartridge genuinely needs text —
say, drawing a string — the answer is **not** a string-passing import. It's a
*compile-time desugar*: `host::display::draw_string(x, y, "HELLO", color, scale)`
lowers at the parser stage to one `draw_char` per glyph (6px stride). The literal is
validated at compile time (printable ASCII, length-bounded) and the integer-only ABI
stays intact. The lesson generalizes: when the data is known at compile time, desugar
it rather than widen the boundary.

A cartridge can also report its resolution with an optional `dims()` export — width
in the high 16 bits, height in the low 16:

```rust
fn dims() -> i32 { (256 * 65536) + 144 }   // 256 x 144, 16:9
```

## 5. Running untrusted bytes safely

A published cartridge is **untrusted** — it can be authored by any agent and fetched
by any visitor — so the runtime is built defensively:

- **Off the main thread.** Cartridges run in a Web Worker. The JS host tables in the
  worker hand-port the Rust loader's import tables, and the two are **parity-tested**
  in CI so a host function can't drift between the native loader and the browser
  worker.
- **A watchdog.** A `frame()` that never returns would freeze a worker; the
  rustlite compiler emits no fuel checks, so the real defense is a main-thread
  watchdog that terminates a worker which stops posting frames. (This was the "brick"
  fix — a runaway cartridge can't lock the tab.)
- **A size gate.** The loader refuses to instantiate anything over a hard 64 KB
  ceiling, independent of the smaller publish-time UI cap — a malicious blob can't
  hand the wasm engine an arbitrarily large buffer.
- **An SSRF gate on networking.** `host::net`'s `open(url)` accepts `wss://` only and
  rejects loopback, LAN, IP-literals (in every notation the URL parser accepts), and
  `.local` names — so a cartridge running in *your* tab can't beacon to hosts inside
  *your* network. That gate is a pure function, native-unit-tested, and hand-ported
  to the worker in lockstep.

The sandbox is the wasm boundary plus a small, explicit set of integer host calls —
which is exactly why an integer-only ABI was worth the constraint.

## 6. Cartridges compose — recursively

The capability that makes this more than a toy: `host::compose` lets a cartridge run
**another subdomain's published cartridge as a child** inside a sub-rectangle of its
own framebuffer. No iframes — the compositor runs the child in its own buffer and
blits it back in. And it's recursive: the child is a full instance with its own
compose table, so it can spawn *its* own children. The canonical demo
(`examples/cartridges/fractal.rl`) spawns *itself*, nesting into a Droste image:

```rust
fn frame(t: i32) {
    host::display::clear(0x000000);
    // ... draw a bordered, animated frame ...
    let spawned: i32 = host::display::state_get(0);     // spawn once, not every frame
    if spawned == 0 {
        host::display::state_set(0, 1);
        let h: i32 = host::compose::spawn_module("fractal", 48, 27, 160, 90);
        host::compose::focus_module(h);
    }
    host::display::present();
}
```

The recursion is real but **bounded** — a compose budget caps depth (5), node count,
and total framebuffer area, so `spawn_module` simply returns `-1` at the cap and no
deeper level mounts. The fractal is finite by construction.

Cartridges can also be **multiplayer**: `host::mp` is an N-peer, host-authoritative
star over WebRTC (up to 8 players), off-chain-signaled. `slither.localharness.xyz` is
a 512×512 multiplayer slither.io written this way; `fractal.localharness.xyz` is the
compose demo above. Both are just URLs — open them.

## 7. Why a whole compiler for this?

Because the author is frequently an *agent*, and that changes the requirements. An
agent that emits a cartridge needs:

1. **A target it can't escape.** The integer-only ABI + wasm sandbox + watchdog mean
   a malformed or malicious cartridge is contained, not catastrophic.
2. **Errors it can act on.** Coded, located diagnostics beat "segfault" — the agent
   reads `LH0204` and the caret, fixes the line, retries.
3. **No toolchain.** The compiler is in the same crate as the agent loop. There's no
   `solc`, no `cc`, no `wasm-pack` to provision in a browser tab. `compile(src)` →
   bytes.
4. **A language it already knows.** A Rust subset means the model writes idiomatic
   Rust, not a bespoke DSL it has to be taught.

The same shape recurs elsewhere in the crate: `SolidityLite` is the EVM analog — a
Solidity/EVM-subset compiler that emits bytecode in-crate (no `solc`), so an agent can
write a contract facet the same way it writes a cartridge. Pure compilers, no external
toolchains, native + wasm.

## Honest scope

- **rustlite is a subset, and stays one.** No heap, no traits, no generics, no
  references. That's a design choice (a tiny, safe, sandboxable target), not a
  roadmap to "full Rust in the browser." If you need that, this isn't it.
- **The runtime defenses are layered, not magic.** The watchdog, the size gate, and
  the SSRF allowlist are the load-bearing controls for running untrusted bytes; the
  fuel counter is only a courtesy for cartridges that voluntarily poll it.
- **Published cartridges are served as static wasm**, no agent backend running per
  request — "24/7, no tab" means the artifact is hosted, not that a process babysits
  it.

## Try it

```rust
use localharness::rustlite;

// Compile a Rust-subset cartridge to wasm bytes — the whole build step.
let wasm: Vec<u8> = rustlite::compile(r#"
    fn frame(t: i32) {
        host::display::clear(0x000000);
        let x = (t / 12) % 240;
        host::display::fill_rect(8 + x, 66, 10, 10, 0xffffff);
        host::display::present();
    }
"#)?;
```

```sh
cargo add localharness        # the SDK + the rustlite compiler
```

- Crate: <https://crates.io/crates/localharness>
- Docs: <https://docs.rs/localharness>
- Source (cartridge corpus in `examples/cartridges/`): <https://github.com/compusophy/localharness>
- Full agent spec (paste it to any agent to onboard it): <https://localharness.xyz/llms.txt>

Apache-2.0. Happy to talk about the wasm emitter, the integer-only host ABI, or the
recursive compositor in the comments.

---

*Disclosure: this article was drafted by an AI agent operated by the localharness
project (the project's own automated account) and reviewed by a human before
publishing. It is AI-generated content and a first-party promotion of localharness.*
