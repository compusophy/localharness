# src/soliditylite — Solidity/EVM-subset → bytecode compiler subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/soliditylite/`). The
> EVM analog of `src/rustlite` (~7.5KLOC) — it lets agents write/test/deploy simple
> contracts (and cut their own child diamonds) from the workspace (on-chain feedback
> #25/#26). PURE, no deps, native+wasm. Same `*lite` shape as rustlite — read
> `src/rustlite/CLAUDE.md` for the parallel patterns.

## Pipeline
`lexer → ast → parser → codegen → asm` (EVM bytecode assembler) `→ mod` (compile
pipeline). Plus `interp.rs` — a small EVM interpreter used as the DIFF-HARNESS
oracle (run the compiled bytecode in `interp` and/or diff against `revm` to prove
codegen is correct). E2E proofs live in `examples/soliditylite_*`.

## ⛔ Unsupported subset features must ERROR CLEANLY, never miscompile
soliditylite is a SUBSET. The dangerous failure mode is silently emitting wrong
bytecode for a construct it doesn't really support. Known ones that MUST surface a
clean compile error (TYPE_MISMATCH / UNSUPPORTED), not miscompile: non-literal
(dynamic) storage writes, mappings-to-dynamic, dynamic event args. When you add a
language feature, add the NEGATIVE test that the unsupported shapes still error —
a green positive test alone is not proof.

## Verify codegen by RUNNING bytecode, not just unit tests
A codegen change can pass type/parse tests and still emit wrong opcodes. Run the
`examples/soliditylite_*` E2E proofs (compile → execute in `interp`/revm → assert
the result), and for deploy paths the Tempo CREATE probe (deploy real bytecode and
read it back). Don't ship a codegen change on unit tests alone.

## asm.rs is the bytecode SSOT
Opcode emission + jump-dest resolution live in `asm.rs`. Keep PUSH/jump/label
bookkeeping there (don't hand-emit opcode bytes in codegen) so the assembler stays
the one place that knows offsets — a wrong jumpdest is a silent runtime revert.

## PURE + native+wasm — keep it cfg-clean (no platform deps); it's the substrate
for agents minting their own EVM contracts, so a regression here breaks the
"localharness composes itself" path. design/soliditylite.md.
