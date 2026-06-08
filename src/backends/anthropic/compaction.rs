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
/// Honors `KEEP_RECENT_TURNS` and never orphans a tool_use from its matching
/// tool_result.
///
/// The kept slice `history[i..]` is API-valid iff its first message is NOT a
/// lone `tool_result` user turn — otherwise the matching `tool_use` (at `i-1`)
/// would be in the summarized prefix and the request 400s on a dangling
/// `tool_result`. (Linear history guarantees a `tool_use` is always followed
/// by its `tool_result`, so a kept *tool_use* never dangles; only the leading
/// `tool_result` can orphan.)
///
/// We therefore start at the keep-window boundary and walk the boundary
/// EARLIER (toward 0) past any leading `tool_result`, absorbing the orphaned
/// pair into the summary. Walking earlier keeps strictly MORE history than
/// requested (never less) and can never run off the end — the old
/// walk-FORWARD logic could chain through a long run of tool round-trips and
/// keep ZERO messages, summarizing away the entire recent context including
/// the turn being answered.
fn pick_split(history: &[Message], keep_pairs: usize) -> usize {
    let keep_entries = keep_pairs * 2;
    if history.len() <= keep_entries {
        return 0;
    }
    let mut split = history.len() - keep_entries;

    // Walk the boundary earlier past any leading orphaned tool_result so the
    // kept slice begins on a clean turn boundary.
    while split > 0 && is_leading_orphan(history, split) {
        split -= 1;
    }
    split
}

/// True if keeping `history[split..]` would orphan a leading `tool_result`:
/// the first kept message is a user turn of only `tool_result` blocks, whose
/// matching `tool_use` lives at `split-1` (and would be summarized away).
fn is_leading_orphan(history: &[Message], split: usize) -> bool {
    match history.get(split) {
        Some(m) => matches!(m.role, Role::User) && turn_is_tool_result(m),
        None => false,
    }
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

    // --- Wire-invariant probes added by the compaction correctness dive ---

    /// Build a linear tool-heavy history: u_text, then N (assistant tool_use,
    /// user tool_result) round-trips. This is the realistic shape of an agent
    /// session that actually triggers compaction.
    fn tool_heavy_history(rounds: usize) -> Vec<Message> {
        let mut h = vec![user_text("start")];
        for i in 0..rounds {
            let id = format!("toolu_{i}");
            h.push(assistant_call(&id, "view_file"));
            h.push(user_result(&id));
        }
        h
    }

    /// Collect the set of tool_use ids in a message slice.
    fn tool_use_ids(ms: &[Message]) -> std::collections::HashSet<String> {
        let mut s = std::collections::HashSet::new();
        for m in ms {
            for b in &m.content {
                if let Block::ToolUse { id, .. } = b {
                    s.insert(id.clone());
                }
            }
        }
        s
    }

    /// Collect the set of tool_result ids in a message slice.
    fn tool_result_ids(ms: &[Message]) -> std::collections::HashSet<String> {
        let mut s = std::collections::HashSet::new();
        for m in ms {
            for b in &m.content {
                if let Block::ToolResult { tool_use_id, .. } = b {
                    s.insert(tool_use_id.clone());
                }
            }
        }
        s
    }

    /// THE CORE INVARIANT: after compaction, the message list sent to the API
    /// (synthetic summary + kept) must have every tool_result paired with a
    /// tool_use *within the kept slice*. The summary is opaque text, so a
    /// tool_use that was summarized cannot satisfy a kept tool_result, and a
    /// kept tool_use whose result was summarized leaves a dangling call.
    ///
    /// We assert directly on `to_keep` (history[split..]): its tool_result ids
    /// must be a subset of its tool_use ids, and vice-versa — fully balanced.
    fn assert_keep_slice_balanced(h: &[Message]) {
        let split = pick_split(h, KEEP_RECENT_TURNS);
        let kept = &h[split..];
        let uses = tool_use_ids(kept);
        let results = tool_result_ids(kept);
        // Every kept tool_result must have its tool_use kept too.
        for r in &results {
            assert!(
                uses.contains(r),
                "ORPHAN tool_result {r:?} kept without its tool_use (split={split}, kept_len={})",
                kept.len()
            );
        }
        // Every kept tool_use must have its tool_result kept too (otherwise the
        // assistant turn ends on an unanswered call — also a 400 from Anthropic).
        for u in &uses {
            assert!(
                results.contains(u),
                "DANGLING tool_use {u:?} kept without its tool_result (split={split}, kept_len={})",
                kept.len()
            );
        }
    }

    #[test]
    fn keep_slice_balanced_for_tool_heavy_history() {
        // Vary the number of round-trips so the natural split lands on every
        // possible alignment (on a tool_use, on a tool_result, on the seam).
        for rounds in 4..=20 {
            let h = tool_heavy_history(rounds);
            assert_keep_slice_balanced(&h);
        }
    }

    #[test]
    fn keep_slice_never_starts_with_orphan_tool_result() {
        // The first kept message must never be a lone tool_result (its tool_use
        // would be in the summarized prefix → orphaned in the API request).
        for rounds in 4..=20 {
            let h = tool_heavy_history(rounds);
            let split = pick_split(&h, KEEP_RECENT_TURNS);
            if split < h.len() {
                let first = &h[split];
                assert!(
                    !(matches!(first.role, Role::User) && turn_is_tool_result(first)),
                    "first kept message is an orphaned tool_result (rounds={rounds}, split={split})"
                );
            }
        }
    }

    #[test]
    fn pick_split_keeps_at_least_something_when_over_window() {
        // Regression guard: the walk-forward must not run away to the end and
        // keep ZERO messages (which would summarize the turn currently being
        // answered and lose ALL recent context).
        for rounds in 4..=30 {
            let h = tool_heavy_history(rounds);
            let split = pick_split(&h, KEEP_RECENT_TURNS);
            assert!(
                split < h.len(),
                "pick_split kept nothing (rounds={rounds}, split={split}, len={})",
                h.len()
            );
        }
    }

    #[test]
    fn pick_split_empty_history() {
        assert_eq!(pick_split(&[], 6), 0);
    }

    #[test]
    fn pick_split_single_message() {
        assert_eq!(pick_split(&[user_text("only")], 6), 0);
    }

    #[test]
    fn pick_split_exactly_at_keep_window() {
        // len == keep_entries → nothing to summarize.
        let h: Vec<Message> = (0..12).map(|i| user_text(&format!("u{i}"))).collect();
        assert_eq!(pick_split(&h, 6), 0);
    }
}
