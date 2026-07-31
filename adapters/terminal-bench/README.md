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
- **Money**: each model round bills ~1 $LH from the key's meter; a hard task
  can take dozens of rounds. Use a throwaway identity funded for the run
  (`localharness create tbench && localharness send … && localharness topup`),
  never a personal key.
- **Install speed**: the install script compiles the crate in-container
  (~5-10 min cold). A prebuilt `x86_64-unknown-linux-musl` release binary
  would replace that block — planned.
- **Scores**: report the dataset + version + task ids + model; `work` uses the
  platform default model unless `--model` is added to the run command.
