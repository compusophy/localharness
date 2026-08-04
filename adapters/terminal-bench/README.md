# Terminal-Bench adapter

Runs **localharness as the agent under test** on [Terminal-Bench 2.x]
(https://www.tbench.ai) via the installed-agent pattern: the task container
gets the CLI installed, then `localharness work` (the local coding-agent loop —
8 fs builtins + `run_command`, workspace-confined, meter-billed) drives the
task instruction.

Interface-validated against the published `terminal-bench` package
(`AbstractInstalledAgent` abstract set: `name`, `_env`,
`_install_agent_script_path`, `_run_agent_commands`) — the adapter
instantiates and emits the correct run command with escaping.

## Run it

```sh
uv tool install terminal-bench     # the tb harness — REQUIRES DOCKER
export LOCALHARNESS_KEY=0x…        # a THROWAWAY funded identity's private key
cd adapters/terminal-bench
tb run --agent-import-path localharness_agent:LocalharnessAgent \
       --dataset-name terminal-bench-core --dataset-version 0.1.1 --task-id hello-world
```

## Honest costs + caveats

- **Docker is required** (every TB task is a container). Not bundled here.
- **Money**: each model round bills from the key's meter — ~1 $LH for the default
  (gemini flash) tier, but **5 $LH for claude-sonnet-5** and 20 $LH for opus (no
  partial spend-down on premium tiers). A hard task can take dozens of rounds, so
  fund a throwaway identity generously (`localharness create tbench && localharness
  send … && localharness topup`) — never a personal key.
- **Install speed**: the CI workflows build a prebuilt static
  `x86_64-unknown-linux-musl` binary (the `binary` job → the rolling
  `tbench-binary` release) that the container curls (~seconds) — an in-container
  cargo compile overran the task timeout. The source-compile fallback stays in
  `install-localharness.sh`.
- **Scores**: report the dataset + version + task ids + model; `work` uses the
  platform default model unless `--model` is added to the run command.
- **Deadline**: harbor never passes the task's timeout to the agent (verified
  against the pinned `harbor==0.20.0` wheel — `trial.py` enforces it externally
  via `asyncio.wait_for`), so `harbor_agent.py` defaults `work --deadline-secs
  870` (30s under harbor's standard 900s); set `LOCALHARNESS_DEADLINE_SECS` for
  runs with a raised timeout (e.g. 3600s tasks). The tb-CLI adapter pins 1770
  under its own 1800s `max_timeout_sec`.

## Terminal-Bench 2.1 (Harbor) + the harness comparison

TB moved to the **Harbor** harness. `harbor_agent.py` is the Harbor adapter
(`--agent-import-path harbor_agent:LocalharnessHarborAgent`), driven by
`.github/workflows/tbench2.yml`; `localharness_agent.py` is the older tb-CLI
(core 0.1.1) adapter, driven by `tbench.yml`.

**"How do we compare to other harnesses?"** `.github/workflows/tbench-compare.yml`
runs a SAME-MODEL head-to-head: our adapter vs harbor's own reference agent
(`terminus-2`), same dataset + same task subset + same underlying model, so the
delta is HARNESS scaffolding, not model capability. Our arm reaches the model
through the credit proxy (`gemini-3.6-flash`); the terminus arm reaches the SAME
Google model via litellm direct (`gemini/gemini-3.6-flash`).

⚠️ The terminus arm needs a **`GEMINI_API_KEY`** GH secret (a Google AI Studio
key; litellm bills it directly — tiny for flash). Without it, that arm self-skips
and only our arm runs. For a claude comparison: `model_lh=claude-sonnet-5`,
`model_terminus=anthropic/claude-sonnet-5`, add `ANTHROPIC_API_KEY`, and land the
proxy 504 fix first (`design/proxy-504-fix.md`). Defaults to flash — cheap and it
dodges the 504, so it runs today once the key is set.
