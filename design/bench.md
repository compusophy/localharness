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
RUN ON TB 2.1 (2026-07-31): Terminal-Bench 2.x moved to the HARBOR harness
(`harbor run -d terminal-bench/terminal-bench-2-1`), so a second adapter —
`adapters/terminal-bench/harbor_agent.py` (Harbor `BaseInstalledAgent`) — and
`.github/workflows/tbench2.yml` drive `localharness work` against the current
benchmark. Proven end-to-end: on the old `tb` core-0.1.1 set, hello-world
resolved 1/1; on TB 2.1, the agent genuinely attempts hard tasks (on
`write-compressor` it explored the env, probed toolchains, and wrote/compiled
multiple C arithmetic-coder implementations with verification passes) —
first 2-task sample scored 0/2, honest for single-shot gemini-3.6-flash on
hard tasks. A leaderboard-relevant number needs a larger sample + a stronger
driving model; the infra is a one-line dispatch (`gh workflow run tbench2.yml
-f n_tasks=N`).

HEAD-TO-HEAD vs terminus-2 (`tbench-compare.yml`, same model both arms —
2026-08-01 progression, first-5-task subset, gemini-3.6-flash): 02:33 run
0/3 (16-round-cap guillotine) → 06:10 run, after the auto-continue fix, 3/3
resolved but 2 infra-errored → 07:15 run 1 resolved, 4 infra-errored.
terminus-2: 5/5 in all three. EVERY loss was OUR INFRA, not the agent: 3×
proxy edge 504 (FUNCTION_INVOCATION_TIMEOUT — ~25s first-byte cap × big
contexts) + 1× auth-token 401 ("stale or future timestamp" — one token
signed at startup vs the proxy's 5-min freshness window, hours into honest
work). Both fixed 2026-08-01: gemini.ts ported to the Node runtime
(design/proxy-504-fix.md — export shape + ESM `.js` imports were the real
config-flip killers) and work/call re-sign the token per request; work's 48K
compaction stopgap restored to the shared 128K. The transcripts show real
capability (write-compressor: multiple compiling C arithmetic coders with
verification passes before the 401 killed it).

**POST-FIX RERUN (run 30695536254, 2026-08-01 10:20 UTC): PARITY — 5/5 vs
5/5, zero infra errors, Δ = 0.0.** Same subset, same model. Every task that
ever died on our side resolved, including the deep ones (schemelike, torch).
Step efficiency (our tool calls vs terminus-2 trajectory steps): kv-store
24/13, pypi 21/11, schemelike 51/96, torch 37/32, write-compressor 19/32 —
totals **152 vs 184** (~17% fewer): terminus is leaner on shallow tasks, we
are markedly leaner on the deep ones. "Beat it" candidates, in order of
signal: (a) a wider subset (n_tasks 15-20 — 5 tasks can't separate equal
harnesses), (b) shallow-task round overhead (our recon rounds on easy
tasks), (c) pass@1 variance replication. Every arm of the 3-run progression
above + this one used the SAME first-5 subset — directly comparable.

15-TASK RUN (30697327161, 2026-08-01): localharness **9/14 (64.3%)** + 1
infra-err vs terminus-2 **11/13 (84.6%)** + 2 err — Δ −20.3. We uniquely
solved dna-assembly + path-tracing; terminus uniquely solved
model-extraction-relu-logits / schemelike / torch-tensor / qemu-alpine-ssh
(our err); both failed regex-chess. ⚠️ schemelike + torch-tensor PASSED at
n=5 and FAILED here (same binary) — pass@1 variance on deep tasks is large;
the 5/5 parity row carried luck. Per-task diagnosis + fix plan: the
beat-78% campaign (target = Google's claimed 78.0% for gemini-3.6-flash,
harness unspecified; best VERIFIED Gemini leaderboard row = Terminus2 +
3-Pro at 73.9%, best flash row = Gemini CLI + 3-Flash 56.9%; Antigravity
itself has NO published TB scores — harbor can run antigravity-sdk/-cli as
an arm_c for a direct comparison).

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
| 2026-08-14 | gemini-3.7-flash | 1 | **75/105** | 8/11 | the pin flip (`567b84ce`, 2026-08-13) shipped with NO measured row, which the hill-climb rule below forbids — this run is that debt paid, after the fact. Score-NEUTRAL: identical points, identical task count, and the *same three* failures as the 3.6 row (`bl-count`, `bl-filter`, `rl-counter`), so the pin neither helped nor regressed on this set |

That the two rows agree to the point — same score, same three failures — is
itself the finding: these tasks fail on **platform-ABI negative space**, not on
model capability, so a Flash-generation change moves nothing here. Expect this
set to stay flat across frontier pins until `datasets/rustlite/` (or a
fine-tune) teaches the ABI; a pin change that DOES move these numbers is the
surprising result worth investigating.

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

### Full-set baseline failure ledger (run 30706586702)

Full TB 2.1 set, `gemini-3.6-flash`, 3-way concurrency, run tree `43124396`
(2026-08-01 15:47:40 → 17:51:39 UTC). Artifact `tb-compare-runs` carries the
**localharness arm only** — 89 trial dirs, no terminus-2 arm, so this run has
no head-to-head Δ. Every number below is read from
`localharness/jobs/2026-08-01__15-47-35/`.

**Headline: mean reward `0.16853932584269662` = 15/89 resolved (16.9%).
That is NOT a capability number.** Harbor counted **71 errored trials**, and
**66 of them were one Google-key quota storm**. Only **18 trials reached a
genuine verdict — and 15 of those 18 resolved (83.3%)**.

| # | class | n | example task(s) | root cause | verdict |
|---|-------|---|-----------------|------------|---------|
| 1 | `ApiRateLimitError` — monthly SPEND CAP | **64** | `adaptive-rejection-sampler`, `portfolio-optimization` | AI Studio project hit its monthly spend cap at 17:23:41Z (`sam-cell-seg`, 18 rounds in); every later trial died — **63 with ZERO tool rounds** | **STILL-OPEN (operational)** — ⛔ 0ef569c1's ladder does NOT cover a spend cap (not transient). Fail-fast shipped 2026-08-04 (`work.rs rate_limit_is_permanent`); the cap itself is a billing decision |
| 2 | `ApiRateLimitError` — per-minute TPM | 2 | `caffe-cifar-10`, `mcmc-sampling-stan` | `…InputTokensPerModelPerMinute-PaidTier2`, quota 3M, `retryDelay 4s`, tripped mid-work by 3-way concurrency | **COVERED** — 0ef569c1 (15/30/45/60s ladder, max 6, same-turn retry). Not in the run tree |
| 3 | `NonZeroAgentExitCodeError` — PROHIBITED_CONTENT | 1 | `dna-assembly` | `gemini sse decode: missing field `role`` on a blocked candidate wired as `"content": {}`; a content filter read as an infra crash | **COVERED** — ad931b47 (`role` defaults to `model`; `classify_empty → Blocked` ends the run named). Not in the run tree |
| 4 | `AgentTimeoutError` | 4 | `qemu-alpine-ssh`, `mteb-leaderboard` (3600s) | not a crash — all four were still issuing productive `run_command` rounds at harbor's wall clock; the agent had no idea a clock existed | **MITIGATED, UNPROVEN IN A RUN** — `work --deadline-secs` (2026-08-04): first turn states the budget, continuation nudges scale to Closing/Final phases (`work.rs deadline_phase`); the harbor adapter defaults 870s (harbor 0.20.0 never passes the task timeout to the agent — verified against the wheel; `LOCALHARNESS_DEADLINE_SECS` overrides for 3600s tasks). Next full-set run judges it |
| 5 | Unresolved — truncated text-tail | 2 | `schemelike-metacircular-eval`, `torch-tensor-parallelism` | after ONE auto-continue each the turn ended in a reasoning dump cut mid-sentence and the run STOPPED (1 of a 12-continuation budget), work incomplete | **COVERED** — 08be588d (continue-decision reads `reply.finished()`; `saw_finish` was dead code; + truncation nudges + 65K output cap). Not in the run tree |
| 6 | Unresolved — agent believed it was done | 1 | `filter-js-from-html` | 46 rounds, clean completion summary, verifier still 0.0; never re-derived the grader's harder cases | **STILL-OPEN (capability)** |
| 7 | Non-fatal tool friction | 12 | `filter-js-from-html` (`/app/filter.py`), `regex-log` | 10× `create_file refuses to overwrite existing file` + 2× `denied by policy 'workspace_only:*'`; each burns a round and pushes the model onto `run_command` heredocs | **STILL-OPEN (minor)** — ~1.4% of the run's 856 tool rounds |
| — | proxy 504 / auth 401 | **0** | — | grepped `401`, `504`, `stale or future`, `FUNCTION_INVOCATION_TIMEOUT` across all 89 logs | **COVERED, HELD UNDER LOAD** — d5845585 + 239ce2fc are ancestors of `43124396` |

Reading for the 78% chase: three classes are fixed on main but were absent from
this run's binary (2, 3, 5), one is money not code (1 — fail-fast now shipped,
the cap itself is billing), and the genuinely open agent-side work is round
efficiency (4 — deadline awareness shipped 2026-08-04, unproven in a run),
self-verification before declaring done (6), and fs-builtin
friction (7). **The next full-set run is the first one that can produce an
honest baseline** — this one measured a billing event.
