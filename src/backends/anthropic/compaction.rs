//! Context-window compaction for the Anthropic backend.
//!
//! Mirrors `backends/gemini/compaction.rs`: when running prompt tokens
//! exceed the configured threshold, summarize the old prefix of history
//! into one synthetic user turn via a non-streaming `/v1/messages`
//! one-shot. Never errors out of a turn — failures log at WARN and fall
//! back to dropping the oldest turns.

use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::backends::anthropic::api::SharedClient;
use crate::backends::anthropic::wire::{Block, Message, MessagesRequest, Role, DEFAULT_MAX_TOKENS};
use crate::error::Result;

/// Tag prepended to a compaction summary so the model (and humans) can
/// tell what they're looking at.
pub const COMPACTION_TAG: &str = "[compacted prior context]";

/// How many recent user/assistant turn pairs we always keep verbatim.
const KEEP_RECENT_TURNS: usize = 6;

/// Below this many history entries, compaction is a no-op.
const MIN_HISTORY_TO_COMPACT: usize = 8;

const SUMMARY_PROMPT: &str = "Summarize the conversation below in 200 words or less. \
    Preserve key facts, decisions, file paths, and any user requests. Drop greetings, \
    chit-chat, and redundant tool output. Output only the summary; no preamble.";

/// Try to compact `history` in place. Returns `true` if anything changed.
/// Safe to call from inside the agent loop — never errors, only logs.
pub async fn try_compact(history: &Mutex<Vec<Message>>, client: &SharedClient, model: &str) -> bool {
    let snapshot = history.lock().clone();
    let total = snapshot.len();
    if total < MIN_HISTORY_TO_COMPACT {
        debug!(total, "compaction: history too short, skipping");
        return false;
    }

    let split = pick_split(&snapshot, KEEP_RECENT_TURNS);
    if split == 0 {
        debug!("compaction: nothing to summarize before the keep-window");
        return false;
    }

    let (to_summarize, _to_keep) = snapshot.split_at(split);
    let summary = match summarize(client, model, to_summarize).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "compaction: summarization failed; falling back to drop-oldest");
            return drop_oldest_fallback(history, split);
        }
    };

    if summary.trim().is_empty() {
        warn!("compaction: summarization returned empty text; falling back to drop-oldest");
        return drop_oldest_fallback(history, split);
    }

    let synthetic = Message {
        role: Role::User,
        content: vec![Block::Text {
            text: format!("{COMPACTION_TAG}\n{summary}"),
        }],
    };
    let mut hist = history.lock();
    if hist.len() != total {
        warn!("compaction: history changed under us; aborting install");
        return false;
    }
    let kept: Vec<Message> = hist.split_off(split);
    hist.clear();
    hist.push(synthetic);
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: installed summary");
    true
}

/// Pick `i` such that history[..i] is summarized and history[i..] is kept.
/// Honors `KEEP_RECENT_TURNS` and never orphans a tool_use from its
/// matching tool_result (a user message of tool_result blocks must stay
/// with the assistant tool_use turn before it).
fn pick_split(history: &[Message], keep_pairs: usize) -> usize {
    let keep_entries = keep_pairs * 2;
    if history.len() <= keep_entries {
        return 0;
    }
    let mut split = history.len() - keep_entries;

    while split < history.len() {
        let prev = split.checked_sub(1).and_then(|i| history.get(i));
        let here = &history[split];

        let prev_carries_calls = prev.is_some_and(turn_has_tool_use);
        let here_is_result =
            matches!(here.role, Role::User) && turn_is_tool_result(here);

        let crosses_call_result = prev_carries_calls && here_is_result;
        let mid_turn = matches!(here.role, Role::Assistant);

        if crosses_call_result || mid_turn {
            split += 1;
            continue;
        }
        break;
    }
    split.min(history.len())
}

fn turn_has_tool_use(m: &Message) -> bool {
    m.content.iter().any(|b| matches!(b, Block::ToolUse { .. }))
}

fn turn_is_tool_result(m: &Message) -> bool {
    !m.content.is_empty()
        && m.content
            .iter()
            .all(|b| matches!(b, Block::ToolResult { .. }))
}

async fn summarize(client: &SharedClient, model: &str, history: &[Message]) -> Result<String> {
    let transcript = render_transcript(history);
    let req = MessagesRequest {
        model: model.to_string(),
        max_tokens: DEFAULT_MAX_TOKENS,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![Block::Text {
                text: format!("{SUMMARY_PROMPT}\n\n---\n{transcript}"),
            }],
        }],
        tools: Vec::new(),
        tool_choice: None,
        stream: false,
        temperature: None,
        thinking: None,
    };
    let resp = client.messages(&req).await?;
    Ok(resp.text())
}

fn render_transcript(history: &[Message]) -> String {
    let mut out = String::with_capacity(history.len() * 64);
    for entry in history {
        let role = match entry.role {
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
        };
        out.push_str("## ");
        out.push_str(role);
        out.push('\n');
        for block in &entry.content {
            match block {
                Block::Text { text } => out.push_str(text),
                Block::Thinking { thinking, .. } => {
                    out.push_str("[thinking] ");
                    out.push_str(thinking);
                }
                Block::ToolUse { name, input, .. } => {
                    out.push_str("[tool_use ");
                    out.push_str(name);
                    out.push_str("] ");
                    out.push_str(&input.to_string());
                }
                Block::ToolResult { content, .. } => {
                    out.push_str("[tool_result] ");
                    let body = content.to_string();
                    if body.len() > 512 {
                        out.push_str(&body[..512]);
                        out.push_str("…[truncated]");
                    } else {
                        out.push_str(&body);
                    }
                }
                Block::Image { source } => {
                    out.push_str("[image ");
                    out.push_str(&source.media_type);
                    out.push(']');
                }
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn drop_oldest_fallback(history: &Mutex<Vec<Message>>, split: usize) -> bool {
    let mut hist = history.lock();
    if split >= hist.len() {
        return false;
    }
    let kept: Vec<Message> = hist.split_off(split);
    hist.clear();
    hist.push(Message {
        role: Role::User,
        content: vec![Block::Text {
            text: format!("{COMPACTION_TAG}\n[prior turns dropped]"),
        }],
    });
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: drop-oldest fallback applied");
    true
}

/// Decide whether to attempt compaction. `threshold` of `None` disables.
pub fn should_compact(total_tokens: Option<i32>, threshold: Option<u32>) -> bool {
    match (total_tokens, threshold) {
        (Some(t), Some(th)) => t as u32 > th,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user_text(s: &str) -> Message {
        Message::user_text(s)
    }
    fn assistant_text(s: &str) -> Message {
        Message::assistant_text(s)
    }
    fn assistant_call(id: &str, name: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![Block::ToolUse {
                id: id.into(),
                name: name.into(),
                input: json!({}),
            }],
        }
    }
    fn user_result(id: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::ToolResult {
                tool_use_id: id.into(),
                content: json!({"ok": true}),
                is_error: None,
            }],
        }
    }

    #[test]
    fn pick_split_below_keep_window() {
        let h = vec![user_text("u1"), assistant_text("a1")];
        assert_eq!(pick_split(&h, 6), 0);
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
    fn pick_split_does_not_orphan_tool_result() {
        let mut h: Vec<Message> = (0..20).map(|i| user_text(&format!("u{i}"))).collect();
        let pair_index = 14;
        h[pair_index] = assistant_call("toolu_1", "view_file");
        h[pair_index + 1] = user_result("toolu_1");
        let split = pick_split(&h, 6);
        assert_ne!(split, pair_index + 1, "split must not orphan result");
    }

    #[test]
    fn should_compact_only_when_over_threshold() {
        assert!(!should_compact(None, Some(1000)));
        assert!(!should_compact(Some(500), None));
        assert!(!should_compact(Some(500), Some(1000)));
        assert!(should_compact(Some(1500), Some(1000)));
    }
}
