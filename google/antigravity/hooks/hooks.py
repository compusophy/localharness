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

"""Base definitions for Antigravity SDK Hooks v2.

This module defines the interface for Hooks and the standard result types
returned by their lifecycle callbacks.
"""
from __future__ import annotations

import abc
from typing import Any, Awaitable, Callable, Generic, Optional, TypeVar

from google.antigravity import types
from google.antigravity.types import AskQuestionInteractionSpec
from google.antigravity.types import HookResult
from google.antigravity.types import QuestionHookResult


# --- Contexts ---


class HookContext:
  """Base context for hooks to share state."""

  def __init__(self, parent: Optional["HookContext"] = None):
    self.parent = parent
    self._store: dict[str, Any] = {}

  def get(self, key: str, default: Any = None) -> Any:
    """Gets a value from the context or its parents.

    Args:
      key: The key to look up.
      default: The default value to return if the key is not found.

    Returns:
      The value associated with the key, or the default value.
    """
    if key in self._store:
      return self._store[key]
    if self.parent:
      return self.parent.get(key, default)
    return default

  def set(self, key: str, value: Any) -> None:
    """Sets a value in the local context.

    Args:
      key: The key to set.
      value: The value to associate with the key.
    """
    self._store[key] = value


class SessionContext(HookContext):
  """Context scoped to an entire session."""

  def __init__(self):
    super().__init__(parent=None)


class TurnContext(HookContext):
  """Context scoped to a single turn."""

  def __init__(self, session_context: SessionContext):
    super().__init__(parent=session_context)


class OperationContext(HookContext):
  """Context scoped to a specific operation (e.g. tool call)."""

  def __init__(self, turn_context: TurnContext):
    super().__init__(parent=turn_context)


# --- Base Hook Types ---


T = TypeVar("T")
R = TypeVar("R")


class InspectHook(abc.ABC, Generic[T]):
  """Read-only, non-blocking hook for observability."""

  @abc.abstractmethod
  async def run(self, context: HookContext, data: T) -> None:
    """Runs the inspection hook.

    Args:
      context: The hook context.
      data: The data to inspect (read-only).
    """
    pass


class DecideHook(abc.ABC, Generic[T]):
  """Read-only, blocking hook for policy decisions."""

  @abc.abstractmethod
  async def run(self, context: HookContext, data: T) -> HookResult:
    """Runs the decision hook.

    Args:
      context: The hook context.
      data: The data to make a decision on.

    Returns:
      A HookResult indicating allow/deny.
    """
    pass


class TransformHook(abc.ABC, Generic[T, R]):
  """Modifying, blocking hook for data transformation."""

  @abc.abstractmethod
  async def run(self, context: HookContext, data: T) -> R:
    """Runs the transformation hook.

    Args:
      context: The hook context.
      data: The data to transform.

    Returns:
      The transformed data.
    """
    pass


Hook = InspectHook | DecideHook | TransformHook


# --- Concrete Hook Interfaces ---


# Session
class OnSessionStartHook(InspectHook[None]):
  """Invoked when the session starts."""

  pass


class OnSessionEndHook(InspectHook[None]):
  """Invoked when the session ends."""

  pass


# Turn
class PreTurnHook(DecideHook[str]):
  """Invoked before a turn starts.

  The `data` parameter receives the user's prompt string.
  """

  pass


class PostTurnHook(InspectHook[str]):
  """Invoked after a turn ends.

  The `data` parameter receives the model's response text for the completed
  turn.
  """

  pass


# Tool
class PreToolCallDecideHook(DecideHook[types.ToolCall]):
  """Invoked before a tool call to decide if it should proceed.

  The `data` parameter receives the `types.ToolCall` object.
  """

  pass


class PostToolCallHook(InspectHook[Any]):
  """Invoked after a tool call completes.

  The `data` parameter receives the `types.Step` object containing the tool call
  and its results.
  """

  pass


class OnToolErrorHook(TransformHook[Exception, Any]):
  """Invoked when a tool fails, allowing for recovery or modification.

  Receives the raised exception and returns the error representation that
  the model should see. If the hook returns None, the harness uses its
  default error formatting instead.

  The hook cannot fix or retry the tool call on its own, but it can guide
  the agent toward a specific resolution.
  """

  pass


# Interaction
class OnInteractionHook(
    TransformHook[AskQuestionInteractionSpec, QuestionHookResult]
):
  """Hook invoked when the agent needs user interaction.

  This is a superset of QuestionHook and handles all user interactions.
  """

  pass


# Compaction
class OnCompactionHook(InspectHook):
  """Invoked when a context compaction event occurs.

  Compaction is triggered by the harness when the context window exceeds the
  configured token threshold. This hook provides an observability point for
  logging, metrics, or UI notifications.
  """

  pass


# --- Decorators ---


def pre_turn(func: Callable[[str], Awaitable[HookResult]]):
  """Decorator for PreTurnHook.

  Args:
    func: The async function to wrap as a pre-turn hook.

  Returns:
    An instance of PreTurnHook.
  """

  class FunctionPreTurnHook(PreTurnHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> HookResult:
      return await self.f(data)

    async def __call__(self, *args, **kwargs):
      return await self.f(*args, **kwargs)

  return FunctionPreTurnHook(func)


def pre_tool_call_decide(
    func: Callable[[types.ToolCall], Awaitable[HookResult]],
):
  """Decorator for PreToolCallDecideHook.

  Args:
    func: The async function to wrap as a pre-tool-call decision hook.

  Returns:
    An instance of PreToolCallDecideHook.
  """

  class FunctionPreToolCallDecideHook(PreToolCallDecideHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> HookResult:
      return await self.f(data)

    async def __call__(self, *args, **kwargs):
      return await self.f(*args, **kwargs)

  return FunctionPreToolCallDecideHook(func)


def on_interaction(
    func: Callable[[AskQuestionInteractionSpec], Awaitable[QuestionHookResult]],
):
  """Decorator for OnInteractionHook.

  Args:
    func: The async function to wrap as an interaction hook.

  Returns:
    An instance of OnInteractionHook.
  """

  class FunctionOnInteractionHook(OnInteractionHook):

    def __init__(self, f):
      self.f = f

    async def run(
        self, context: HookContext, data: AskQuestionInteractionSpec
    ) -> QuestionHookResult:
      return await self.f(data)

    async def __call__(self, *args, **kwargs):
      return await self.f(*args, **kwargs)

  return FunctionOnInteractionHook(func)


def on_compaction(func: Callable[[Any], Awaitable[None]]):
  """Decorator for OnCompactionHook.

  Args:
    func: The async function to wrap as a compaction hook.

  Returns:
    An instance of OnCompactionHook.
  """

  class FunctionOnCompactionHook(OnCompactionHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> None:
      await self.f(data)

    async def __call__(self, *args, **kwargs):
      await self.f(*args, **kwargs)

  return FunctionOnCompactionHook(func)


def on_session_start(func: Callable[[], Awaitable[None]]):
  """Decorator for OnSessionStartHook.

  Args:
    func: The async function to wrap as a session start hook.

  Returns:
    An instance of OnSessionStartHook.
  """

  class FunctionOnSessionStartHook(OnSessionStartHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> None:
      await self.f()

    async def __call__(self, *args, **kwargs):
      await self.f(*args, **kwargs)

  return FunctionOnSessionStartHook(func)


def on_session_end(func: Callable[[], Awaitable[None]]):
  """Decorator for OnSessionEndHook.

  Args:
    func: The async function to wrap as a session end hook.

  Returns:
    An instance of OnSessionEndHook.
  """

  class FunctionOnSessionEndHook(OnSessionEndHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> None:
      await self.f()

    async def __call__(self, *args, **kwargs):
      await self.f(*args, **kwargs)

  return FunctionOnSessionEndHook(func)


def post_turn(func: Callable[[str], Awaitable[None]]):
  """Decorator for PostTurnHook.

  Args:
    func: The async function to wrap as a post-turn hook.

  Returns:
    An instance of PostTurnHook.
  """

  class FunctionPostTurnHook(PostTurnHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> None:
      await self.f(data)

    async def __call__(self, *args, **kwargs):
      await self.f(*args, **kwargs)

  return FunctionPostTurnHook(func)


def post_tool_call(func: Callable[[Any], Awaitable[None]]):
  """Decorator for PostToolCallHook.

  Args:
    func: The async function to wrap as a post-tool-call hook.

  Returns:
    An instance of PostToolCallHook.
  """

  class FunctionPostToolCallHook(PostToolCallHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> None:
      await self.f(data)

    async def __call__(self, *args, **kwargs):
      await self.f(*args, **kwargs)

  return FunctionPostToolCallHook(func)


def on_tool_error(func: Callable[[Exception], Awaitable[Any]]):
  """Decorator for OnToolErrorHook.

  Args:
    func: The async function to wrap as a tool error hook.

  Returns:
    An instance of OnToolErrorHook.
  """

  class FunctionOnToolErrorHook(OnToolErrorHook):

    def __init__(self, f):
      self.f = f

    async def run(self, context: HookContext, data: Any) -> Any:
      return await self.f(data)

    async def __call__(self, *args, **kwargs):
      return await self.f(*args, **kwargs)

  return FunctionOnToolErrorHook(func)
