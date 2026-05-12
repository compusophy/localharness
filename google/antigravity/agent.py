# Copyright 2026 Google LLC
#
# Licensed under the Apache License, Version 2.0 (the "License");
# you may not use this file except in compliance with the License.
# You may obtain a copy of the License at
#
#     https://www.apache.org/licenses/LICENSE-2.0
#
# Unless required by applicable law or agreed to in writing, software
# distributed under the License is distributed on an "AS IS" BASIS,
# WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
# See the License for the specific language governing permissions and
# limitations under the License.

"""Layer 1 API for Antigravity SDK."""

import asyncio
import contextlib
import logging

from google.antigravity import types
from google.antigravity.connections import connection as connection_module
from google.antigravity.conversation import conversation
from google.antigravity.hooks import cli
from google.antigravity.hooks import hook_runner
from google.antigravity.hooks import hooks
from google.antigravity.hooks import policy
from google.antigravity.mcp import bridge
from google.antigravity.tools import tool_context
from google.antigravity.tools import tool_runner
from google.antigravity.triggers import trigger_runner
from google.antigravity.triggers import triggers as triggers_lib


__all__ = ["Agent"]


class Agent:
  """High-level Agent API for simplified interaction."""

  def __init__(self, config: connection_module.AgentConfig):
    """Initializes the Agent.

    Args:
        config: Declarative agent configuration.
    """
    self._config = config
    if config.response_schema:
      self._config.capabilities.finish_tool_schema_json = config.response_schema
    self._strategy = None
    self._conversation = None
    self._conversation_cm = None
    self._tool_runner = None
    self._hook_runner = None
    self._trigger_runner = None
    self._mcp_bridge = None
    self._pending_hooks = list(config.hooks)
    self._pending_triggers = list(config.triggers)
    self._exit_stack = contextlib.AsyncExitStack()

  def register_hook(self, hook: hooks.Hook):
    """Registers a hook by inferring its type.

    Args:
        hook: The hook to register.
    """
    if not self._hook_runner:
      self._pending_hooks.append(hook)
      return
    self._hook_runner.register_hook(hook)

  def register_trigger(self, trigger: triggers_lib.Trigger):
    """Registers a trigger.

    Cannot be called after the agent has started.

    Args:
      trigger: The trigger function to register.

    Raises:
      RuntimeError: If the agent has already started.
    """
    if self._trigger_runner:
      raise RuntimeError(
          "Cannot register triggers after the agent has started."
      )
    self._pending_triggers.append(trigger)

  async def __aenter__(self) -> "Agent":
    """Starts the agent session.

    Returns:
        The started Agent instance.
    """
    logging.info("Starting Agent session")
    try:
      self._hook_runner = hook_runner.HookRunner()

      # Register pending hooks
      for hook in self._pending_hooks:
        self._hook_runner.register_hook(hook)
      self._pending_hooks.clear()

      # Apply policies
      active_policies = list(self._config.policies)
      cfg = self._config.capabilities
      read_only_tools = set(types.BuiltinTools.read_only())
      # enabled_tools and disabled_tools are mutually exclusive
      # (enforced by CapabilitiesConfig validation).
      if cfg.enabled_tools is not None:
        active_tools = set(cfg.enabled_tools)
      elif cfg.disabled_tools is not None:
        active_tools = set(types.BuiltinTools) - set(cfg.disabled_tools)
      else:
        active_tools = set(types.BuiltinTools)
      has_write_tools = bool(active_tools - read_only_tools)
      has_mcp_servers = bool(self._config.mcp_servers)
      has_tool_decide_hook = bool(self._hook_runner.pre_tool_call_decide_hooks)

      if (
          (has_write_tools or has_mcp_servers)
          and not active_policies
          and not has_tool_decide_hook
      ):
        raise ValueError(
            "Write tools or MCP servers are enabled without a safety policy. "
            "Add policies=[policy.allow_all()] to approve all tool calls, "
            "or policies=[policy.deny_all(), policy.allow('tool_name')] "
            "to selectively allow specific tools."
        )

      if active_policies:
        self._hook_runner.register_hook(policy.enforce(active_policies))

      all_tools = list(self._config.tools)
      # Connect MCP servers
      if self._config.mcp_servers:
        logging.info("Connecting to MCP servers...")
        self._mcp_bridge = bridge.McpBridge()
        self._exit_stack.push_async_callback(self._mcp_bridge.stop)

        for server_cfg in self._config.mcp_servers:
          await self._mcp_bridge.connect(server_cfg)
        all_tools.extend(self._mcp_bridge.tools)

      self._tool_runner = tool_runner.ToolRunner(tools=all_tools)

      self._strategy = self._config.create_strategy(
          tool_runner=self._tool_runner,
          hook_runner=self._hook_runner,
      )

      logging.info("Starting connection and creating conversation...")
      self._conversation = await self._exit_stack.enter_async_context(
          conversation.Conversation.create(self._strategy)
      )

      # Start triggers via TriggerRunner.
      if self._pending_triggers:
        logging.info("Starting triggers...")
        self._trigger_runner = await self._exit_stack.enter_async_context(
            trigger_runner.TriggerRunner(
                triggers=list(self._pending_triggers),
                connection=self._conversation._connection,
            )
        )
        self._pending_triggers.clear()

      # Wire ToolContext into ToolRunner so tools can access
      # conversation capabilities (same pattern as TriggerRunner).
      if self._tool_runner:
        ctx = tool_context.ToolContext(self._conversation._connection)
        self._tool_runner.set_context(ctx)

      return self
    except Exception:
      logging.exception("Failed to start Agent session, cleaning up...")
      await self._exit_stack.aclose()
      raise

  async def __aexit__(self, exc_type, exc_val, exc_tb):
    """Stops the agent session.

    Args:
        exc_type: The exception type, if any.
        exc_val: The exception value, if any.
        exc_tb: The traceback, if any.
    """
    logging.info("Stopping Agent session")
    return await self._exit_stack.__aexit__(exc_type, exc_val, exc_tb)

  async def chat(self, prompt: types.Content) -> types.ChatResponse:
    """Sends a prompt and returns the final response.

    Args:
        prompt: The user prompt or content to send.

    Returns:
        The final response from the agent.
    """
    if not self._conversation:
      raise RuntimeError(
          "Agent session not started. Use 'async with Agent(...)'."
      )
    return await self._conversation.chat(prompt)

  async def run_interactive_loop(self):
    """Runs an interactive CLI loop."""
    if not self._conversation:
      raise RuntimeError(
          "Agent session not started. Use 'async with Agent(...)'."
      )

    assert self._hook_runner is not None
    self._hook_runner.register_hook(cli.AskQuestionHook())
    print("Starting interactive loop. Type 'exit' or 'quit' to end.")
    while True:
      try:
        user_input = await asyncio.to_thread(input, "User: ")
        user_input = user_input.strip()
        if not user_input:
          continue
        if user_input.lower() in ("exit", "quit"):
          print("Goodbye!")
          break

        await self._conversation.send(user_input)

        async for step in self._conversation.receive_steps():
          if step.is_complete_response:
            print(f"Agent: {step.content}")

      except (KeyboardInterrupt, EOFError):
        print("\nGoodbye!")
        break
      except Exception as e:  # pylint: disable=broad-exception-caught
        logging.exception("Error in interactive loop: %s", e)
        print(f"Error: {e}")

  @property
  def connection(self) -> connection_module.Connection:
    """Returns the underlying Connection.

    Intended for advanced use cases that need direct transport access.
    Prefer Agent methods for normal interaction — bypassing the Agent
    and Conversation layers skips history tracking and hook dispatch.

    Raises:
      RuntimeError: If the agent session has not been started.
    """
    if not self._conversation:
      raise RuntimeError(
          "Agent session not started. Use 'async with Agent(...)'."
      )
    return self._conversation._connection

  @property
  def conversation_id(self) -> str | None:
    """Returns the conversation identifier assigned by the runtime.

    Available after the session has started and at least one message has
    been exchanged.  Pass this value back via SessionConfig.conversation_id
    to resume from a saved session.  Returns None before the session starts.
    """
    if not self._conversation:
      return None
    return self._conversation.conversation_id or None

  @property
  def total_usage(self) -> types.UsageMetadata:
    """Cumulative token usage across all turns in this session.

    Raises:
      RuntimeError: If the agent session has not been started.
    """
    if not self._conversation:
      raise RuntimeError(
          "Agent session not started. Use 'async with Agent(...)'."
      )
    return self._conversation.total_usage
