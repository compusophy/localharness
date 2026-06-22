# src/bashlite — sandboxed shell (localharnesslite) subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/bashlite/`). A tiny
> sandboxed shell: `lexer → token → parser → ast → eval` over a `BashHost`. Two
> entry points: CLI `localharness sh <.bl>` and browser `execute_script`. PURE-ish,
> native+wasm. Full design: `design/bashlite.md`.

## What it does
fs builtins (`builtins.rs`) + `run`/`source` script COMPOSITION (fractal — a script
runs a script) + `&&`/`||` + `for f in $( … )` field-split fan-out + `lh-*` platform
reads/writes (`platform.rs`, `feature=wallet`). The point is composable
platform-scripting: glue lh-* reads/writes together in a sandbox.

## ⛔ Two safety invariants — do NOT weaken either
1. **FUEL-BOUNDED.** `run`/`source` recurse (script-runs-script, fractal), so eval is
   capped by a FUEL budget — that's the only thing stopping a runaway/​recursive
   script from hanging the CLI or the browser tab. Keep every loop/compose path
   debiting fuel; never add an unbounded execution path.
2. **lh-* WRITES go behind the DRY-RUN MANIFEST CONFIRM GATE.** Value-moving /
   on-chain `lh-*` ops (`platform.rs`) are collected into a dry-run manifest and
   CONFIRMED before execution — a script can't silently spend/transfer. New lh-*
   write builtins MUST route through the gate, not execute directly.

## Sandbox = RootedFilesystem
fs ops are confined to a sub-tree via `RootedFilesystem` (see
`src/filesystem/CLAUDE.md`) — a bashlite script can't escape its root. Don't hand a
bashlite host a bare Native/OPFS FS.

## Extension seam: `BashHost::run_builtin`
Add platform/host commands via the `BashHost` trait's `run_builtin` hook (the wired
extension point), not by forking the evaluator. Everything is `?Send` (wasm) — mirror
that on new async surfaces or it breaks the wasm build silently.

## Verify with the live scripts, not just unit tests
`fractal.bl` composes 3 levels + an `lh-resolve` — run the example end-to-end after
an eval/compose change (the fuel + field-split + recursion interact). 47 tests cover
the cores, but the composition behavior is best proven by running a real `.bl`.
