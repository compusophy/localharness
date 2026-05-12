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

"""Hooks package for the Google Antigravity SDK."""

from google.antigravity.hooks import policy
from google.antigravity.hooks.hooks import on_compaction
from google.antigravity.hooks.hooks import on_interaction
from google.antigravity.hooks.hooks import on_session_end
from google.antigravity.hooks.hooks import on_session_start
from google.antigravity.hooks.hooks import on_tool_error
from google.antigravity.hooks.hooks import OnCompactionHook
from google.antigravity.hooks.hooks import OnInteractionHook
from google.antigravity.hooks.hooks import OnSessionEndHook
from google.antigravity.hooks.hooks import OnSessionStartHook
from google.antigravity.hooks.hooks import OnToolErrorHook
from google.antigravity.hooks.hooks import post_tool_call
from google.antigravity.hooks.hooks import post_turn
from google.antigravity.hooks.hooks import PostToolCallHook
from google.antigravity.hooks.hooks import PostTurnHook
from google.antigravity.hooks.hooks import pre_tool_call_decide
from google.antigravity.hooks.hooks import pre_turn
from google.antigravity.hooks.hooks import PreToolCallDecideHook
from google.antigravity.hooks.hooks import PreTurnHook

__all__ = [
    "policy",
    "OnCompactionHook",
    "OnInteractionHook",
    "OnSessionEndHook",
    "OnSessionStartHook",
    "OnToolErrorHook",
    "PostToolCallHook",
    "PostTurnHook",
    "PreToolCallDecideHook",
    "PreTurnHook",
    "on_compaction",
    "on_interaction",
    "on_session_end",
    "on_session_start",
    "on_tool_error",
    "post_tool_call",
    "post_turn",
    "pre_tool_call_decide",
    "pre_turn",
]
