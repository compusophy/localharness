//! Context-window compaction for the Anthropic backend —
//! recency-weighted, incremental ("fold").
//!
//! Mirrors `backends/gemini/compaction.rs` exactly (keep them parallel). The
//! model's context is `[ one rolling summary turn ] ++ [ recent raw window ]`.
//! On each compaction we recognize the EXISTING rolling-summary turn at the
//! head by its tag, and fold ONLY the newly-aged turns into it:
//! `new_summary = summarize(prior_summary ++ newly_aged_turns)` — never
//! re-summarizing the original raw turns (discarded at the first compaction).
//! This keeps the synthetic prefix one turn no matter how long the chat runs
//! (boundedness) and re-summarizes only a bounded delta each time
//! (amortization). Never errors out of a turn — failures log at WARN and fall
//! back to dropping the oldest turns (still preserving the prior summary).
//!
//! ONE rolling tier ships; a deeper "gist" tier (the two-prior Fibonacci fold)
//! is a documented follow-up — see the Gemini module's header.

use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::backends::anthropic::api::SharedClient;
use crate::backends::anthropic::wire::{Block, Message, MessagesRequest, Role, DEFAULT_MAX_TOKENS};
use crate::error::Result;

/// Tag prepended to the rolling-summary turn so the model (and humans) can tell
/// what they're looking at AND so the next compaction can RECOGNIZE the prior
/// summary and fold into it rather than re-summarize it. Recognition is
/// load-bearing; don't change the tag without updating `extract_prior_summary`.
pub const COMPACTION_TAG: &str = "[compacted prior context]";

/// How many recent user/assistant turn pairs we always keep verbatim.
const KEEP_RECENT_TURNS: usize = 6;

/// Below this many history entries, compaction is a no-op.
const MIN_HISTORY_TO_COMPACT: usize = 8;

const SUMMARY_PROMPT: &str = "You maintain a rolling summary of a long agent conversation. \
    Below is the PRIOR SUMMARY (already-distilled older context) followed by NEW TURNS that \
    just aged out of the live window. Produce an UPDATED rolling summary in 200 words or less \
    that folds the new turns into the prior summary: preserve key facts, decisions, file paths, \
    and any open user requests; drop greetings, chit-chat, and redundant tool output. Do not \
    lose information that was in the prior summary unless it is now obsolete. Output only the \
    updated summary; no preamble.";

/// The plan for a single fold, computed PURELY from a history snapshot — no
/// network, no client. Splitting this out makes the amortization + boundedness
/// guarantees unit-testable with a stub summarizer (see tests).
struct FoldPlan {
    split: usize,
    prior_summary: Option<String>,
    delta_start: usize,
}

/// Try to compact `history` in place. Returns `true` if anything changed.
/// Safe to call from inside the agent loop — never errors, only logs.
pub async fn try_compact(history: &Mutex<Vec<Message>>, client: &SharedClient, model: &str) -> bool {
    let snapshot = history.lock().clone();
    let total = snapshot.len();
    if total < MIN_HISTORY_TO_COMPACT {
        debug!(total, "compaction: history too short, skipping");
        return false;
    }

    let plan = match plan_fold(&snapshot) {
        Some(p) => p,
        None => {
            debug!("compaction: nothing to fold before the keep-window");
            return false;
        }
    };

    let delta = &snapshot[plan.delta_start..plan.split];
    debug!(
        prior_summary = plan.prior_summary.is_some(),
        delta = delta.len(),
        to_keep = total - plan.split,
        "compaction: attempting incremental fold"
    );

    let summary = match summarize(client, model, plan.prior_summary.as_deref(), delta).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "compaction: summarization failed; folding to drop-oldest");
            return drop_oldest_fallback(history, plan.split, plan.prior_summary.as_deref());
        }
    };

    if summary.trim().is_empty() {
        warn!("compaction: summarization returned empty text; folding to drop-oldest");
        return drop_oldest_fallback(history, plan.split, plan.prior_summary.as_deref());
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
    let kept: Vec<Message> = hist.split_off(plan.split);
    hist.clear();
    hist.push(synthetic);
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: installed folded summary");
    true
}

/// Compute the fold plan for `history`, or `None` if there's nothing worth
/// folding. Recognizes a prior rolling-summary turn at index 0 by
/// `COMPACTION_TAG`; when present, the delta to fold STARTS AFTER it, so the
/// summarizer receives `(prior summary) + (only the newly-aged turns)` — the
/// amortization.
fn plan_fold(history: &[Message]) -> Option<FoldPlan> {
    let split = pick_split(history, KEEP_RECENT_TURNS);
    if split == 0 {
        return None;
    }
    let prior_summary = extract_prior_summary(history.first());
    let delta_start = if prior_summary.is_some() { 1 } else { 0 };
    // Head-only prefix (just the prior summary) → empty delta → no re-fold.
    if delta_start >= split {
        return None;
    }
    Some(FoldPlan {
        split,
        prior_summary,
        delta_start,
    })
}

/// Recover the prior rolling summary text from a head turn iff it is the
/// tagged synthetic summary. Returns the summary WITHOUT the tag line.
fn extract_prior_summary(head: Option<&Message>) -> Option<String> {
    let m = head?;
    if !matches!(m.role, Role::User) {
        return None;
    }
    let text = match m.content.as_slice() {
        [Block::Text { text }] => text,
        _ => return None,
    };
    let rest = text.strip_prefix(COMPACTION_TAG)?;
    let body = rest.trim_start_matches('\n').to_string();
    // A whitespace-only body is NOT a usable prior summary. Recognizing it as
    // one would set `delta_start = 1` (excluding the head from the fold delta)
    // while `fold_prompt`'s `!s.trim().is_empty()` guard would then refuse to
    // emit it as PRIOR SUMMARY — so the head turn's content would be dropped
    // from the summarizer input entirely (silent loss). Treat it as a normal
    // turn instead, so it's folded verbatim into the delta. Keeps the two
    // predicates consistent.
    if body.trim().is_empty() {
        return None;
    }
    Some(body)
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

/// Build the one-shot summarizer prompt for an incremental fold: the prior
/// rolling summary (if any) followed by the newly-aged delta transcript.
/// Pure + network-free so tests can assert the summarizer's INPUT contains
/// only `(prior summary + delta)`, proving the fold doesn't re-summarize the
/// whole history.
fn fold_prompt(prior_summary: Option<&str>, delta: &[Message]) -> String {
    let mut body = String::new();
    body.push_str(SUMMARY_PROMPT);
    body.push_str("\n\n--- PRIOR SUMMARY ---\n");
    match prior_summary {
        Some(s) if !s.trim().is_empty() => body.push_str(s),
        _ => body.push_str("(none — this is the first compaction)"),
    }
    body.push_str("\n\n--- NEW TURNS ---\n");
    body.push_str(&render_transcript(delta));
    body
}

async fn summarize(
    client: &SharedClient,
    model: &str,
    prior_summary: Option<&str>,
    delta: &[Message],
) -> Result<String> {
    let req = MessagesRequest {
        model: model.to_string(),
        max_tokens: DEFAULT_MAX_TOKENS,
        system: None,
        messages: vec![Message {
            role: Role::User,
            content: vec![Block::Text {
                text: fold_prompt(prior_summary, delta),
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
                        // Truncate at a CHAR boundary — tool-result content is
                        // arbitrary network text; a blind `[..512]` panics when
                        // byte 512 lands inside a multibyte UTF-8 char.
                        let mut end = 512;
                        while end > 0 && !body.is_char_boundary(end) {
                            end -= 1;
                        }
                        out.push_str(&body[..end]);
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
                Block::Other => {}
            }
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Last-resort fallback when summarization isn't available. Drops the aged-out
/// delta but PRESERVES the prior rolling summary if there is one — so a network
/// blip doesn't throw away every prior fold. The head stays one turn either
/// way, so boundedness still holds.
fn drop_oldest_fallback(
    history: &Mutex<Vec<Message>>,
    split: usize,
    prior_summary: Option<&str>,
) -> bool {
    let mut hist = history.lock();
    if split >= hist.len() {
        return false;
    }
    let kept: Vec<Message> = hist.split_off(split);
    hist.clear();
    let text = match prior_summary {
        Some(s) if !s.trim().is_empty() => {
            format!("{COMPACTION_TAG}\n{s}\n[some prior turns dropped without summary]")
        }
        _ => format!("{COMPACTION_TAG}\n[prior turns dropped]"),
    };
    hist.push(Message {
        role: Role::User,
        content: vec![Block::Text { text }],
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
