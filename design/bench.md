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
Ship (a) as a separate small adapter repo/script when prioritized; nothing in
v1 blocks it.

## 3. v1 shipped vs deferred

**Shipped:** `scripts/bench/` — 14 tasks (6 rustlite / 5 bashlite / 3
artifact), offline + live runner with machine scorers, reference solutions
passing 145/145, live smoke green (2 tasks vs a real agent over the metered
call path, 2 $LH). **Deferred:** bounty/validation/attestation wiring (facets
live, glue not written), scheduler-driven score history, TB agent adapter,
corpus growth + anti-overfit (holdout tasks, paraphrased prompts, fixture
randomization — setup_files/expected values are trivially parameterizable),
multi-model comparison tables.
