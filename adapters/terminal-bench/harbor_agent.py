# adapters/terminal-bench/harbor_agent.py — localharness as the agent under
# test on Terminal-Bench 2.x, which moved to the HARBOR harness.
#
#   pip install harbor            # (needs Docker)
#   export LOCALHARNESS_KEY=0x…   # a THROWAWAY funded identity's private key
#   harbor run -d terminal-bench/terminal-bench-2-1 \
#              --agent-import-path harbor_agent:LocalharnessHarborAgent \
#              --task-id hello-world -k 1
#
# Harbor's BaseInstalledAgent contract (introspected from harbor 0.20.0):
# abstract {name, install, run}. `install`/`run` are async and drive the task
# container via self.exec_as_root / self.exec_as_agent. Unlike the old
# `terminal_bench` adapter (localharness_agent.py), there is no install-script
# path or _run_agent_commands — the bodies run commands directly.
#
# The driven surface is the SAME `localharness work` local coding-agent loop;
# model calls meter real $LH through the credit proxy, so use a throwaway
# funded key.
import os
import shlex

from harbor.agents.installed.base import BaseInstalledAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

_BINARY_URL = (
    "https://github.com/compusophy/localharness/releases/download/"
    "tbench-binary/localharness-x86_64-linux"
)


class LocalharnessHarborAgent(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "localharness"

    def get_version_command(self) -> str | None:
        return "localharness version"

    async def install(self, environment: BaseEnvironment) -> None:
        key = os.environ.get("LOCALHARNESS_KEY", "")
        if not key:
            raise ValueError(
                "set LOCALHARNESS_KEY to a throwaway funded identity's private key"
            )
        # Bare task images may lack curl/ca-certificates.
        await self.exec_as_root(
            environment,
            command="apt-get update && apt-get install -y curl ca-certificates",
            env={"DEBIAN_FRONTEND": "noninteractive"},
        )
        # FAST PATH: download the prebuilt static x86_64 binary (a cargo compile
        # overruns the task's agent timeout — see the CI `binary` job).
        await self.exec_as_root(
            environment,
            command=(
                "set -euo pipefail; "
                f"curl -fsSL {shlex.quote(_BINARY_URL)} -o /usr/local/bin/localharness; "
                "chmod +x /usr/local/bin/localharness; "
                "localharness version"
            ),
        )
        # Provision the identity `work` loads (~/.localharness/keys/<name>.key).
        await self.exec_as_agent(
            environment,
            command=(
                "set -euo pipefail; "
                'mkdir -p "$HOME/.localharness/keys"; '
                'printf "%s\\n" "$LOCALHARNESS_KEY" '
                '> "$HOME/.localharness/keys/tbench.localharness.key"; '
                'chmod 600 "$HOME/.localharness/keys/tbench.localharness.key"'
            ),
            env={"LOCALHARNESS_KEY": key},
        )

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        escaped = shlex.quote(instruction)
        # Harbor's `-m/--model` arrives as self.model_name. localharness routes
        # by id prefix through the credit proxy (claude-* → Anthropic backend,
        # else Gemini), so pass it straight to `work --model`; no model = the
        # platform default (gemini-3.6-flash).
        model = (self.model_name or "").strip()
        model_flag = f"--model {shlex.quote(model)} " if model else ""
        await self.exec_as_agent(
            environment,
            command=(
                f"localharness work --as tbench {model_flag}{escaped} "
                "2>&1 | stdbuf -oL tee /logs/agent/localharness.txt"
            ),
        )
