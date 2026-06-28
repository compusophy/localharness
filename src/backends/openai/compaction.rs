//! OpenAI adapter for the shared compaction fold engine
//! (`crate::backends::compaction`).
//!
//! ALL algorithm logic — the fold plan, keep-window split, prior-summary
//! recognition, prompt build, install, drop-oldest fallback, thresholds —
//! lives in the engine (see its module doc for the full strategy write-up).
//! This module supplies only the OpenAI-specific bits: the wire-message seam
//! (`CompactionModel` over `Message`) and the one-shot Chat-Completions
//! summarization request.

use parking_lot::Mutex;

use crate::backends::compaction::{self as engine, CompactionModel};
#[allow(unused_imports)] // COMPACTION_TAG: used only in cfg(test); should_compact in non-test code
pub use crate::backends::compaction::{should_compact, COMPACTION_TAG};
use crate::backends::openai::api::SharedClient;
use crate::backends::openai::wire::{ChatRequest, Message, Role};
use crate::error::Result;

/// The OpenAI side of the [`CompactionModel`] seam — a zero-sized marker the
/// engine is monomorphized over.
struct OpenAiCompaction;

impl CompactionModel for OpenAiCompaction {
    type Message = Message;

    fn is_user(m: &Message) -> bool {
        matches!(m.role, Role::User)
    }

    fn sole_text(m: &Message) -> Option<&str> {
        // A "sole text" message is one whose only payload is its content
        // string and no tool calls (the synthetic summary turn's shape).
        if m.tool_calls.is_empty() && m.tool_call_id.is_none() {
            m.content.as_deref()
        } else {
            None
        }
    }

    fn is_tool_result_turn(m: &Message) -> bool {
        // OpenAI tool results are individual `role:"tool"` messages.
        matches!(m.role, Role::Tool)
    }

    fn user_text(text: String) -> Message {
        Message::user_text(text)
    }

    fn render_message(entry: &Message, out: &mut String) {
        let role = match entry.role {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL",
        };
        out.push_str("## ");
        out.push_str(role);
        out.push('\n');
        if let Some(text) = &entry.content {
            match entry.role {
                Role::Tool => {
                    out.push_str("[tool_result] ");
                    engine::push_truncated(out, text);
                }
                _ => out.push_str(text),
            }
            out.push('\n');
        }
        for call in &entry.tool_calls {
            out.push_str("[tool_call ");
            out.push_str(&call.function.name);
            out.push_str("] ");
            out.push_str(&call.function.arguments);
            out.push('\n');
        }
    }
}

/// Try to compact `history` in place. Returns `true` if anything changed.
/// Safe to call from inside the agent loop — never errors, only logs.
pub async fn try_compact(history: &Mutex<Vec<Message>>, client: &SharedClient, model: &str) -> bool {
    engine::try_compact::<OpenAiCompaction, _, _>(history, |prompt| summarize(client, model, prompt))
        .await
}

/// One-shot summarization request: feed the fold prompt as a single user
/// message — no system, no tools, no history.
async fn summarize(client: &SharedClient, model: &str, prompt: String) -> Result<String> {
    let req = ChatRequest {
        model: model.to_string(),
        messages: vec![Message::user_text(prompt)],
        tools: Vec::new(),
        tool_choice: None,
        stream: false,
        stream_options: None,
        temperature: None,
        max_completion_tokens: None,
    };
    let resp = client.chat(&req).await?;
    Ok(resp.text())
}

// ---- test-only thin wrappers ----
//
// Exercise the SHARED engine through this adapter's seam, mirroring the
// Anthropic/Gemini adapters' pinned suites.

#[cfg(test)]
use crate::backends::compaction::KEEP_RECENT_TURNS;

#[cfg(test)]
fn pick_split(history: &[Message], keep_pairs: usize) -> usize {
    engine::pick_split::<OpenAiCompaction>(history, keep_pairs)
}

#[cfg(test)]
fn plan_fold(history: &[Message]) -> Option<engine::FoldPlan> {
    engine::plan_fold::<OpenAiCompaction>(history)
}

#[cfg(test)]
fn extract_prior_summary(head: Option<&Message>) -> Option<String> {
    engine::extract_prior_summary::<OpenAiCompaction>(head)
}

#[cfg(test)]
fn render_transcript(history: &[Message]) -> String {
    engine::render_transcript::<OpenAiCompaction>(history)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::openai::wire::{FunctionCall, ToolCall};

    fn user_text(s: &str) -> Message {
        Message::user_text(s)
    }
    fn assistant_text(s: &str) -> Message {
        Message::assistant_text(s)
    }
    fn assistant_call(id: &str, name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: None,
            tool_calls: vec![ToolCall {
                id: id.into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: name.into(),
                    arguments: "{}".into(),
                },
            }],
            tool_call_id: None,
        }
    }
    fn tool_result(id: &str) -> Message {
        Message::tool_result(id, r#"{"ok":true}"#)
    }
    fn summary_turn(s: &str) -> Message {
        Message::user_text(format!("{COMPACTION_TAG}\n{s}"))
    }

    #[test]
    fn pick_split_respects_keep_window() {
        let h: Vec<Message> = (0..10)
            .flat_map(|i| vec![user_text(&format!("u{i}")), assistant_text(&format!("a{i}"))])
            .collect();
        assert_eq!(h.len(), 20);
        assert_eq!(pick_split(&h, 6), 8);
    }

    #[test]
    fn should_compact_only_when_over_threshold() {
        assert!(!should_compact(None, Some(1000)));
        assert!(!should_compact(Some(500), Some(1000)));
        assert!(should_compact(Some(1500), Some(1000)));
    }

    #[test]
    fn extract_prior_summary_recognizes_tagged_head() {
        let h = summary_turn("rolling state X");
        assert_eq!(
            extract_prior_summary(Some(&h)).as_deref(),
            Some("rolling state X")
        );
        // A plain user/assistant turn is NOT a prior summary.
        assert_eq!(extract_prior_summary(Some(&user_text("hi"))), None);
        assert_eq!(extract_prior_summary(Some(&assistant_text("a"))), None);
    }

    /// A whitespace-only tagged head must NOT be recognized (parity with the
    /// other backends — recognition uses the same predicate as `fold_prompt`).
    #[test]
    fn extract_prior_summary_rejects_whitespace_only_body() {
        let ws = user_text(&format!("{COMPACTION_TAG}\n   \n\t"));
        assert_eq!(extract_prior_summary(Some(&ws)), None);
    }

    /// A second compaction FOLDS: input is (prior summary) + (only the new
    /// delta), not the original raw turns.
    #[test]
    fn second_compaction_folds_prior_summary_plus_only_delta() {
        let mut h = vec![summary_turn("EARLIER distilled")];
        for i in 0..10 {
            h.push(user_text(&format!("new_user_{i}")));
            h.push(assistant_text(&format!("new_assistant_{i}")));
        }
        let plan = plan_fold(&h).expect("second fold planned");
        assert_eq!(plan.prior_summary.as_deref(), Some("EARLIER distilled"));
        assert_eq!(plan.delta_start, 1, "delta excludes the prior-summary turn");
    }

    /// The keep window stays balanced for a tool-heavy history — an
    /// `assistant` tool-call turn is paired with its `tool` result message and
    /// the split never orphans the result.
    #[test]
    fn keep_slice_balanced_for_tool_heavy_history() {
        let mut h = vec![user_text("start")];
        for i in 0..16 {
            let id = format!("call_{i}");
            h.push(assistant_call(&id, "view_file"));
            h.push(tool_result(&id));
        }
        let split = pick_split(&h, KEEP_RECENT_TURNS);
        if split < h.len() {
            // The first kept message must not be an orphaned tool result.
            assert!(
                !matches!(h[split].role, Role::Tool),
                "split orphaned a tool-result message (split={split})"
            );
        }
    }

    /// A long tool-result content is truncated on a char boundary, not
    /// panicking on a multibyte split.
    #[test]
    fn render_transcript_truncates_long_tool_result_on_char_boundary() {
        let filler = "a".repeat(509);
        let body = format!("{filler}世世世世世世");
        let history = vec![Message::tool_result("call_1", body)];
        let rendered = render_transcript(&history);
        assert!(rendered.contains("[tool_result]"));
        assert!(rendered.contains("…[truncated]"));
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
    }
}
