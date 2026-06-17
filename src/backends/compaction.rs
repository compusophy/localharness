//! Context-window compaction — recency-weighted, incremental ("fold") — the
//! ONE generic engine shared by the Gemini and Anthropic backends.
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
//!    tool-call/result pairs are kept together (`pick_split`).
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
//! `synthetic_prefix_is_one_turn_and_history_bounded` in the adapter tests).
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
//!
//! The engine is generic over [`CompactionModel`] — the small per-provider
//! seam covering wire-message shape (role test, sole-text extraction,
//! tool-result detection, user-turn construction, transcript rendering).
//! The provider's summarization REQUEST (its network call) is passed into
//! [`try_compact`] as a closure, so the engine stays client-agnostic. Each
//! backend keeps a thin `compaction.rs` adapter that implements the seam and
//! re-exports the engine's public surface, so existing paths
//! (`backends::{gemini,anthropic}::compaction::*`) keep working.

use std::future::Future;

use parking_lot::Mutex;
use tracing::{debug, warn};

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
pub(crate) const KEEP_RECENT_TURNS: usize = 6;

/// Pre-summary check: if history has fewer than this many entries
/// past the keep-window, compaction is a no-op (nothing to gain).
const MIN_HISTORY_TO_COMPACT: usize = 8;

/// The summarizer instruction. Cheap + fast is right here — the
/// summary doesn't need to be brilliant, just faithful.
const SUMMARY_PROMPT: &str = "You maintain a rolling summary of a long agent conversation. \
    Below is the PRIOR SUMMARY (already-distilled older context) followed by NEW TURNS that \
    just aged out of the live window. Produce an UPDATED rolling summary in 200 words or less \
    that folds the new turns into the prior summary: preserve key facts, decisions, file paths, \
    and any open user requests; drop greetings, chit-chat, and redundant tool output. Do not \
    lose information that was in the prior summary unless it is now obsolete. Output only the \
    updated summary; no preamble.";

/// The per-provider seam: everything the fold engine needs to know about a
/// backend's wire-message shape. Implemented on a zero-sized marker type in
/// each backend's `compaction.rs` adapter; the engine is monomorphized over
/// it (static dispatch — no trait objects, no async methods, so no
/// `MaybeSendSync`/`?Send` gymnastics are needed).
pub(crate) trait CompactionModel {
    /// The backend's wire turn type (Gemini `Content`, Anthropic `Message`).
    type Message: Clone;

    /// True iff the turn carries the user role.
    fn is_user(m: &Self::Message) -> bool;

    /// The text of a turn whose content is EXACTLY one text part, if so.
    /// (Role is checked separately — see `extract_prior_summary`.)
    fn sole_text(m: &Self::Message) -> Option<&str>;

    /// True iff the turn consists solely of tool-result parts (and is
    /// non-empty) — the shape whose matching tool CALL must not be
    /// summarized away from under it.
    fn is_tool_result_turn(m: &Self::Message) -> bool;

    /// Build a plain user-text turn (the synthetic rolling-summary turn).
    fn user_text(text: String) -> Self::Message;

    /// Append one turn's transcript rendering (role header + parts) to
    /// `out`, ending each part with a newline. Tool-result bodies go
    /// through [`push_truncated`].
    fn render_message(m: &Self::Message, out: &mut String);
}

/// The plan for a single fold, computed PURELY from a history snapshot — no
/// network, no client. Splitting this out makes the amortization + boundedness
/// guarantees unit-testable with a stub summarizer (see the adapter tests).
pub(crate) struct FoldPlan {
    /// Index such that `history[..split]` is folded away and `history[split..]`
    /// is kept verbatim.
    pub(crate) split: usize,
    /// The prior rolling summary recovered from a tagged head turn, if any. On
    /// the FIRST compaction this is `None`; on every later one it's `Some`.
    pub(crate) prior_summary: Option<String>,
    /// The turns that just aged out and must be folded into the prior summary.
    /// On the first compaction this is the whole `history[..split]`; afterwards
    /// it EXCLUDES the prior-summary turn — so the summarizer only ever sees
    /// `(prior summary + delta)`, never the original raw turns again.
    pub(crate) delta_start: usize,
}

/// Try to compact `history` in place. Returns `true` if anything
/// changed. Safe to call from inside the agent loop — never errors out,
/// only logs.
///
/// `summarize` is the provider's one-shot summarization request: it receives
/// the fully built fold prompt and returns the model's summary text.
pub(crate) async fn try_compact<A, F, Fut>(
    history: &Mutex<Vec<A::Message>>,
    summarize: F,
) -> bool
where
    A: CompactionModel,
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<String>>,
{
    let snapshot = history.lock().clone();
    let total = snapshot.len();
    if total < MIN_HISTORY_TO_COMPACT {
        debug!(total, "compaction: history too short, skipping");
        return false;
    }

    let plan = match plan_fold::<A>(&snapshot) {
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

    let prompt = fold_prompt::<A>(plan.prior_summary.as_deref(), delta);
    let summary = match summarize(prompt).await {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "compaction: summarization failed; folding to drop-oldest");
            return drop_oldest_fallback::<A>(history, plan.split, plan.prior_summary.as_deref());
        }
    };

    if summary.trim().is_empty() {
        warn!("compaction: summarization returned empty text; folding to drop-oldest");
        return drop_oldest_fallback::<A>(history, plan.split, plan.prior_summary.as_deref());
    }

    // Install the new rolling summary as a single synthetic user turn at the
    // head, replacing BOTH the prior summary turn (if any) and the aged-out
    // delta with one folded turn.
    let synthetic = A::user_text(format!("{COMPACTION_TAG}\n{summary}"));
    let mut hist = history.lock();
    if hist.len() != total {
        // Another turn raced us. Bail rather than corrupt the new state.
        warn!("compaction: history changed under us; aborting install");
        return false;
    }
    let kept: Vec<A::Message> = hist.split_off(plan.split);
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
pub(crate) fn plan_fold<A: CompactionModel>(history: &[A::Message]) -> Option<FoldPlan> {
    let split = pick_split::<A>(history, KEEP_RECENT_TURNS);
    if split == 0 {
        return None;
    }
    let prior_summary = extract_prior_summary::<A>(history.first());
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
pub(crate) fn extract_prior_summary<A: CompactionModel>(
    head: Option<&A::Message>,
) -> Option<String> {
    let m = head?;
    if !A::is_user(m) {
        return None;
    }
    // The synthetic turn is exactly one text part beginning with the tag.
    let text = A::sole_text(m)?;
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
/// user+model pair, so 2 entries) and tool-call/result pairing.
///
/// The kept slice `history[i..]` is API-valid iff its first message is NOT
/// a lone tool-result user turn — otherwise the matching tool CALL (at
/// `i-1`) would be summarized away and orphaned. (Linear history guarantees
/// a tool call is always followed by its result, so a kept *call* never
/// dangles; only a leading result can orphan.)
///
/// We start at the keep-window boundary and walk it EARLIER (toward 0) past
/// any leading tool result, absorbing the orphaned pair into the
/// summary. Walking earlier keeps strictly MORE history than requested
/// (never less) and can never run off the end — the old walk-FORWARD logic
/// could chain through a long run of tool round-trips and keep ZERO messages,
/// summarizing away the entire recent context including the turn being
/// answered.
pub(crate) fn pick_split<A: CompactionModel>(history: &[A::Message], keep_pairs: usize) -> usize {
    let keep_entries = keep_pairs * 2;
    if history.len() <= keep_entries {
        return 0;
    }
    let mut split = history.len() - keep_entries;

    while split > 0 && is_leading_orphan::<A>(history, split) {
        split -= 1;
    }
    split
}

/// True if keeping `history[split..]` would orphan a leading tool result:
/// the first kept message is a tool-result turn whose matching tool call lives
/// at `split-1` (and would be summarized away).
///
/// Tests `is_tool_result_turn` ALONE — NOT `is_user && is_tool_result_turn`. On
/// Anthropic/Gemini a tool-result turn IS a user turn, so the two are equivalent;
/// but on OpenAI a tool result is its OWN `role:"tool"` message (never user), so
/// the `is_user` conjunct made this ALWAYS FALSE there — and a parallel-tool-call
/// run split mid-way left orphaned leading `tool` messages, which OpenAI 400s
/// ("messages with role 'tool' must be a response to a preceding 'tool_calls'"),
/// bricking the next request after compaction. The walk steps back through the
/// whole run of leading tool results to the assistant turn that issued them.
fn is_leading_orphan<A: CompactionModel>(history: &[A::Message], split: usize) -> bool {
    match history.get(split) {
        Some(m) => A::is_tool_result_turn(m),
        None => false,
    }
}

/// Build the one-shot summarizer prompt for an incremental fold: the prior
/// rolling summary (if any) followed by the newly-aged delta transcript.
/// Pure + network-free so tests can assert the summarizer's INPUT contains
/// only `(prior summary + delta)`, proving the fold doesn't re-summarize the
/// whole history.
pub(crate) fn fold_prompt<A: CompactionModel>(
    prior_summary: Option<&str>,
    delta: &[A::Message],
) -> String {
    let mut body = String::new();
    body.push_str(SUMMARY_PROMPT);
    body.push_str("\n\n--- PRIOR SUMMARY ---\n");
    match prior_summary {
        Some(s) if !s.trim().is_empty() => body.push_str(s),
        _ => body.push_str("(none — this is the first compaction)"),
    }
    body.push_str("\n\n--- NEW TURNS ---\n");
    body.push_str(&render_transcript::<A>(delta));
    body
}

/// Render `history` as the plain-text transcript fed to the summarizer:
/// each turn via [`CompactionModel::render_message`], blank-line separated.
pub(crate) fn render_transcript<A: CompactionModel>(history: &[A::Message]) -> String {
    let mut out = String::with_capacity(history.len() * 64);
    for entry in history {
        A::render_message(entry, &mut out);
        out.push('\n');
    }
    out
}

/// Append `body` to `out`, truncating past 512 bytes at a CHAR boundary —
/// the summarizer doesn't need every byte of a huge tool result, and a blind
/// `[..512]` panics when byte 512 lands inside a multibyte UTF-8 char.
pub(crate) fn push_truncated(out: &mut String, body: &str) {
    if body.len() > 512 {
        let mut end = 512;
        while end > 0 && !body.is_char_boundary(end) {
            end -= 1;
        }
        out.push_str(&body[..end]);
        out.push_str("…[truncated]");
    } else {
        out.push_str(body);
    }
}

/// Last-resort fallback when summarization isn't available. Drops the aged-out
/// delta but PRESERVES the prior rolling summary if there is one — so a network
/// blip doesn't throw away every prior fold. The head stays one turn either
/// way, so boundedness still holds.
fn drop_oldest_fallback<A: CompactionModel>(
    history: &Mutex<Vec<A::Message>>,
    split: usize,
    prior_summary: Option<&str>,
) -> bool {
    let mut hist = history.lock();
    if split >= hist.len() {
        return false;
    }
    let kept: Vec<A::Message> = hist.split_off(split);
    hist.clear();
    let text = match prior_summary {
        Some(s) if !s.trim().is_empty() => {
            format!("{COMPACTION_TAG}\n{s}\n[some prior turns dropped without summary]")
        }
        _ => format!("{COMPACTION_TAG}\n[prior turns dropped]"),
    };
    hist.push(A::user_text(text));
    hist.extend(kept);
    debug!(new_len = hist.len(), "compaction: drop-oldest fallback applied");
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // A mock backend with OpenAI-style DISTINCT roles: a tool result is its OWN
    // role, NOT a user turn (unlike Anthropic/Gemini, where a tool-result turn IS
    // a user turn). This is the shape that exposed the orphan bug.
    #[derive(Clone, PartialEq, Debug)]
    enum Role {
        User,
        Assistant,
        Tool,
    }
    #[derive(Clone)]
    struct MockMsg {
        role: Role,
        text: String,
    }
    fn m(role: Role) -> MockMsg {
        MockMsg { role, text: String::new() }
    }
    struct MockModel;
    impl CompactionModel for MockModel {
        type Message = MockMsg;
        fn is_user(m: &MockMsg) -> bool {
            m.role == Role::User
        }
        fn sole_text(m: &MockMsg) -> Option<&str> {
            (!m.text.is_empty()).then_some(m.text.as_str())
        }
        fn is_tool_result_turn(m: &MockMsg) -> bool {
            m.role == Role::Tool
        }
        fn user_text(text: String) -> MockMsg {
            MockMsg { role: Role::User, text }
        }
        fn render_message(m: &MockMsg, out: &mut String) {
            out.push_str(&m.text);
            out.push('\n');
        }
    }

    #[test]
    fn pick_split_walks_back_past_a_leading_tool_run_when_roles_are_distinct() {
        // OpenAI parallel tool calls: one assistant turn issues N calls, followed
        // by N SEPARATE `role:tool` messages. A keep-window boundary landing
        // mid-run must walk back to the issuing assistant — never leave an
        // orphaned leading tool result (OpenAI 400s: "role 'tool' must follow
        // 'tool_calls'"). Before the fix, `is_user && is_tool_result` was always
        // false for distinct roles, so the walk-back never fired → orphan.
        let h = vec![
            m(Role::User),
            m(Role::Assistant),
            m(Role::Tool), m(Role::Tool), m(Role::Tool), m(Role::Tool), // call_0 ×4
            m(Role::Assistant),
            m(Role::Tool), m(Role::Tool), m(Role::Tool), m(Role::Tool), // call_1 ×4
            m(Role::Assistant),
            m(Role::Tool), m(Role::Tool), m(Role::Tool), m(Role::Tool), // call_2 ×4
        ]; // len 16
        // keep_pairs=6 → keep_entries=12 → raw split = 16-12 = 4, which is a Tool.
        let split = pick_split::<MockModel>(&h, 6);
        assert!(
            !MockModel::is_tool_result_turn(&h[split]),
            "kept head at split={split} must not be an orphaned tool result"
        );
        assert_eq!(h[split].role, Role::Assistant, "walks back to the issuing assistant turn");
    }

    #[test]
    fn pick_split_keeps_a_clean_user_boundary_as_is() {
        // A boundary on a normal (non-tool) user turn is kept verbatim — the
        // walk-back must NOT over-trigger on ordinary history.
        let h = vec![
            m(Role::User), m(Role::Assistant),
            m(Role::User), m(Role::Assistant),
            m(Role::User), m(Role::Assistant),
        ]; // len 6
        let split = pick_split::<MockModel>(&h, 1); // keep_entries=2 → split=4 (a User turn)
        assert_eq!(split, 4);
        assert_eq!(h[split].role, Role::User);
    }
}

/// Decide whether to attempt compaction based on the running token
/// count. `threshold` of `None` disables compaction entirely.
pub fn should_compact(total_tokens: Option<i32>, threshold: Option<u32>) -> bool {
    match (total_tokens, threshold) {
        (Some(t), Some(th)) => t as u32 > th,
        _ => false,
    }
}
