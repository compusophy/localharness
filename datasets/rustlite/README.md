# datasets/rustlite — distillation seed corpus (telemetry #82)

50 task/solution pairs for rustlite codegen fine-tuning. Each `NNN.json` is
`{id, prompt, solution_rl, tags}`; every `solution_rl` compiles (the compiler is
the referee — no aspirational syntax). Strategy + pipeline: `design/local-models.md`.

Coverage: display primitives, `dims()`, pointer input, state slots, arrays,
match/ranges, unit enums, loops/recursion, LCG PRNG, `host::audio`, complete
small games, library-style fn modules, `host::compose` (lib + module halves).
Prompts state the subset boundaries the compiler enforces (no struct literals,
unit-only enums, per-frame locals) so a fine-tune learns the negative space.

Verify (requires `cargo build` first):

```sh
node datasets/rustlite/verify.mjs   # pass table; exits 0 only on N/N
```

## Wave 2 (051-170) + compiler boundaries the corpus encodes

120 pairs added 2026-07-30 (4 themed generators, every solution
compiler-gated): 051-080 simulation/motion, 081-110 interaction/UI widgets,
111-140 complete mini-games, 141-170 audio/libraries/host::compose.

Boundaries PROBED GREEN that the 001-050 style hid (teach them as legal):
`else if` chains, `for i in 0..n` (exclusive only), `break`/`continue`, early
`return` in value-returning fns, unary minus on variables (`0 - v` is style,
not requirement), negative literals in array literals + match arms,
variable-amount shifts, i32-wrapping big literals, if/match as expressions.

Conventions that avoid real traps:
- Always parenthesize bit tests: `((mask >> i) & 1) == 1` (precedence unprobed).
- `idx & 31` on a possibly-negative i32 is the safe sin-table index idiom
  (two's-complement AND is non-negative for power-of-two-minus-one masks).
- LH0202 trap: a fn TAIL expression must not start with `(` right after a
  block statement — `if .. { .. } (v / f(k)) % 10` parses the paren as a CALL
  of the if-block. Bind to a `let` first.
