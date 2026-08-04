# adapters/terminal-bench/localharness_agent.py — run localharness as the agent
# under test on Terminal-Bench 2.x (the AbstractInstalledAgent pattern: install
# the agent inside the task container, then run it against the instruction).
#
#   uv tool install terminal-bench          # the tb harness (needs Docker)
#   export LOCALHARNESS_KEY=0x…             # a THROWAWAY funded identity's
#                                           # private key (holds/meters real $LH
#                                           # — never a personal key; each model
#                                           # round bills ~1 $LH)
#   tb run --agent-import-path localharness_agent:LocalharnessAgent \
#          --dataset terminal-bench-core --task-id hello-world
#
# The agent surface driven here is `localharness work` — the local coding-agent
# loop (8 fs builtins + run_command, workspace-confined to the container cwd,
# model calls metered through the credit proxy over HTTPS). Honest costs:
# install compiles the crate inside the container (~5-10 min first task on an
# uncached image; a prebuilt musl binary release would remove this), and every
# model round spends real $LH from the key's meter.

import os
from pathlib import Path

from terminal_bench.agents.installed_agents.abstract_installed_agent import (
    AbstractInstalledAgent,
)
from terminal_bench.terminal.models import TerminalCommand


class LocalharnessAgent(AbstractInstalledAgent):
    @staticmethod
    def name() -> str:
        return "localharness"

    @property
    def _env(self) -> dict[str, str]:
        key = os.environ.get("LOCALHARNESS_KEY", "")
        if not key:
            raise RuntimeError(
                "set LOCALHARNESS_KEY to a throwaway funded identity's private key"
            )
        return {"LOCALHARNESS_KEY": key}

    @property
    def _install_agent_script_path(self) -> Path:
        return Path(__file__).parent / "install-localharness.sh"

    def _run_agent_commands(self, task_description: str) -> list[TerminalCommand]:
        escaped = task_description.replace("'", "'\\''")
        # Unlike harbor, THIS adapter sets its own wall clock (max_timeout_sec
        # below), so the matching --deadline-secs is exact: 30s of safety under
        # it, letting the agent land verification before the kill.
        return [
            TerminalCommand(
                command=f"localharness work --as tbench --deadline-secs 1770 '{escaped}'",
                max_timeout_sec=1800.0,
                block=True,
            )
        ]
