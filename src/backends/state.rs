//! The per-connection mutable state container shared by the streaming
//! backends (Gemini, Anthropic, OpenAI).
//!
//! Every streaming backend's `LoopState` was byte-identical except the
//! conversation-history element type, so this is ONE generic
//! [`LoopState<M>`]; each backend keeps its `LoopState` name as a thin type
//! alias (`type LoopState = state::LoopState<wire::Content>` etc.) so call
//! sites and struct literals don't churn. This is a NARROW shared piece (the
//! state bag + its step-bookkeeping helpers), not a generic backend.
//!
//! The companion [`history`] codec helpers back each backend's
//! `history_bytes` / `set_history_bytes` / `decode_transcript_bytes`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use parking_lot::Mutex;
use serde_json::Value;
use tokio::sync::{broadcast, Notify};

use crate::types::{Step, StepStatus, StreamChunk, UsageMetadata};

/// Per-connection mutable state, generic over the backend's wire-history
/// message type `M` (`gemini::wire::Content`, `anthropic::wire::Message`,
/// `openai::wire::Message`). Every field is backend-neutral except `history`.
pub(crate) struct LoopState<M> {
    /// The conversation history in the backend's own wire shape.
    pub history: Mutex<Vec<M>>,
    pub idle: Arc<AtomicBool>,
    pub idle_notify: Arc<Notify>,
    /// Set by `cancel_turn` (the UI stop button). The turn loop checks it at
    /// every loop boundary and ends the turn cleanly. Reset at turn start.
    pub cancel: Arc<AtomicBool>,
    pub steps: broadcast::Sender<Step>,
    pub next_step_index: AtomicU32,
    pub last_turn_usage: Mutex<Option<UsageMetadata>>,
    pub last_structured_output: Mutex<Option<Value>>,
}

impl<M> LoopState<M> {
    pub fn new(steps: broadcast::Sender<Step>) -> Self {
        Self {
            history: Mutex::new(Vec::new()),
            idle: Arc::new(AtomicBool::new(true)),
            idle_notify: Arc::new(Notify::new()),
            cancel: Arc::new(AtomicBool::new(false)),
            steps,
            next_step_index: AtomicU32::new(0),
            last_turn_usage: Mutex::new(None),
            last_structured_output: Mutex::new(None),
        }
    }

    pub fn alloc_step_index(&self) -> u32 {
        self.next_step_index.fetch_add(1, Ordering::Relaxed)
    }

    pub fn emit(&self, step: Step) {
        let _ = self.steps.send(step);
    }

    /// Emit a System-sourced turn-FAILURE step (HTTP non-200, stream decode
    /// error, idle stall) which `subscribe_step_stream` translates into a
    /// stream `Err` for `chat()`/`text()`. Hoisted from the per-backend
    /// `fn emit_error` free functions (L24). Gated on `feature = "openai"`
    /// (the only migrated caller today); widen as gemini/anthropic adopt it.
    #[cfg(feature = "openai")]
    pub fn emit_error(&self, message: String) {
        self.emit(Step::turn_error(self.alloc_step_index(), message));
    }

    /// Wrap a [`StreamChunk`] as a [`Step`] so it flows through the same
    /// broadcast. Tool calls AND results surface as `Done` (observability
    /// only) — the call was ALREADY dispatched inline, and the Agent's
    /// `spawn_tool_dispatcher` RE-EXECUTES any non-`Done` registered tool-call
    /// step it sees on the broadcast, so emitting `Active` here would
    /// double-fire every tool (and re-fire on history replay).
    pub fn emit_chunk_step(&self, chunk: StreamChunk) {
        match chunk {
            StreamChunk::ToolCall(tc) => {
                self.emit(Step::tool_call(self.alloc_step_index(), tc, StepStatus::Done))
            }
            StreamChunk::ToolResult(tr) => self.emit(Step::tool_result(self.alloc_step_index(), tr)),
            _ => {}
        }
    }
}

/// Opaque-history JSON codecs shared by the streaming backends' `history_bytes`
/// / `set_history_bytes` / `decode_transcript_bytes`. The on-disk format (a
/// JSON array of the backend's wire messages) is NOT a public API.
pub(crate) mod history {
    use serde::de::DeserializeOwned;
    use serde::Serialize;

    use crate::error::{Error, Result};

    /// Snapshot a wire-history slice as opaque bytes. Round-trips through
    /// [`decode`].
    pub fn encode<M: Serialize>(history: &[M]) -> Result<Vec<u8>> {
        serde_json::to_vec(history).map_err(|e| Error::other(format!("history_bytes: {e}")))
    }

    /// Strict decode for `set_history_bytes`: empty bytes → empty history; a
    /// malformed array is a hard error (the whole restore failed).
    pub fn decode<M: DeserializeOwned>(bytes: &[u8]) -> Result<Vec<M>> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_slice(bytes).map_err(|e| Error::other(format!("set_history_bytes: {e}")))
    }

    /// Per-entry-lenient decode for transcript repaint: parse the array
    /// generically, decode each entry independently, and SKIP the failures so
    /// a single malformed/older-format entry can't blank the WHOLE restored
    /// transcript. Only a top-level "this isn't a JSON array" error is fatal.
    pub fn decode_lenient<M: DeserializeOwned>(bytes: &[u8]) -> Result<Vec<M>> {
        if bytes.is_empty() {
            return Ok(Vec::new());
        }
        let raw: Vec<serde_json::Value> = serde_json::from_slice(bytes)
            .map_err(|e| Error::other(format!("decode_transcript_bytes: {e}")))?;
        Ok(raw
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect())
    }
}

/// Cross-provider transcript-projection contract assertions. Each backend's
/// `project_history` lays out tool calls differently on the wire (Gemini
/// matches a `functionResponse` to its call by NAME in the NEXT user content;
/// Anthropic/OpenAI correlate by tool-use/tool_call ID), but the projected
/// [`crate::types::TranscriptEntry`] shape MUST be identical. These helpers
/// encode that shared contract from ONE place so every backend's transcript
/// test exercises the same invariants (see the per-backend `mod.rs` tests).
#[cfg(test)]
pub(crate) mod transcript_contract {
    use crate::types::{TranscriptEntry, TranscriptRole};

    /// A successful single tool call must surface as exactly ONE assistant
    /// entry whose sole tool call carries `name`, has its `result` correlated
    /// (regardless of wire id/name matching), and has NO error. Returns the
    /// matched result `Value` for any provider-specific follow-up assertion.
    pub fn assert_single_call_result(
        entries: &[TranscriptEntry],
        name: &str,
    ) -> serde_json::Value {
        let asst = entries
            .iter()
            .find(|e| !e.tool_calls.is_empty())
            .expect("an assistant entry with a tool call");
        assert!(
            matches!(asst.role, TranscriptRole::Assistant),
            "tool calls live on the assistant turn"
        );
        assert_eq!(asst.tool_calls.len(), 1, "exactly one tool call projected");
        let call = &asst.tool_calls[0];
        assert_eq!(call.name, name, "tool name preserved");
        assert!(call.error.is_none(), "a success must not set error");
        call.result
            .clone()
            .expect("the result is correlated back to its call")
    }

    /// A FAILED single tool call must surface its failure as the typed `error`
    /// field (the red replay pill), never as a success `result`. Returns the
    /// matched error string.
    pub fn assert_single_call_error(entries: &[TranscriptEntry], name: &str) -> String {
        let asst = entries
            .iter()
            .find(|e| !e.tool_calls.is_empty())
            .expect("an assistant entry with a tool call");
        assert_eq!(asst.tool_calls.len(), 1, "exactly one tool call projected");
        let call = &asst.tool_calls[0];
        assert_eq!(call.name, name, "tool name preserved");
        assert!(
            call.result.is_none(),
            "a failure must surface as error, not result"
        );
        call.error.clone().expect("the failure is the typed error")
    }
}
