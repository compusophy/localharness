//! Anthropic adapter for the shared compaction fold engine
//! (`crate::backends::compaction`).
//!
//! ALL algorithm logic — the fold plan, keep-window split, prior-summary
//! recognition, prompt build, install, drop-oldest fallback, thresholds —
//! lives in the engine (see its module doc for the full strategy write-up).
//! This module supplies only the Anthropic-specific bits: the wire-message
//! seam (`CompactionModel` over `Message`/`Block`) and the one-shot
//! Messages-API summarization request.

use parking_lot::Mutex;

use crate::backends::anthropic::api::SharedClient;
use crate::backends::anthropic::wire::{
    Block, Message, MessagesRequest, Role, DEFAULT_MAX_TOKENS, DEFAULT_MODEL,
};
use crate::backends::compaction::{self as engine, CompactionModel};
pub use crate::backends::compaction::{should_compact, COMPACTION_TAG};
use crate::error::Result;

/// Model used to fold the rolling summary — a FIXED cheap/fast tier, NOT the
/// session model. The engine's own `SUMMARY_PROMPT` notes cheap+fast is right
/// here (the summary must be faithful, not brilliant), so an Opus session folds
/// its summary on Haiku instead of paying Opus rates per compaction. Haiku is
/// already this backend's `DEFAULT_MODEL` (the cheapest tier).
const SUMMARIZER_MODEL: &str = DEFAULT_MODEL;

/// The Anthropic side of the [`CompactionModel`] seam — a zero-sized marker
/// the engine is monomorphized over.
struct AnthropicCompaction;

impl CompactionModel for AnthropicCompaction {
    type Message = Message;

    fn is_user(m: &Message) -> bool {
        matches!(m.role, Role::User)
    }

    fn sole_text(m: &Message) -> Option<&str> {
        match m.content.as_slice() {
            [Block::Text { text }] => Some(text),
            _ => None,
        }
    }

    fn is_tool_result_turn(m: &Message) -> bool {
        turn_is_tool_result(m)
    }

    fn user_text(text: String) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::Text { text }],
        }
    }

    fn render_message(entry: &Message, out: &mut String) {
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
                    engine::push_truncated(out, &content.to_string());
                }
                Block::Image { source } => {
                    out.push_str("[image ");
                    out.push_str(&source.media_type);
                    out.push(']');
                }
                Block::Other => {}
            }
            out.push('\n');
        }
    }
}

/// True iff every block of the turn is a `tool_result` (and there is at
/// least one) — the shape `pick_split` must never orphan.
fn turn_is_tool_result(m: &Message) -> bool {
    !m.content.is_empty()
        && m.content
            .iter()
            .all(|b| matches!(b, Block::ToolResult { .. }))
}

/// Try to compact `history` in place. Returns `true` if anything changed.
/// Safe to call from inside the agent loop — never errors, only logs.
///
/// The `_session_model` is intentionally ignored: compaction folds with the
/// fixed cheap [`SUMMARIZER_MODEL`], never the (possibly Opus-tier) session
/// model. Kept in the signature so callers don't need to change.
pub async fn try_compact(
    history: &Mutex<Vec<Message>>,
    client: &SharedClient,
    _session_model: &str,
) -> bool {
    engine::try_compact::<AnthropicCompaction, _, _>(history, |prompt| {
        summarize(client, SUMMARIZER_MODEL, prompt)
    })
    .await
}

/// One-shot summarization request: feed the fold prompt as a single user
/// message — no system, no tools, no history.
async fn summarize(client: &SharedClient, model: &str, prompt: String) -> Result<String> {
    let req = MessagesRequest {
        model: model.to_string(),
        max_tokens: DEFAULT_MAX_TOKENS,
        system: MessagesRequest::system_from(None),
        messages: vec![Message {
            role: Role::User,
            content: vec![Block::Text { text: prompt }],
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

// ---- test-only thin wrappers ----
//
// The tests below are the PINNED pre-extraction suite, kept byte-identical:
// they exercise the SHARED engine through this adapter's seam. These wrappers
// restore the original module-local names/signatures the tests call.

#[cfg(test)]
use crate::backends::compaction::KEEP_RECENT_TURNS;

#[cfg(test)]
fn pick_split(history: &[Message], keep_pairs: usize) -> usize {
    engine::pick_split::<AnthropicCompaction>(history, keep_pairs)
}

#[cfg(test)]
fn plan_fold(history: &[Message]) -> Option<engine::FoldPlan> {
    engine::plan_fold::<AnthropicCompaction>(history)
}

#[cfg(test)]
fn extract_prior_summary(head: Option<&Message>) -> Option<String> {
    engine::extract_prior_summary::<AnthropicCompaction>(head)
}

#[cfg(test)]
fn fold_prompt(prior_summary: Option<&str>, delta: &[Message]) -> String {
    engine::fold_prompt::<AnthropicCompaction>(prior_summary, delta)
}

#[cfg(test)]
fn render_transcript(history: &[Message]) -> String {
    engine::render_transcript::<AnthropicCompaction>(history)
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
    /// A synthetic rolling-summary turn, exactly as `try_compact` installs it.
    fn summary_turn(s: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::Text {
                text: format!("{COMPACTION_TAG}\n{s}"),
            }],
        }
    }

    // ---- pick_split (unchanged behavior) ----

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

    // ---- prior-summary recognition ----

    #[test]
    fn extract_prior_summary_recognizes_tagged_head() {
        let h = summary_turn("rolling state X");
        assert_eq!(
            extract_prior_summary(Some(&h)).as_deref(),
            Some("rolling state X")
        );
    }

    #[test]
    fn extract_prior_summary_ignores_plain_user_turn() {
        assert_eq!(extract_prior_summary(Some(&user_text("just a message"))), None);
        assert_eq!(extract_prior_summary(Some(&assistant_text("a reply"))), None);
        assert_eq!(extract_prior_summary(None), None);
    }

    /// A tagged head whose body is EMPTY or whitespace-only must NOT be
    /// recognized as a prior summary. `plan_fold` keys `delta_start` off
    /// `prior_summary.is_some()`, but `fold_prompt` only emits a summary when
    /// `!s.trim().is_empty()`. If those two predicates disagree, the head turn
    /// is excluded from the delta AND omitted from PRIOR SUMMARY — dropped from
    /// the summarizer input entirely (silent loss). Recognition must use the
    /// SAME predicate as `fold_prompt`. (Parity with the Gemini backend.)
    #[test]
    fn extract_prior_summary_rejects_whitespace_only_body() {
        let empty = user_text(COMPACTION_TAG);
        let ws = user_text(&format!("{COMPACTION_TAG}\n   \n\t"));
        assert_eq!(extract_prior_summary(Some(&empty)), None);
        assert_eq!(extract_prior_summary(Some(&ws)), None);
    }

    /// End-to-end consequence of the fix: a whitespace-only tagged head is
    /// folded as part of the delta (treated as a normal turn), NOT excluded.
    #[test]
    fn whitespace_tagged_head_is_folded_not_dropped() {
        let mut h = vec![user_text(&format!("{COMPACTION_TAG}\n  "))];
        for i in 0..10 {
            h.push(user_text(&format!("u{i}")));
            h.push(assistant_text(&format!("a{i}")));
        }
        let plan = plan_fold(&h).expect("fold planned");
        assert!(
            plan.prior_summary.is_none(),
            "whitespace-only head must not be taken as a prior summary"
        );
        assert_eq!(
            plan.delta_start, 0,
            "the head turn must be INCLUDED in the fold delta, not skipped"
        );
    }

    // ---- THE AMORTIZATION PROOF ----

    /// Render the input the summarizer WOULD receive for a given snapshot,
    /// using the same pure planning + prompt builder `try_compact` uses. No
    /// network. This is the stub-summarizer probe.
    fn summarizer_input(history: &[Message]) -> String {
        let plan = plan_fold(history).expect("a fold is planned");
        let delta = &history[plan.delta_start..plan.split];
        fold_prompt(plan.prior_summary.as_deref(), delta)
    }

    /// A second compaction must FOLD: its summarizer input is
    /// `(prior summary) + (only the newly-aged delta)` — it must NOT contain
    /// the original raw turns already folded into the prior summary.
    #[test]
    fn second_compaction_folds_prior_summary_plus_only_delta() {
        let mut h = vec![summary_turn("EARLIER_RAW_FACT distilled")];
        for i in 0..10 {
            h.push(user_text(&format!("new_user_{i}")));
            h.push(assistant_text(&format!("new_assistant_{i}")));
        }

        let plan = plan_fold(&h).expect("second fold planned");
        assert_eq!(plan.prior_summary.as_deref(), Some("EARLIER_RAW_FACT distilled"));
        assert_eq!(plan.delta_start, 1, "delta excludes the prior-summary turn");

        let input = summarizer_input(&h);
        assert!(
            input.contains("EARLIER_RAW_FACT distilled"),
            "fold input must include the prior summary"
        );
        assert!(input.contains("new_user_0"), "fold input must include the delta");
        assert!(
            !input.contains(COMPACTION_TAG),
            "the prior summary turn must NOT be re-rendered as a transcript entry to summarize"
        );

        let delta_len = plan.split - plan.delta_start;
        assert!(
            delta_len < h.len(),
            "the fold delta ({delta_len}) must be a strict subset of history ({})",
            h.len()
        );
    }

    #[test]
    fn first_compaction_has_no_prior_summary() {
        let mut h = vec![user_text("genesis")];
        for i in 0..10 {
            h.push(user_text(&format!("u{i}")));
            h.push(assistant_text(&format!("a{i}")));
        }
        let plan = plan_fold(&h).expect("first fold planned");
        assert!(plan.prior_summary.is_none());
        assert_eq!(plan.delta_start, 0, "no prior summary → fold the whole prefix");
    }

    // ---- BOUNDEDNESS ----

    fn compact_install(history: &[Message], stub_summary: &str) -> Vec<Message> {
        let plan = match plan_fold(history) {
            Some(p) => p,
            None => return history.to_vec(),
        };
        let mut out = vec![summary_turn(stub_summary)];
        out.extend_from_slice(&history[plan.split..]);
        out
    }

    /// BOUNDEDNESS INVARIANT: as the conversation grows without bound, the
    /// synthetic prefix stays exactly ONE turn and the whole compacted history
    /// stays bounded — it does NOT grow with N.
    #[test]
    fn synthetic_prefix_is_one_turn_and_history_bounded() {
        let keep_entries = KEEP_RECENT_TURNS * 2;
        let bound = 1 + keep_entries;

        let mut h: Vec<Message> = Vec::new();
        let mut lengths = Vec::new();
        for i in 0..500 {
            h.push(user_text(&format!("u{i}")));
            h.push(assistant_text(&format!("a{i}")));

            if h.len() >= keep_entries + 4 {
                h = compact_install(&h, "rolling");
                let summary_count = h
                    .iter()
                    .filter(|m| extract_prior_summary(Some(m)).is_some())
                    .count();
                assert_eq!(
                    summary_count, 1,
                    "exactly one rolling summary turn after compaction (i={i})"
                );
                assert!(
                    extract_prior_summary(h.first()).is_some(),
                    "the single summary turn is at the head (i={i})"
                );
                lengths.push(h.len());
            }
        }

        for (k, &len) in lengths.iter().enumerate() {
            assert!(
                len <= bound,
                "compacted history length {len} exceeded bound {bound} at compaction #{k}"
            );
        }
        let first = lengths.first().copied().unwrap();
        let last = lengths.last().copied().unwrap();
        assert!(
            last <= first,
            "compacted history must not grow with N (first={first}, last={last})"
        );
    }

    // ---- keep-window preservation (tool round-trips never summarized away) ----

    fn tool_heavy_history(rounds: usize) -> Vec<Message> {
        let mut h = vec![user_text("start")];
        for i in 0..rounds {
            let id = format!("toolu_{i}");
            h.push(assistant_call(&id, "view_file"));
            h.push(user_result(&id));
        }
        h
    }

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

    fn assert_keep_slice_balanced(h: &[Message]) {
        let split = pick_split(h, KEEP_RECENT_TURNS);
        let kept = &h[split..];
        let uses = tool_use_ids(kept);
        let results = tool_result_ids(kept);
        for r in &results {
            assert!(
                uses.contains(r),
                "ORPHAN tool_result {r:?} kept without its tool_use (split={split}, kept_len={})",
                kept.len()
            );
        }
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
        for rounds in 4..=20 {
            let h = tool_heavy_history(rounds);
            assert_keep_slice_balanced(&h);
        }
    }

    #[test]
    fn keep_slice_never_starts_with_orphan_tool_result() {
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

    /// The kept window stays balanced even when a prior summary occupies the
    /// head (the fold path).
    #[test]
    fn fold_keep_window_balanced_with_prior_summary_head() {
        let mut h = vec![summary_turn("prior")];
        for i in 0..12 {
            let id = format!("toolu_{i}");
            h.push(assistant_call(&id, "view_file"));
            h.push(user_result(&id));
        }
        let plan = plan_fold(&h).expect("fold planned");
        let kept = &h[plan.split..];
        assert_eq!(
            tool_use_ids(kept),
            tool_result_ids(kept),
            "kept window must be a balanced set of tool pairs"
        );
    }

    #[test]
    fn pick_split_keeps_at_least_something_when_over_window() {
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
        let h: Vec<Message> = (0..12).map(|i| user_text(&format!("u{i}"))).collect();
        assert_eq!(pick_split(&h, 6), 0);
    }

    /// A head-only prefix (just the prior summary, nothing aged out yet) is a
    /// no-op fold — we must not re-summarize the summary into itself.
    #[test]
    fn no_op_fold_when_only_prior_summary_before_window() {
        let mut h = vec![summary_turn("prior")];
        for i in 0..6 {
            h.push(user_text(&format!("u{i}")));
            h.push(assistant_text(&format!("a{i}")));
        }
        assert!(
            plan_fold(&h).is_none(),
            "a head-only prefix must not trigger a re-fold of the summary"
        );
    }

    // ---- render_transcript regressions (preserved) ----

    /// A tool_result whose body exceeds 512 bytes with a multibyte UTF-8 char
    /// straddling byte 512 must truncate at a char boundary, not panic.
    #[test]
    fn render_transcript_truncates_long_tool_result_on_char_boundary() {
        let filler = "a".repeat(509);
        let body = format!("{filler}世世世世世世");
        let history = vec![Message {
            role: Role::Assistant,
            content: vec![Block::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: json!(body),
                is_error: None,
            }],
        }];
        let rendered = render_transcript(&history);
        assert!(rendered.contains("[tool_result]"));
        assert!(rendered.contains("…[truncated]"));
        assert!(std::str::from_utf8(rendered.as_bytes()).is_ok());
    }

    /// An unmodeled content block decoded as `Block::Other` must render
    /// harmlessly, not panic the transcript builder.
    #[test]
    fn render_transcript_ignores_other_block() {
        let history = vec![Message {
            role: Role::Assistant,
            content: vec![
                Block::Other,
                Block::Text {
                    text: "after".into(),
                },
            ],
        }];
        let rendered = render_transcript(&history);
        assert!(rendered.contains("after"));
    }
}
