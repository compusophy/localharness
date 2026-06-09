//! Context-window compaction — recency-weighted, incremental ("fold").
//!
//! When the running prompt-token count for a turn exceeds
//! `CapabilitiesConfig::compaction_threshold`, we shrink history. The model's
//! context is kept as:
//!
//! ```text
//! [ one rolling summary turn ]  ++  [ the recent raw keep-window ]
//! ```
//!
//! The strategy is an *amortized fold*, not a re-summarize-everything blob:
//!
//! 1. Keep the system instruction (it lives outside `history`).
//! 2. Keep the most-recent `KEEP_RECENT_TURNS` user/model turn pairs verbatim —
//!    function-call/response pairs are kept together (`pick_split`).
//! 3. Recognize the EXISTING rolling-summary turn at the head by its tag
//!    (`COMPACTION_TAG`). On every compaction after the first, that head turn
//!    already holds the distilled prior context.
//! 4. Produce `new_summary = summarize(prior_summary ++ newly_aged_turns)` —
//!    i.e. fold ONLY the turns that just aged out of the keep-window into the
//!    prior summary. The original raw turns were discarded at the first
//!    compaction and are NEVER re-summarized again.
//! 5. Replace the prefix with one synthetic rolling-summary turn and keep the
//!    recent raw window.
//!
//! Why a fold: each compaction re-summarizes only `(prior summary + delta)`,
//! which is bounded, rather than the whole growing history. Cheaper, lower
//! drift, and — crucially — the synthetic prefix stays ONE turn no matter how
//! long the conversation runs (the boundedness invariant; see
//! `synthetic_prefix_is_one_turn`).
//!
//! If summarization fails (network error, missing client) we fall back to
//! dropping the oldest turns (still preserving the prior summary if present).
//! The agent never errors out of a turn because of a compaction failure — the
//! dispatch loop logs at WARN and continues.
//!
//! ONE rolling tier ships here. A second, deeper "gist" tier (the literal
//! two-prior Fibonacci fold, `deep = summarize(deep, rolling)`) is a documented
//! follow-up: the single rolling tier already gives boundedness + amortization,
//! and a second tier adds folding-order subtleties that aren't worth the risk
//! until long-chat dogfooding shows the rolling summary itself growing too
//! large.

use parking_lot::Mutex;
use tracing::{debug, warn};

use crate::backends::gemini::api::SharedClient;
use crate::backends::gemini::wire::{
    Content, ContentRole, FinishReason, GenerateContentRequest, Part,
};
use crate::error::Result;

/// Tag prepended to the rolling-summary turn so the model (and humans
/// inspecting history) can tell what they're looking at — AND so the next
/// compaction can RECOGNIZE the prior summary and fold into it rather than
/// re-summarize it from scratch. The recognition is load-bearing; don't change
/// the tag without updating `extract_prior_summary`.
pub const COMPACTION_TAG: &str = "[compacted prior context]";

/// How many recent user/model turn pairs we always keep verbatim. The
/// model needs immediate context — a hard ceiling that's not too
/// stingy (don't break a multi-step tool use) and not too generous
/// (don't defeat the point of compaction).
const KEEP_RECENT_TURNS: usize = 6;

/// Pre-summary check: if history has fewer than this many entries
/// past the keep-window, compaction is a no-op (nothing to gain).
const MIN_HISTORY_TO_COMPACT: usize = 8;

/// Model used for summarization. Cheap + fast is right here — the
/// summary doesn't need to be brilliant, just faithful.
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
    /// Index such that `history[..split]` is folded away and `history[split..]`
    /// is kept verbatim.
    split: usize,
    /// The prior rolling summary recovered from a tagged head turn, if any. On
    /// the FIRST compaction this is `None`; on every later one it's `Some`.
    prior_summary: Option<String>,
    /// The turns that just aged out and must be folded into the prior summary.
    /// On the first compaction this is the whole `history[..split]`; afterwards
    /// it EXCLUDES the prior-summary turn — so the summarizer only ever sees
    /// `(prior summary + delta)`, never the original raw turns again.
    delta_start: usize,
}

/// Try to compact `history` in place. Returns `true` if anything
/// changed. Safe to call from inside the agent loop — never errors out,
/// only logs.
pub async fn try_compact(
    history: &Mutex<Vec<Content>>,
    client: &SharedClient,
    model: &str,
) -> bool {
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

    // Install the new rolling summary as a single synthetic user turn at the
    // head, replacing BOTH the prior summary turn (if any) and the aged-out
    // delta with one folded turn.
    let synthetic = Content {
        role: ContentRole::User,
        parts: vec![Part::Text {
            text: format!("{COMPACTION_TAG}\n{summary}"),
        }],
    };
    let mut hist = history.lock();
    if hist.len() != total {
        // Another turn raced us. Bail rather than corrupt the new state.
        warn!("compaction: history changed under us; aborting install");
        return false;
    }
    let kept: Vec<Content> = hist.split_off(plan.split);
    hist.clear();
    hist.push(synthetic);
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: installed folded summary");
    true
}

/// Compute the fold plan for `history`, or `None` if there's nothing worth
/// folding (the kept window already covers everything before the head).
///
/// Recognizes a prior rolling-summary turn at index 0 by `COMPACTION_TAG`. When
/// present, the delta to fold STARTS AFTER it (index 1), so the summarizer
/// receives `(prior summary text) + (only the newly-aged turns)` — never the
/// original raw turns, which were already folded into the prior summary. That
/// is the amortization: each compaction re-summarizes a bounded delta, not the
/// whole history.
fn plan_fold(history: &[Content]) -> Option<FoldPlan> {
    let split = pick_split(history, KEEP_RECENT_TURNS);
    if split == 0 {
        return None;
    }
    let prior_summary = extract_prior_summary(history.first());
    // If the head IS the prior summary, the delta is everything after it up to
    // the split. Otherwise the whole prefix is the delta (first compaction).
    let delta_start = if prior_summary.is_some() { 1 } else { 0 };
    // Guard: a head-only prefix (split == 1 with a prior summary) has an empty
    // delta — nothing new aged out, so re-folding would just re-summarize the
    // summary. Skip it; boundedness already holds (head is one turn).
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
fn extract_prior_summary(head: Option<&Content>) -> Option<String> {
    let c = head?;
    if !matches!(c.role, ContentRole::User) {
        return None;
    }
    // The synthetic turn is exactly one Text part beginning with the tag.
    let text = match c.parts.as_slice() {
        [Part::Text { text }] => text,
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

/// Pick an index `i` such that history[..i] is summarized and
/// history[i..] is kept. Honors `KEEP_RECENT_TURNS` (a turn = one
/// user+model pair, so 2 entries) and function-call/response pairing.
///
/// The kept slice `history[i..]` is API-valid iff its first message is NOT
/// a lone `functionResponse` user turn — otherwise the matching
/// `functionCall` (at `i-1`) would be summarized away and orphaned. (Linear
/// history guarantees a `functionCall` is always followed by its
/// `functionResponse`, so a kept *call* never dangles; only a leading
/// response can orphan.)
///
/// We start at the keep-window boundary and walk it EARLIER (toward 0) past
/// any leading `functionResponse`, absorbing the orphaned pair into the
/// summary. Walking earlier keeps strictly MORE history than requested
/// (never less) and can never run off the end — the old walk-FORWARD logic
/// could chain through a long run of tool round-trips and keep ZERO messages,
/// summarizing away the entire recent context including the turn being
/// answered.
fn pick_split(history: &[Content], keep_pairs: usize) -> usize {
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

/// True if keeping `history[split..]` would orphan a leading
/// `functionResponse`: the first kept message is a user turn of only
/// `functionResponse` parts, whose matching `functionCall` lives at
/// `split-1` (and would be summarized away).
fn is_leading_orphan(history: &[Content], split: usize) -> bool {
    match history.get(split) {
        Some(c) => matches!(c.role, ContentRole::User) && turn_is_function_response(c),
        None => false,
    }
}

fn turn_is_function_response(c: &Content) -> bool {
    c.parts
        .iter()
        .all(|p| matches!(p, Part::FunctionResponse { .. }))
        && !c.parts.is_empty()
}

/// Build the one-shot summarizer prompt for an incremental fold: the prior
/// rolling summary (if any) followed by the newly-aged delta transcript.
/// Pure + network-free so tests can assert the summarizer's INPUT contains
/// only `(prior summary + delta)`, proving the fold doesn't re-summarize the
/// whole history.
fn fold_prompt(prior_summary: Option<&str>, delta: &[Content]) -> String {
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
    delta: &[Content],
) -> Result<String> {
    use futures_util::stream::StreamExt;

    // Feed the fold prompt as a one-shot user message — no system
    // instruction, no tools, no history.
    let req = GenerateContentRequest {
        contents: vec![Content {
            role: ContentRole::User,
            parts: vec![Part::Text {
                text: fold_prompt(prior_summary, delta),
            }],
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
                    if let Part::Text { text } = part {
                        out.push_str(&text);
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

fn render_transcript(history: &[Content]) -> String {
    let mut out = String::with_capacity(history.len() * 64);
    for entry in history {
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
                Part::Thought {
                    text: Some(t), ..
                } => {
                    out.push_str("[thought] ");
                    out.push_str(t);
                }
                Part::FunctionCall { function_call } => {
                    out.push_str("[tool_call ");
                    out.push_str(&function_call.name);
                    out.push_str("] ");
                    out.push_str(&function_call.args.to_string());
                }
                Part::FunctionResponse { function_response } => {
                    out.push_str("[tool_result ");
                    out.push_str(&function_response.name);
                    out.push_str("] ");
                    let body = function_response.response.to_string();
                    // Truncate huge tool results at a CHAR boundary — the
                    // summarizer doesn't need every byte, and a blind
                    // `[..512]` panics mid multibyte char.
                    if body.len() > 512 {
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
                Part::InlineData { inline_data } => {
                    out.push_str("[inline_data ");
                    out.push_str(&inline_data.mime_type);
                    out.push(']');
                }
                _ => {}
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
    history: &Mutex<Vec<Content>>,
    split: usize,
    prior_summary: Option<&str>,
) -> bool {
    let mut hist = history.lock();
    if split >= hist.len() {
        return false;
    }
    let kept: Vec<Content> = hist.split_off(split);
    hist.clear();
    let text = match prior_summary {
        Some(s) if !s.trim().is_empty() => {
            format!("{COMPACTION_TAG}\n{s}\n[some prior turns dropped without summary]")
        }
        _ => format!("{COMPACTION_TAG}\n[prior turns dropped]"),
    };
    hist.push(Content {
        role: ContentRole::User,
        parts: vec![Part::Text { text }],
    });
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: drop-oldest fallback applied");
    true
}

/// Decide whether to attempt compaction based on the running token
/// count. `threshold` of `None` disables compaction entirely.
pub fn should_compact(total_tokens: Option<i32>, threshold: Option<u32>) -> bool {
    match (total_tokens, threshold) {
        (Some(t), Some(th)) => t as u32 > th,
        _ => false,
    }
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
                if let Part::FunctionCall { function_call } = p {
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
