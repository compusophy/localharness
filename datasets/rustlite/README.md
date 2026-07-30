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
