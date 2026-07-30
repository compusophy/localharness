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
node scripts/bench/run.mjs                    # offline (default): score solutions/, seed 1
node scripts/bench/run.mjs --seed 7           # re-derive fixtures/args/probes/prompts
node scripts/bench/run.mjs --solutions <dir>  # score someone else's answers
node scripts/bench/run.mjs --only rl-clear,bl-hello
node scripts/bench/run.mjs --json             # machine-readable summary (carries the seed)
node scripts/bench/run.mjs --live --as <me> --target <agent>   # answers via `localharness call`
```

Exit 0 iff every scored task passed. Offline mode reads answers from a
solutions dir (reference answers in `solutions/` score 145/145 at ANY seed).
Live mode sends each task's selected prompt phrasing to a real agent
(`localharness call`), extracts the last fenced code block from the reply, and
runs the SAME scorers. Live costs real $LH (~1 $LH/message via the meter);
artifact tasks are skipped in live (they score a workspace, not a chat reply).

## Anti-overfit: seeded parameterization

This repo is open — a "holdout" task dir would be public the moment it landed,
so holdouts are theater here. The real lever is deriving the task INSTANCE at
score time: tasks marked `"seeded": true` get their fixtures, behavioral call
args, and content probes generated from `--seed`, and every expected value is
COMPUTED from the generated material by the runner — nothing memorizable is
stored. A model (or checked-in answer) that memorized one instance's outputs
fails every other seed; only a general program passes all of them.

- **Seed 1 (the default) is the frozen v1 instance** — generators emit the
  original fixture values (through the same compute path) and pick the original
  prompt phrasing, so historical baseline scores stay comparable.
- **Any other seed** re-derives fixtures (bashlite file trees / log lines /
  status words / child scripts), behavioral args (rl-lib-math's add/mul/clamp
  probes), artifact probe rotation (cli-scaffold), and picks one of the task's
  2-3 prompt phrasings.
- **A score without its seed is meaningless** — the runner prints the seed in
  the header, the TOTAL line, and the `--json` summary. Cite it.
- Per-task PRNG streams are derived from `fnv1a(task.id) ^ mix(seed)`, so an
  instance depends only on (seed, id) — stable under `--only` and task order.

Seeded: `bl-count`, `bl-filter`, `bl-branch`, `bl-compose` (fixtures →
computed stdout), `rl-lib-math` (seeded call args → computed i32 expectations),
`cli-scaffold` (probe rotation). The rest are inherently structural or
prompt-pinned: `rl-clear`/`rl-anim`/`rl-counter`/`rl-pointer` check host-call
structure only (nothing behavioral to vary), `rl-dims` checks a value stated in
the prompt (a static source can't adapt to a seeded canvas), `bl-hello` is a
verbatim-echo probe with no fixture a general answer could compute from, and
`cli-jobs`/`cli-plan` probe only prompt-required strings (their references are
static workspaces). Every task still carries prompt paraphrases — the live-mode
lever even where the scorer is structural.

## Task schema (`tasks/*.json`)

```json
{
  "id": "rl-clear",
  "kind": "rustlite | bashlite | artifact",
  "points": 5,
  "seeded": false,
  "prompt": "what the agent is asked (== prompts[0], kept for back-compat)",
  "prompts": ["phrasing 1", "phrasing 2", "phrasing 3"],
  "solution": "path under solutions/ (file, or dir for artifact tasks)",
  "scorer": { ... }
}
```

The seed picks the phrasing (seed 1 → `prompts[0]`). For `"seeded": true`
tasks the seed-varying scorer fields (`setup_files`, `expect_stdout`, `calls`,
rotated `contains`) live in `run.mjs`'s `SEEDED` generator table, NOT in the
JSON — the JSON keeps only the structural parts. The flag and the table are
cross-checked at startup.

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
machine can't score doesn't belong here. Setup files live with the TASK (both
offline and live answers get the same fixtures); the answer is only the
script/source itself. If any fixture value or expected output is an arbitrary
constant, don't hardcode it: mark the task `"seeded": true`, add a generator to
`SEEDED` in `run.mjs` (pin the seed-1 values, COMPUTE the expectations), write
a reference answer that computes rather than memorizes, and prove `--only <id>`
at seeds 1, 7, and 42.

## Current suite

14 tasks / 145 points: 6 rustlite (60), 5 bashlite (45), 3 artifact (40) — 6 of
them seeded.
