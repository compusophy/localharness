//! Gemini adapter for the shared compaction fold engine
//! (`crate::backends::compaction`).
//!
//! ALL algorithm logic — the fold plan, keep-window split, prior-summary
//! recognition, prompt build, install, drop-oldest fallback, thresholds —
//! lives in the engine (see its module doc for the full strategy write-up).
//! This module supplies only the Gemini-specific bits: the wire-message seam
//! (`CompactionModel` over `Content`/`Part`) and the one-shot streaming
//! summarization request.

use parking_lot::Mutex;
use tracing::warn;

use crate::backends::compaction::{self as engine, CompactionModel};
pub use crate::backends::compaction::{should_compact, COMPACTION_TAG};
use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::wire::{
    Content, ContentRole, FinishReason, GenerateContentRequest, Part,
};
use crate::error::Result;

/// The Gemini side of the [`CompactionModel`] seam — a zero-sized marker the
/// engine is monomorphized over.
struct GeminiCompaction;

impl CompactionModel for GeminiCompaction {
    type Message = Content;

    fn is_user(m: &Content) -> bool {
        matches!(m.role, ContentRole::User)
    }

    fn sole_text(m: &Content) -> Option<&str> {
        match m.parts.as_slice() {
            [Part::Text { text }] => Some(text),
            _ => None,
        }
    }

    fn is_tool_result_turn(m: &Content) -> bool {
        turn_is_function_response(m)
    }

    fn user_text(text: String) -> Content {
        Content {
            role: ContentRole::User,
            parts: vec![Part::Text { text }],
        }
    }

    fn render_message(entry: &Content, out: &mut String) {
        let role = match entry.role {
            ContentRole::User => "USER",
            ContentRole::Model => "MODEL",
        };
        out.push_str("## ");
        out.push_str(role);
        out.push('\n');
        for part in &entry.parts {
            match part {
                Part::Text { text } => out.push_str(text),
                Part::Thought { text: Some(t), .. } => {
                    out.push_str("[thought] ");
                    out.push_str(t);
                }
                Part::FunctionCall { function_call, .. } => {
                    out.push_str("[tool_call ");
                    out.push_str(&function_call.name);
                    out.push_str("] ");
                    out.push_str(&function_call.args.to_string());
                }
                Part::FunctionResponse { function_response } => {
                    out.push_str("[tool_result ");
                    out.push_str(&function_response.name);
                    out.push_str("] ");
                    engine::push_truncated(out, &function_response.response.to_string());
                }
                Part::InlineData { inline_data } => {
                    out.push_str("[inline_data ");
                    out.push_str(&inline_data.mime_type);
                    out.push(']');
                }
                _ => {}
            }
            out.push('\n');
        }
    }
}

/// True iff every part of the turn is a `functionResponse` (and there is at
/// least one) — the shape `pick_split` must never orphan.
fn turn_is_function_response(c: &Content) -> bool {
    c.parts
        .iter()
        .all(|p| matches!(p, Part::FunctionResponse { .. }))
        && !c.parts.is_empty()
}

/// Try to compact `history` in place. Returns `true` if anything
/// changed. Safe to call from inside the agent loop — never errors out,
/// only logs.
pub async fn try_compact(
    history: &Mutex<Vec<Content>>,
    client: &SharedClient,
    model: &str,
) -> bool {
    engine::try_compact::<GeminiCompaction, _, _>(history, |prompt| {
        summarize(client, model, prompt)
    })
    .await
}

/// One-shot summarization request: feed the fold prompt as a single user
/// message — no system instruction, no tools, no history.
async fn summarize(client: &SharedClient, model: &str, prompt: String) -> Result<String> {
    use futures_util::stream::StreamExt;

    let req = GenerateContentRequest {
        contents: vec![Content {
            role: ContentRole::User,
            parts: vec![Part::Text { text: prompt }],
        }],
        ..Default::default()
    };

    let mut stream = client.stream_generate(model, &req).await?;
    let mut out = String::new();
    let mut finish: Option<FinishReason> = None;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        for cand in chunk.candidates {
            if let Some(content) = cand.content {
                for part in content.parts {
                    // Gemini 3.x stamps EVERY part with `thought`, so visible
                    // summary text arrives as `Thought { thought: false,
                    // text: Some(_) }` — same quirk the main loop guards
                    // (`loop.rs`). Without the second arm the summary came back
                    // EMPTY on 3.x (issue #83).
                    match part {
                        Part::Text { text }
                        | Part::Thought {
                            thought: false,
                            text: Some(text),
                            ..
                        } => out.push_str(&text),
                        _ => {}
                    }
                }
            }
            if let Some(r) = cand.finish_reason {
                finish = Some(r);
            }
        }
    }
    if !matches!(finish, Some(FinishReason::Stop) | None) {
        warn!(?finish, "compaction summary finished abnormally");
    }
    Ok(out)
}

// ---- test-only thin wrappers ----
//
// The tests below are the PINNED pre-extraction suite, kept byte-identical:
// they exercise the SHARED engine through this adapter's seam. These wrappers
// restore the original module-local names/signatures the tests call.

#[cfg(test)]
use crate::backends::compaction::KEEP_RECENT_TURNS;

#[cfg(test)]
fn pick_split(history: &[Content], keep_pairs: usize) -> usize {
    engine::pick_split::<GeminiCompaction>(history, keep_pairs)
}

#[cfg(test)]
fn plan_fold(history: &[Content]) -> Option<engine::FoldPlan> {
    engine::plan_fold::<GeminiCompaction>(history)
}

#[cfg(test)]
fn extract_prior_summary(head: Option<&Content>) -> Option<String> {
    engine::extract_prior_summary::<GeminiCompaction>(head)
}

#[cfg(test)]
fn fold_prompt(prior_summary: Option<&str>, delta: &[Content]) -> String {
    engine::fold_prompt::<GeminiCompaction>(prior_summary, delta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::gemini::wire::{FunctionCall, FunctionResponse};
    use serde_json::json;

    fn user_text(s: &str) -> Content {
        Content {
            role: ContentRole::User,
            parts: vec![Part::Text { text: s.into() }],
        }
    }
    fn model_text(s: &str) -> Content {
        Content {
            role: ContentRole::Model,
            parts: vec![Part::Text { text: s.into() }],
        }
    }
    fn model_call(name: &str) -> Content {
        Content {
            role: ContentRole::Model,
            parts: vec![Part::FunctionCall {
                function_call: FunctionCall {
                    name: name.into(),
                    args: json!({}),
                },
                thought_signature: None,
            }],
        }
    }
    fn user_response(name: &str) -> Content {
        Content {
            role: ContentRole::User,
            parts: vec![Part::FunctionResponse {
                function_response: FunctionResponse {
                    name: name.into(),
                    response: json!({"ok": true}),
                },
            }],
        }
    }
    /// A synthetic rolling-summary turn, exactly as `try_compact` installs it.
    fn summary_turn(s: &str) -> Content {
        Content {
            role: ContentRole::User,
            parts: vec![Part::Text {
                text: format!("{COMPACTION_TAG}\n{s}"),
            }],
        }
    }

    // ---- pick_split (unchanged behavior) ----

    #[test]
    fn pick_split_below_keep_window() {
        let h = vec![user_text("u1"), model_text("m1")];
        assert_eq!(pick_split(&h, 6), 0);
    }

    #[test]
    fn pick_split_respects_keep_window() {
        // 10 user/model pairs = 20 entries. Keep last 6 pairs = 12
        // entries. Expected split: 20 - 12 = 8.
        let h: Vec<Content> = (0..10)
            .flat_map(|i| {
                vec![
                    user_text(&format!("u{i}")),
                    model_text(&format!("m{i}")),
                ]
            })
            .collect();
        assert_eq!(h.len(), 20);
        assert_eq!(pick_split(&h, 6), 8);
    }

    #[test]
    fn pick_split_does_not_orphan_function_response() {
        let mut h: Vec<Content> = (0..20).map(|i| user_text(&format!("u{i}"))).collect();
        let pair_index = 14;
        h[pair_index] = model_call("view_file");
        h[pair_index + 1] = user_response("view_file");
        let split = pick_split(&h, 6);
        assert_ne!(split, pair_index + 1, "split must not orphan response");
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
        assert_eq!(extract_prior_summary(Some(&model_text("model reply"))), None);
        assert_eq!(extract_prior_summary(None), None);
    }

    /// A tagged head whose body is EMPTY or whitespace-only must NOT be
    /// recognized as a prior summary. `plan_fold` keys `delta_start` off
    /// `prior_summary.is_some()`, but `fold_prompt` only emits a summary when
    /// `!s.trim().is_empty()`. If those two predicates disagree, the head turn
    /// is excluded from the delta AND omitted from PRIOR SUMMARY — dropped from
    /// the summarizer input entirely (silent loss). Recognition must use the
    /// SAME predicate as `fold_prompt`.
    #[test]
    fn extract_prior_summary_rejects_whitespace_only_body() {
        // Exactly the tag (empty body) and the tag + only whitespace.
        let empty = user_text(COMPACTION_TAG);
        let ws = user_text(&format!("{COMPACTION_TAG}\n   \n\t"));
        assert_eq!(extract_prior_summary(Some(&empty)), None);
        assert_eq!(extract_prior_summary(Some(&ws)), None);
    }

    /// End-to-end consequence of the fix: a whitespace-only tagged head is
    /// folded as part of the delta (treated as a normal turn), NOT excluded.
    /// Before the fix, `delta_start` was 1 here while the prompt printed
    /// "(none)" — so the head's content never reached the summarizer.
    #[test]
    fn whitespace_tagged_head_is_folded_not_dropped() {
        let mut h = vec![user_text(&format!("{COMPACTION_TAG}\n  "))];
        for i in 0..10 {
            h.push(user_text(&format!("u{i}")));
            h.push(model_text(&format!("m{i}")));
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
    fn summarizer_input(history: &[Content]) -> String {
        let plan = plan_fold(history).expect("a fold is planned");
        let delta = &history[plan.delta_start..plan.split];
        fold_prompt(plan.prior_summary.as_deref(), delta)
    }

    /// A second compaction must FOLD: its summarizer input is
    /// `(prior summary) + (only the newly-aged delta)` — it must NOT contain
    /// the original raw turns that were already folded into the prior summary.
    #[test]
    fn second_compaction_folds_prior_summary_plus_only_delta() {
        // State right after a first compaction: a rolling summary turn, then a
        // raw window, then MORE turns appended by subsequent activity (so a
        // second compaction is due).
        let mut h = vec![summary_turn("EARLIER_RAW_FACT distilled")];
        // 20 fresh entries since the first compaction.
        for i in 0..10 {
            h.push(user_text(&format!("new_user_{i}")));
            h.push(model_text(&format!("new_model_{i}")));
        }

        let plan = plan_fold(&h).expect("second fold planned");
        // The prior summary is recognized and the delta starts AFTER it.
        assert_eq!(plan.prior_summary.as_deref(), Some("EARLIER_RAW_FACT distilled"));
        assert_eq!(plan.delta_start, 1, "delta excludes the prior-summary turn");

        let input = summarizer_input(&h);
        // It carries the prior summary verbatim...
        assert!(
            input.contains("EARLIER_RAW_FACT distilled"),
            "fold input must include the prior summary"
        );
        // ...and the newly-aged turns...
        assert!(input.contains("new_user_0"), "fold input must include the delta");
        // ...but NEVER the raw form of what the prior summary already captured.
        // We assert the summary turn's TAG is not re-fed as a transcript entry
        // (it's fed as plain prior-summary text, not re-summarized).
        assert!(
            !input.contains(COMPACTION_TAG),
            "the prior summary turn must NOT be re-rendered as a transcript entry to summarize"
        );

        // Hard amortization bound: the delta fed to the summarizer is bounded
        // by what aged out THIS round, not the whole history. Only the entries
        // between delta_start and split are rendered.
        let delta_len = plan.split - plan.delta_start;
        assert!(
            delta_len < h.len(),
            "the fold delta ({delta_len}) must be a strict subset of history ({})",
            h.len()
        );
    }

    /// First compaction has no prior summary → whole prefix is the delta.
    #[test]
    fn first_compaction_has_no_prior_summary() {
        let mut h = vec![user_text("genesis")];
        for i in 0..10 {
            h.push(user_text(&format!("u{i}")));
            h.push(model_text(&format!("m{i}")));
        }
        let plan = plan_fold(&h).expect("first fold planned");
        assert!(plan.prior_summary.is_none());
        assert_eq!(plan.delta_start, 0, "no prior summary → fold the whole prefix");
    }

    // ---- BOUNDEDNESS ----

    /// Simulate the install step of a compaction on a snapshot: returns the new
    /// history (rolling summary + kept window) using a deterministic stub
    /// summarizer. Mirrors `try_compact`'s install logic without the network.
    fn compact_install(history: &[Content], stub_summary: &str) -> Vec<Content> {
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
    /// stays bounded by `1 + keep_entries (+ orphan walk)` — it does NOT grow
    /// with N. We simulate N turns of growth, compacting whenever we exceed a
    /// small cap, and assert the post-compaction length never grows with N.
    #[test]
    fn synthetic_prefix_is_one_turn_and_history_bounded() {
        let keep_entries = KEEP_RECENT_TURNS * 2;
        // The bound: one summary turn + the kept window. The orphan-walk can
        // only ever extend the SUMMARIZED side (walk earlier), so the KEPT side
        // is at most keep_entries. Generous ceiling.
        let bound = 1 + keep_entries;

        let mut h: Vec<Content> = Vec::new();
        let mut summary_turns_after_compaction = Vec::new();
        for i in 0..500 {
            // Append one user/model pair per "turn".
            h.push(user_text(&format!("u{i}")));
            h.push(model_text(&format!("m{i}")));

            // Compact aggressively once we're a few turns over the window —
            // models the auto-compaction trigger firing repeatedly.
            if h.len() >= keep_entries + 4 {
                h = compact_install(&h, "rolling");
                // Exactly ONE summary turn at the head.
                let summary_count = h
                    .iter()
                    .filter(|c| extract_prior_summary(Some(c)).is_some())
                    .count();
                assert_eq!(
                    summary_count, 1,
                    "exactly one rolling summary turn after compaction (i={i})"
                );
                assert!(
                    extract_prior_summary(h.first()).is_some(),
                    "the single summary turn is at the head (i={i})"
                );
                summary_turns_after_compaction.push(h.len());
            }
        }

        // Every post-compaction length is within the fixed bound, independent
        // of how many turns (N=500) we processed.
        for (k, &len) in summary_turns_after_compaction.iter().enumerate() {
            assert!(
                len <= bound,
                "compacted history length {len} exceeded bound {bound} at compaction #{k}"
            );
        }
        // And the LAST compacted length is no bigger than the FIRST — flat, not
        // growing with N.
        let first = summary_turns_after_compaction.first().copied().unwrap();
        let last = summary_turns_after_compaction.last().copied().unwrap();
        assert!(
            last <= first,
            "compacted history must not grow with N (first={first}, last={last})"
        );
    }

    // ---- keep-window preservation (tool round-trips never summarized away) ----

    fn tool_heavy_history(rounds: usize) -> Vec<Content> {
        let mut h = vec![user_text("start")];
        for i in 0..rounds {
            let name = format!("view_file_{i}");
            h.push(model_call(&name));
            h.push(user_response(&name));
        }
        h
    }

    fn call_names(cs: &[Content]) -> std::collections::HashSet<String> {
        let mut s = std::collections::HashSet::new();
        for c in cs {
            for p in &c.parts {
                if let Part::FunctionCall { function_call, .. } = p {
                    s.insert(function_call.name.clone());
                }
            }
        }
        s
    }

    fn response_names(cs: &[Content]) -> std::collections::HashSet<String> {
        let mut s = std::collections::HashSet::new();
        for c in cs {
            for p in &c.parts {
                if let Part::FunctionResponse { function_response } = p {
                    s.insert(function_response.name.clone());
                }
            }
        }
        s
    }

    #[test]
    fn keep_slice_balanced_for_tool_heavy_history() {
        for rounds in 4..=20 {
            let h = tool_heavy_history(rounds);
            let split = pick_split(&h, KEEP_RECENT_TURNS);
            let kept = &h[split..];
            let calls = call_names(kept);
            let resps = response_names(kept);
            for r in &resps {
                assert!(
                    calls.contains(r),
                    "ORPHAN functionResponse {r:?} kept without its call (rounds={rounds}, split={split})"
                );
            }
            for c in &calls {
                assert!(
                    resps.contains(c),
                    "DANGLING functionCall {c:?} kept without its response (rounds={rounds}, split={split})"
                );
            }
        }
    }

    #[test]
    fn keep_slice_never_starts_with_orphan_function_response() {
        for rounds in 4..=20 {
            let h = tool_heavy_history(rounds);
            let split = pick_split(&h, KEEP_RECENT_TURNS);
            if split < h.len() {
                let first = &h[split];
                assert!(
                    !(matches!(first.role, ContentRole::User) && turn_is_function_response(first)),
                    "first kept message is an orphaned functionResponse (rounds={rounds}, split={split})"
                );
            }
        }
    }

    /// A tool round-trip that just aged out is summarized as a UNIT — the fold
    /// delta never splits a call from its response across the summary/keep seam
    /// (pick_split walks the seam earlier). Verified for the fold path: the kept
    /// window is balanced even when a prior summary occupies the head.
    #[test]
    fn fold_keep_window_balanced_with_prior_summary_head() {
        // summary turn + tool round-trips.
        let mut h = vec![summary_turn("prior")];
        for i in 0..12 {
            let name = format!("tool_{i}");
            h.push(model_call(&name));
            h.push(user_response(&name));
        }
        let plan = plan_fold(&h).expect("fold planned");
        let kept = &h[plan.split..];
        let calls = call_names(kept);
        let resps = response_names(kept);
        assert_eq!(calls, resps, "kept window must be a balanced set of tool pairs");
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
        let h: Vec<Content> = (0..12).map(|i| user_text(&format!("u{i}"))).collect();
        assert_eq!(pick_split(&h, 6), 0);
    }

    /// A head-only prefix (just the prior summary, nothing aged out yet) is a
    /// no-op fold — we must not re-summarize the summary into itself.
    #[test]
    fn no_op_fold_when_only_prior_summary_before_window() {
        // summary turn + exactly the keep window, nothing extra.
        let mut h = vec![summary_turn("prior")];
        for i in 0..6 {
            h.push(user_text(&format!("u{i}")));
            h.push(model_text(&format!("m{i}")));
        }
        // split lands at 1 (only the summary turn is before the window) → empty
        // delta → no fold.
        assert!(
            plan_fold(&h).is_none(),
            "a head-only prefix must not trigger a re-fold of the summary"
        );
    }
}
