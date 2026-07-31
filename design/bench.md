# bench — dual benchmarking (telemetry #81)

Origin: feedback from `frank` — benchmark localharness two ways at once:
(1) a NATIVE, machine-verifiable suite whose scores can become on-chain reward
signals for autonomous hill-climbing, and (2) an adapter onto an established
external benchmark (TerminalBench) for outside credibility. Both, not either.

## 1. Native verifiable suite (v1 SHIPPED — `scripts/bench/`)

**Principle: no LLM judging.** Every scorer is deterministic and re-runnable by
any third party: compile the answer, inspect the wasm (exports / host-call
surface), CALL exported functions against a stubbed host and compare i32s, run
the bashlite script in the rooted sandbox and diff stdout, probe artifacts.
Same answer → same verdict, on anyone's machine. That property — not the task
content — is what makes the suite usable as a REWARD signal.

Three task kinds, one runner (`run.mjs`, offline default / `--live` via
`localharness call`), 14 tasks / 145 points, reference solutions score 100%
(proven by running; a corrupted-answer negative control scores 0).

- **rustlite codegen** — prompt → cartridge source; scored by
  `localharness compile … --host-calls` + `WebAssembly.Module` introspection +
  behavioral calls (e.g. `clamp(-3,0,5) == 0`, `dims() == (256<<16)|128`).
- **bashlite scripting** — prompt → `.bl` script; scored by
  `localharness sh` over task-supplied fixture files + exact stdout.
- **CLI/agentic** — a produced WORKSPACE; scored by artifact presence, content
  probes, compile checks, and executing the produced scripts.

### Anti-overfit: seeded parameterization (SHIPPED)

An open repo cannot have a holdout set — a "holdout" dir is public the moment
it lands, and fixed fixtures + fixed expected values are a memorization target
(a model could learn to emit `3 cartridges` instead of counting). The honest
lever is deriving the task INSTANCE at score time: `run.mjs --seed <n>`
(default 1) drives a per-task PRNG (mulberry32 over `fnv1a(id) ^ mix(seed)`);
tasks marked `"seeded": true` generate their fixtures / behavioral call args /
content probes from it, and the runner COMPUTES every expected value from the
generated material — expectations are never stored. Each task also carries 2-3
prompt paraphrases; the seed picks the phrasing sent in `--live`.

Seed 1 is pinned to the frozen v1 instance (original fixtures, original
phrasing), so pre-seed scores read as seed-1 scores unchanged. Reference
solutions are GENERAL programs (count/filter/compute, never echo a memorized
answer) and pass at any seed; a memorizing answer passes seed 1 and fails
everywhere else — proven by the negative control. Seeded: bl-count, bl-filter,
bl-branch, bl-compose, rl-lib-math, cli-scaffold. Structurally-scored or
prompt-pinned tasks (rl-clear/anim/counter/pointer, rl-dims, bl-hello,
cli-jobs, cli-plan) stay fixed — their scored surface has no arbitrary
constants a seed could vary without breaking the "reference passes at any
seed" invariant. **A score must always cite its seed** (the runner prints it
in the header, TOTAL line, and `--json`).

### The on-chain reward loop (wiring plan — facets already exist)

The scorer verdict is the oracle; the chain holds the money and the record.
Nothing below needs a new facet:

1. **$LH bounty per task/suite** — `BountyFacet` (rung 1, proven E2E):
   `postBounty` escrows the reward with the task id + scorer commit hash in the
   spec; a worker `claimBounty` + `submit_result` with the answer; the poster's
   agent re-runs `run.mjs --solutions <answer>` and `acceptResult` iff exit 0 —
   payout to the worker's TBA. Accept is automatable precisely because the
   scorer is deterministic.
2. **ERC-8004 validation staking** — `ValidationFacet` stake/challenge/resolve
   on a workRef = `keccak(task_id ‖ answer_hash ‖ score)`. A validator stakes on
   a claimed score; anyone can challenge by re-running the scorer; resolve pays
   the honest side. Disputes are decidable, not social.
3. **Attestations** — `ReputationFacet.attest(subject, 1..5, workRef)` after a
   verified run (per-work dedup already on-chain). Accumulated attestations =
   the public skill curve an agent hill-climbs.
4. **Receipts** — `receipt.rs` binds source → wasm hash → call results; a
   benchmark run can ship a receipts line so the artifact chain is auditable.
5. **The climb loop** — the off-chain scheduler (or a plain cron) re-runs the
   suite per agent/model; `--json` output is the time series. Live mode already
   pays the meter (1 $LH/message), so benchmarking IS platform usage.

## 2. TerminalBench adapter (PLAN — deferred)

TerminalBench tasks are (container, instruction, verification script) triples
driven through a real POSIX terminal. Two possible adapter shapes:

- **(a) Agent adapter (the credible path).** TerminalBench's agent side is
  pluggable: implement their agent interface as a thin shim that forwards the
  task instruction to a localharness agent (`localharness call` / the ACP
  server) and relays the agent's proposed shell commands back into the TB
  terminal session. localharness is the AGENT under test; the terminal is
  theirs. This measures the loop + model honestly and needs no lying about our
  sandbox.
- **(b) Task import (mostly impossible).** Running TB tasks INSIDE localharness
  fails honestly: bashlite is a deliberately tiny sandbox (echo/test/ls/cat/
  wc/grep/head/tail/find/mkdir/write + run/source + lh-*), not a POSIX shell —
  no arbitrary binaries, no processes, no network. Only a trivial fs/text
  subset of TB would import, and cherry-picking it would overstate coverage.

What maps to "a terminal" today: bashlite (sandboxed scripting w/ fs + platform
commands) + the ~40-command CLI. Honest gaps for (a): TB shims are Python +
Docker (external harness dep), interactive multi-step terminal sessions need
streaming keystroke relay (our `call` is request/response — the ACP surface is
the better fit), and TB scoring assumes container-side state we don't control.
SHIPPED 2026-07-31: `adapters/terminal-bench/` — an AbstractInstalledAgent
adapter (interface-validated against the published package) driving the new
`localharness work` local coding-agent loop (native tools + run_command,
workspace-confined, meter-billed). Remaining to an actual scored run: Docker
on the host + a funded throwaway identity; runbook in the adapter README.

## 3. v1 shipped vs deferred

**Shipped:** `scripts/bench/` — 14 tasks (6 rustlite / 5 bashlite / 3
artifact), offline + live runner with machine scorers, reference solutions
passing 145/145 at seeds 1/7/42, live smoke green (2 tasks vs a real agent over
the metered call path, 2 $LH), seeded parameterization + prompt paraphrases
(the anti-overfit section above; holdout dirs rejected as theater in an open
repo). **Deferred:** bounty/validation/attestation wiring (facets live, glue
not written), scheduler-driven score history, TB agent adapter, corpus growth,
multi-model comparison tables, multi-seed sweep reporting (one `--json` run per
seed already works; an aggregator does not exist yet).

## 4. Baselines (live runs, `--live --as claude --target claude`)

| date | model (agent default) | seed | live-scoreable | tasks | notes |
|------|----------------------|------|----------------|-------|-------|
| 2026-07-30 | gemini-3.6-flash | 1 | **75/105** | 8/11 | artifact tasks (40 pts) are chat-unscoreable by design in `--live`; run predates `--seed` — seed 1 IS the frozen v1 instance, so it reads as seed 1 |

Failures — all platform-ABI negative space (the exact gap
`datasets/rustlite/` + a fine-tune target):
- `bl-count`, `bl-filter`: bashlite command-substitution misuse — the script
  printed the label with an EMPTY count (`" cartridges"`, `"errors:"`); the
  `n=$(... | wc -l)` pipeline shape wasn't produced correctly.
- `rl-counter`: called `host::display::draw_number(x, y, value)` with 3 args —
  the host fn takes 5 (`x, y, value, rgb, scale`). Compile-rejected LH0203.

Hill-climb rule: a candidate model (fine-tuned local, new frontier pin) must
beat the current row on the SAME task set AND the same seed before a pin
change; add rows here, never overwrite. Seed-1 rows are the comparable series;
an anti-overfit spot-check runs 2-3 extra seeds and a seed-robust score should
not collapse vs seed 1.
