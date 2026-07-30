# bench — native verifiable benchmark v1 (telemetry #81)

Every task is scored by a MACHINE: compile checks, wasm export/import
inspection, behavioral wasm calls against a stubbed host, exact bashlite stdout
assertions, artifact presence/content probes. No LLM judging anywhere in the
scoring path — any party re-running the scorer on the same answer gets the same
verdict. That determinism is what makes scores usable as on-chain reward
signals later (see `design/bench.md`).

## Run

```sh
cargo build                                   # scorer shells out to target/debug/localharness
node scripts/bench/run.mjs                    # offline (default): score solutions/
node scripts/bench/run.mjs --solutions <dir>  # score someone else's answers
node scripts/bench/run.mjs --only rl-clear,bl-hello
node scripts/bench/run.mjs --json             # machine-readable summary
node scripts/bench/run.mjs --live --as <me> --target <agent>   # answers via `localharness call`
```

Exit 0 iff every scored task passed. Offline mode reads answers from a
solutions dir (reference answers in `solutions/` score 145/145). Live mode
sends each task's `prompt` to a real agent (`localharness call`), extracts the
last fenced code block from the reply, and runs the SAME scorers. Live costs
real $LH (~1 $LH/message via the meter); artifact tasks are skipped in live
(they score a workspace, not a chat reply).

## Task schema (`tasks/*.json`)

```json
{
  "id": "rl-clear",
  "kind": "rustlite | bashlite | artifact",
  "points": 5,
  "prompt": "what the agent is asked",
  "solution": "path under solutions/ (file, or dir for artifact tasks)",
  "scorer": { ... }
}
```

Scorer params by kind:

- **rustlite** — compiled via `localharness compile <src> out.wasm --host-calls`:
  `required_exports` (checked via `WebAssembly.Module` exports),
  `required_host_calls` / `forbidden_host_calls` (the `--host-calls` listing),
  `calls` (`[{export, args, expect}]` — behavioral: instantiate with all host
  imports stubbed to `() => 0`, call, compare i32), `must_run` (exports that
  must survive 3 calls without trapping), `max_bytes`.
- **bashlite** — run via `localharness sh` in a temp dir (the rooted sandbox);
  `setup_files` ({path: content} placed next to the script), `expect_stdout`
  (exact, CRLF-normalized, trailing-space-trimmed), `expect_stdout_contains`,
  `expect_exit`.
- **artifact** — a workspace directory: `artifacts` (files that must exist),
  `contains` ({file: [substrings]}), `compile` (.rl files that must compile),
  `run` ({script, expect_stdout_contains, expect_exit} — executed in a COPY).

## Adding a task

Drop `tasks/<id>.json` + a reference answer under `solutions/`, then prove
`node scripts/bench/run.mjs --only <id>` passes. Keep scorers exact — a task a
machine can't score doesn't belong here. Setup files live in the TASK (both
offline and live answers get the same fixtures); the answer is only the
script/source itself.

## Current suite

14 tasks / 145 points: 6 rustlite (60), 5 bashlite (45), 3 artifact (40).
