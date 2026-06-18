# rustlite-problems — a verified coding dataset (the moat)

A growing dataset of **`{problem, reference rustlite solution, tests}`** triples
where every triple is **mechanically verified**: the reference `.rl` actually
**compiles** through the real rustlite compiler, **instantiates** as wasm,
**renders** on a host framebuffer, and **matches** executable pixel/state
assertions.

## Why this is the moat

localharness already owns a real **RLVR verifier** — `scripts/verify.sh` (stages
3–10) and `scripts/test-cartridges.mjs` don't just lint text, they **compile,
instantiate, run, and assert pixels** on rustlite cartridges. Almost every public
"coding dataset" is scraped text with no executable oracle; here the oracle is
free and deterministic.

That verifier turns into a data layer: for any candidate solution we can compute a
**verifiable reward** ("does it pass the gate?") at scale, with no human in the
loop. A teacher model proposes solutions; the verifier filters; only passers are
kept. The resulting corpus is exactly what an own-coding-model wants for
distillation / reinforcement learning from verifiable rewards (RLVR) — and the
verifier that produces it is something **no one else has**.

## The triple format

Each problem is a directory under `problems/<id>/` with three files:

| file            | what                                                              |
|-----------------|-------------------------------------------------------------------|
| `problem.json`  | the spec: `id`, `surface` (ABI/language features exercised), `difficulty`, `statement`, `constraints`. |
| `reference.rl`  | a reference rustlite solution that **passes the gate**.           |
| `tests.json`    | executable checks asserting the rendered framebuffer / host state. |

### `tests.json` check kinds

The harness (`scripts/gen-problems.mjs`) backs `host::display` with a real
`Uint8Array` framebuffer (256×144) and runs `frame(t)` under the
**present-after-frame** model (the same host the corpus uses). Supported checks:

- `pixel_at` `{frame, x, y, expect_rgb}` — render one frame (fresh host), assert a pixel.
- `pixel_after_frames` `{frames:[…], x, y, expect_rgb}` — drive a frame sequence against ONE shared state map, then assert a pixel.
- `state_after_frames` `{frames:[…], slot, expect}` — assert a host state slot after a frame sequence (proves cross-frame persistence).
- `no_trap` `{frames:[…]}` — instantiate + run the frames without trapping.
- `non_blank` `{frames:[…]}` — the framebuffer is not all-black.

Set `"needs_state": true` when the cartridge persists across `frame()` calls.

Colours are `0xRRGGBB`. A common reference trick: pack a computed integer into a
colour channel (e.g. clear to `value`) so a deterministic compute result is
**readable as one pixel** — the same convention `examples/cartridges/*.rl` use.

## The seed set

Seven hand-authored, gate-passing triples covering the core rustlite + host-ABI
surface:

| id                          | exercises                                                   |
|-----------------------------|-------------------------------------------------------------|
| `p01_persistent_counter`    | `state_get/set` across frames, `fill_rect`, `draw_number`   |
| `p02_fill_rect_quadrants`   | `width()/height()`, four-quadrant `fill_rect`               |
| `p03_arithmetic_expr`       | arithmetic + operator precedence in a helper                |
| `p04_clamp_helper`          | `if/else` branches, comparisons, a 3-arg helper             |
| `p05_http_status_probe`     | `host::http` poll model (get/ready/status), state machine   |
| `p06_array_max`             | array literal + indexed reads + array-by-pointer param      |
| `p07_collatz_steps`         | data-dependent `while` loop, even/odd via `%`, recursion-free algorithm |

Re-verify the whole seed set at any time (compiler + node only, no network):

```sh
node scripts/gen-problems.mjs --verify-seeds
```

This runs the exact gate `verify.sh` would: `cargo run --features wallet --bin
localharness -- compile <ref.rl> <out.wasm>`, instantiate the wasm, render, and
assert every check. (The seeds also pass `node scripts/validate-cartridge.js`
and `node scripts/render-cartridge.js` directly — same proofs as verify.sh
stages 4–5.)

## Scaling to ~200 triples

`scripts/gen-problems.mjs` is the generator harness. The pipeline per problem:

1. **Teacher** — ask Claude Opus to emit a candidate `reference.rl` + `tests.json`
   from the problem statement, given the rustlite + host-ABI cheat-sheet and the
   seed triples as few-shot context.
2. **Compile** — through the real rustlite compiler (the verify.sh stage-3 command).
3. **Verify** — instantiate the wasm, render, run the tests.
4. **Keep** — only triples that pass *every* check are written to disk.

```sh
# 1. Write a specs file: a JSON array of {id, statement, constraints, surface, difficulty}.
#    (datasets/rustlite-problems/specs.example.json has two ready-to-run examples.)

# 2. Wire a teacher model (the model call is STUBBED in the worktree — it needs a
#    key/proxy at run time). Pick ONE:
export ANTHROPIC_API_KEY=sk-ant-...           # direct Claude Messages API, OR
#   set LH_PROXY_URL + a signer to route Opus through the $LH credit proxy
#   (proxy/api/gemini.ts multi-provider passthrough), or shell out to
#   `localharness call` for a headless proxy turn.

# 3. Generate — the verifier filters; only passers land on disk:
node scripts/gen-problems.mjs --gen datasets/rustlite-problems/specs.example.json
```

To reach ~200, author ~200 problem statements (broad coverage: arithmetic,
control flow, loops, recursion, arrays + writes, `match`, the draw primitives,
pointer/state machines, `host::http`/`net`/`audio`/`agent`, `host::compose`),
batch them into specs files, and let the teacher+gate loop run. The teacher is
allowed to fail: a dropped candidate costs nothing and a kept one is **provably
correct against the verifier**. That asymmetry is the whole point.

> The model call is deliberately pluggable + stubbed (`callTeacher` in
> `gen-problems.mjs`): the worktree has no API key, so the stub refuses to
> fabricate. Swap in a real client to generate. The `--verify-seeds` path needs
> no model and proves the format + the gate end to end today.
