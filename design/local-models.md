# local-models — fine-tuned small model vs frontier Flash (telemetry #82)

> **STATUS: strategy + seed dataset shipped.** The question from #82: where does
> a fine-tuned small model (Gemma 3 270M / Kimi-class) actually beat the frontier
> Flash default for THIS platform, and what is the concrete path to one? Answer:
> three seams where small wins on structure, not scale — the intent router, rustlite
> codegen, and latency/cost — plus a distillation pipeline whose seed corpus now
> lives in `datasets/rustlite/` (50 task/solution pairs, 100% compiler-verified).

## 1. Where small beats Flash here

**The intent router (`src/router.rs`).** The free-vs-metered gate runs BEFORE
every chat turn and is currently `HeuristicClassifier` — an exact-allowlist
string match, deliberately conservative because a wrongly-metered message costs
the user $LH and a wrongly-freed one costs them a real answer. The seam is
already a trait (`IntentClassifier`) with the local-Gemma swap named in its doc
comment. Classification of short UI intents is exactly what a 270M model does
well: closed label set, sub-40-char inputs, no generation. A fine-tuned small
classifier widens free-route coverage (paraphrases, typos, "show my balance
pls") without the frontier round-trip — the pre-flight must feel instant, and a
local forward pass beats a network hop to any hosted model at any size.

**rustlite codegen.** rustlite is a closed ~5KLOC-compiler language: i32-first,
integer-only host ABI, no struct literals (LH0300), unit-only enums, read-mostly
arrays, state slots instead of heap persistence. Frontier models fail here in a
characteristic way — they emit real Rust (structs, `Vec`, `String`, closures)
because their prior is real Rust; 5 of 6 telemetry root causes for "dumb agent"
incidents were our own model-facing text diverging from the compiler. A small
model fine-tuned ONLY on compiler-verified pairs has the opposite prior: the
subset is its whole world. The grammar is small enough that 270M-class capacity
is plausibly sufficient, and the compiler is a free, deterministic verifier for
every training sample and every eval — no LLM judging anywhere in the loop.

**Latency + cost + sovereignty.** Every metered turn is 1 $LH through the proxy;
a local model is zero marginal cost and works offline. For the platform's pitch
(self-sovereign browser-resident agents) "the agent still thinks when the proxy
is down" is a product property, not an optimization.

**Where Flash stays.** Open-ended chat, tool orchestration, planning, anything
crossing domains. The small model is a specialist organ behind existing seams,
never the general brain. Model pins stay in `src/types.rs` / `docs_manifest.rs`
(never hand-copied here).

## 2. The seed dataset (`datasets/rustlite/`)

50 task/solution pairs, `NNN.json` = `{id, prompt, solution_rl, tags}`, spanning
the REAL subset: display primitives (clear/fill_rect/draw_line/fill_triangle/
draw_string/draw_number/set_pixel), `dims()` overrides, pointer input +
edge-detection, state slots, arrays (literal/repeat/writes), match (literals,
inclusive + exclusive ranges, wildcard), unit enums, loops/recursion, LCG PRNG
patterns, `host::audio`, complete small games (pong, breakout-lite, whack,
reaction timer), library-style fn modules, and `host::compose`
(spawn_lib/call/call_ok, spawn_module/focus).

Two properties matter more than count:

- **Every solution compiles.** `datasets/rustlite/verify.mjs` extracts each
  `solution_rl`, runs `localharness compile`, and prints a pass table; the gate
  is N/N (currently 50/50, 112B–1641B wasm each). A dataset row that doesn't
  compile is a lie taught at training time.
- **The subset boundary is IN the data.** Prompts state the constraints the
  compiler enforces ("rustlite has no struct literals — model the ball as state
  slots", "enums are unit-only", "locals reset every frame"), so a fine-tune
  learns the negative space, which is precisely where frontier models fail.

## 3. Distillation pipeline

```
seed (50, hand-built, compiler-verified)          datasets/rustlite/NNN.json
  → frontier expansion (Flash/Claude via existing backends generate
    prompt variants + new prompts + candidate solutions)
  → compiler filter (rejection sampling: keep only what
    `localharness compile` accepts; dedupe by wasm hash)   → ~5K pairs
  → fine-tune Gemma 3 270M (LoRA first; training runs OUTSIDE this
    crate — a deliberate, contained exception to Rust-native)
  → eval on the lane-B bench (scripts/bench/run.mjs): deterministic
    scorers — compile rate, export introspection, behavioral i32 calls —
    small-model score vs the Flash baseline on the same tasks
```

The compiler-as-referee is the whole trick: expansion is cheap because
verification is free and exact, and eval reuses `scripts/bench/` (telemetry
#81) unchanged — same tasks, same scorers, two models, one number.

For the router: label a corpus of real inputs (telemetry + fleet transcripts)
with Flash, train the classifier, then run it in SHADOW mode behind
`IntentClassifier` — heuristic keeps deciding, disagreements get logged — until
the wrongly-metered rate is provably ~0.

## 4. Deployment seam (already built)

The `local` cargo feature is the in-browser Gemma 3 270M backend via Burn
wgpu/WebGPU: model module, safetensors loader, tokenizer, greedy generation,
and the `Connection`/`ConnectionStrategy` wiring (`src/backends/local/`). The
full in-tab path — model selector, OPFS weight download, `start_local` — is
already in `browser-app` gated on `local`; the `browser-app-local` composite
turns it on. A fine-tune ships as a weight swap on this path, not new plumbing.

## 5. Honest costs

- **~570MB artifact** (f32 safetensors). Needs quantization (Q8/Q4) before it
  is a sane default download; Burn quant support on wgpu is the open question.
  Off the default bundle by design; that stays true.
- **WebGPU variance.** Known build gotchas are pinned in CLAUDE.md (getrandom
  wasm backend, burn-store DIRECT, async GPU read-back); device/driver variance
  is real and a live in-browser WebGPU run is STILL PENDING — the deployment
  seam is compiled, not yet field-proven.
- **No KV cache** in `generate.rs` (greedy argmax, quadratic re-forward).
  Fine for a one-shot router label; needs the cache before codegen-length
  generations are usable.
- **270M ceiling.** The codegen bet only works because the domain is closed.
  If eval shows the ceiling, the same pipeline retargets a Kimi-class small
  model (still local-first on desktop via CLI, hosted-cheap fallback) — the
  dataset and scorers are model-agnostic.
- **Tool-call templating unbuilt** for the local backend (model-agnostic.md
  Phase D) — router + codegen don't need it; general chat would.
- **Training infra is not in this repo.** Fine-tuning happens in Python land;
  what this repo owns is the dataset contract, the verifier, the eval, and the
  deployment seam.

## 6. Next-step ladder

1. **DONE (this doc's lane):** seed dataset 50/50 compiler-verified +
   `verify.mjs` gate + this strategy.
2. **Expansion run:** frontier generates ~100 variants per seed through the
   existing backends; compile-filter + dedupe; target ~5K pairs under
   `datasets/rustlite/gen/`. Ship the expansion script next to `verify.mjs`.
3. **Baseline first:** score Flash + Claude on the lane-B rustlite tasks
   (`scripts/bench/run.mjs --live`) so the small model has a number to beat
   BEFORE any training spend.
4. **LoRA fine-tune** Gemma 3 270M on the expanded set; eval = compile rate on
   held-out prompts + lane-B score. Go/no-go: small model within ~10% of Flash
   on rustlite tasks at <1% of the cost.
5. **Router shadow mode:** distilled classifier behind `IntentClassifier`,
   logging disagreements against the heuristic on live traffic; promote when
   wrongly-metered ≈ 0.
6. **Ship weights:** quantize, OPFS download path (already wired), live WebGPU
   validation, then default-on router / opt-in codegen under
   `browser-app-local`.
