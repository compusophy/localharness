//! Pure turn-outcome classification for the in-tab agent's continuous-execution
//! loop — native-testable, no DOM, no state, no async.
//!
//! These were inlined in `app::chat` (a 4k-line file), which only compiles
//! under `browser-app` + wasm32 — so their guard tests (including the
//! loop-termination invariant) were dead under a native `cargo test`. Hoisting
//! them to the crate root — alongside `encoding`, `raster`, and `compose` —
//! makes them real, native-run unit tests. Same pattern as `src/encoding.rs`;
//! behavior is unchanged.

/// Upper bound on automatic "continue toward the goal" turns per single
/// user message. A safety cap so a confused model can't loop forever
/// (and to bound credit spend). The user can always send again to extend.
pub const MAX_AUTO_CONTINUATIONS: u32 = 10;

/// How a single streamed turn ended — drives the continuous-execution loop.
#[derive(Debug, PartialEq, Eq)]
pub enum TurnOutcome {
    /// The model called `finish` — task explicitly complete. Stop.
    Finished,
    /// The turn ended on a final text answer with no tool activity this
    /// turn (plain conversation / a closing reply / a question). Stop —
    /// don't spam empty auto-continues on a chat reply.
    FinalAnswer,
    /// The turn performed tool actions and ended WITHOUT a completion
    /// signal — the model likely stopped mid-goal. Auto-continue.
    Incomplete,
    /// Nothing visible was produced, but the turn was TRUNCATED mid-answer
    /// (max-tokens / all reasoning, no final text). The model isn't done —
    /// retry with a "finish concisely" nudge, bounded like `Incomplete`.
    EmptyTruncated,
    /// Nothing visible was produced for a terminal reason (genuinely blank,
    /// safety-blocked, or a credits problem). Stop.
    Empty,
    /// The turn errored (already surfaced in the transcript). Stop.
    Error,
    /// The user hit stop mid-turn. Stop.
    Cancelled,
}

/// Why a turn produced no visible output — drives the message shown and
/// whether the turn is retryable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyKind {
    /// The model hit its output-token limit mid-answer (finish-reason
    /// MAX_TOKENS, or it streamed only reasoning and never reached a final
    /// text part). RETRYABLE — continue toward an answer.
    Truncated,
    /// A safety / content filter blocked the response. Terminal.
    Blocked,
    /// Genuinely nothing — no reasoning, no finish-reason note. Most often a
    /// credits/session problem (the proxy returned an empty body) or a stray
    /// blank from the model. Terminal.
    Blank,
}

/// Classify a no-visible-output turn from the model's terminal finish-reason
/// note (`ChatResponse::finish_note`) plus whether ANY reasoning streamed.
/// Pure → unit-testable without a browser.
///
/// - A MAX_TOKENS finish-reason → `Truncated` (ran out of room mid-answer).
/// - A CONTENT-block finish-reason → `Blocked`: safety / blocklist / prohibited
///   / refusal (Anthropic) / content filter (OpenAI) / RECITATION (Gemini stops
///   to avoid reproducing training data). These are terminal — retrying won't
///   help — and the remedy is to rephrase, NOT "check your balance".
/// - No note but the model DID reason (`saw_thinking`) → `Truncated`: it spent
///   the whole window thinking and never emitted a final text part (the exact
///   "(empty response) on hard tasks" bug; some paths drop the finish-reason).
/// - Otherwise → `Blank` (likely credits/session, or a stray empty reply).
pub fn classify_empty(finish_note: Option<&str>, saw_thinking: bool) -> EmptyKind {
    let note = finish_note.unwrap_or("").to_lowercase();
    if note.contains("max token") {
        EmptyKind::Truncated
    } else if note.contains("safety")
        || note.contains("blocklist")
        || note.contains("prohibited")
        || note.contains("refusal")
        || note.contains("recitation")
        || note.contains("content filter")
    {
        // A content block is terminal even if the model reasoned first — checked
        // BEFORE `saw_thinking` so a recitation/filter stop isn't mis-retried as
        // `Truncated`.
        EmptyKind::Blocked
    } else if saw_thinking {
        // Reasoned but produced no answer: budget-starved mid-thought.
        EmptyKind::Truncated
    } else {
        EmptyKind::Blank
    }
}

/// User-facing message for a terminal empty turn (Truncated is retried, so it
/// has no message — see `stream_turn`). Names the likely cause + the remedy,
/// per the on-chain feedback asking for "more informative error logging".
pub fn empty_message(kind: EmptyKind) -> &'static str {
    match kind {
        // Shown only if a Truncated turn somehow reaches the cap without an
        // answer; the remedy is to decompose the task.
        EmptyKind::Truncated => {
            "(the request was too large to finish in one step — try breaking it \
             into smaller asks.)"
        }
        EmptyKind::Blocked => {
            "(the model stopped this response under its safety filter. Try \
             rephrasing the request.)"
        }
        EmptyKind::Blank => {
            "(empty response — the model returned no text. If you're on platform \
             credits, check your session/balance in the account tab.)"
        }
    }
}

/// Decide how a completed (non-cancelled) turn ended, for the
/// continuous-execution loop. Pure over the signals tracked while
/// streaming so it can be unit-tested without a browser:
/// - `saw_finish`: the model called `finish` → task complete, stop.
/// - `saw_question`: the model called `ask_question` → it's blocking on the
///   user, stop and wait (do NOT auto-continue — that would spam the model
///   and never let the user answer).
/// - `saw_tool_call`: a goal-step tool ran (NOT finish / ask_question).
/// - `any_visible`: anything (text or a tool block) was rendered.
/// - `retryable_empty`: the turn was empty BECAUSE it was truncated mid-answer
///   (max tokens / all-thinking) → `EmptyTruncated`, which the loop retries.
///
/// Precedence: `finish` wins over everything (the model can call other tools
/// then `finish` in the same turn — that's still "done"). A blocking question
/// stops next. Then empty turns — a TRUNCATED empty retries (`EmptyTruncated`),
/// any other empty stops (`Empty`). Then a goal-step-only turn auto-continues
/// (`Incomplete`). A pure text reply with no tool activity is a `FinalAnswer`.
pub fn classify_turn(
    saw_finish: bool,
    saw_question: bool,
    saw_tool_call: bool,
    any_visible: bool,
    retryable_empty: bool,
) -> TurnOutcome {
    if saw_finish {
        TurnOutcome::Finished
    } else if saw_question {
        // Blocking on the user — a conversational stop, like FinalAnswer.
        // Never auto-continue (the default ask_question returns "skipped",
        // so a continue would loop the model 10x without a real answer).
        TurnOutcome::FinalAnswer
    } else if !any_visible {
        // No output. If it was truncated mid-answer (model ran out of budget
        // while reasoning), retry toward an answer; otherwise it's a real
        // dead-end (blank / safety / credits).
        if retryable_empty {
            TurnOutcome::EmptyTruncated
        } else {
            TurnOutcome::Empty
        }
    } else if saw_tool_call {
        // Ended right after tool activity with no explicit completion —
        // the model probably has more to do. Auto-continue.
        TurnOutcome::Incomplete
    } else {
        // Pure text reply, no tool calls — a conversational answer or a
        // question. Don't auto-continue (would spam empty turns).
        TurnOutcome::FinalAnswer
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_empty, classify_turn, EmptyKind, TurnOutcome, MAX_AUTO_CONTINUATIONS};

    // --- Turn classification (the continuous-execution loop's decision) -----
    //
    // `classify_turn(saw_finish, saw_question, saw_tool_call, any_visible,
    // retryable_empty)`. `retryable_empty` only matters when `any_visible` is
    // false; it's `false` for every visible-output case below.

    #[test]
    fn finish_wins_over_everything() {
        // finish + a goal-step tool in the same turn is still "done".
        assert_eq!(
            classify_turn(true, false, true, true, false),
            TurnOutcome::Finished
        );
        // finish alone.
        assert_eq!(
            classify_turn(true, false, false, true, false),
            TurnOutcome::Finished
        );
        // finish even alongside a question.
        assert_eq!(
            classify_turn(true, true, true, true, false),
            TurnOutcome::Finished
        );
    }

    #[test]
    fn ask_question_stops_the_loop_not_incomplete() {
        // REGRESSION: a blocking ask_question used to be read as a goal step
        // (saw_tool_call) → Incomplete → auto-continue, spamming the model and
        // never letting the user answer. It must stop like a FinalAnswer.
        assert_eq!(
            classify_turn(false, true, false, true, false),
            TurnOutcome::FinalAnswer
        );
        // A question accompanied by some other goal-step tool still stops:
        // the question is the blocking signal.
        assert_eq!(
            classify_turn(false, true, true, true, false),
            TurnOutcome::FinalAnswer
        );
    }

    #[test]
    fn goal_step_tool_only_auto_continues() {
        assert_eq!(
            classify_turn(false, false, true, true, false),
            TurnOutcome::Incomplete
        );
    }

    /// REGRESSION (on-chain feedback #100/#101): backends INTERCEPT `finish`
    /// and never emit it as a ToolCall chunk, so the in-tab loop reads it off
    /// `ChatResponse::finished()` and feeds it as `saw_finish`. These pin the
    /// loop-control invariant the app relies on now that the signal flows:
    /// a finished turn ALWAYS stops (no redundant auto-continue sign-off,
    /// #100), and a finish-with-no-text turn is `Finished` — NOT `Empty` —
    /// so it never paints an "(empty response)" bubble (#101).
    #[test]
    fn finish_stops_the_loop_in_every_shape() {
        // finish + text → stop (a normal closing turn).
        assert_eq!(
            classify_turn(true, false, false, true, false),
            TurnOutcome::Finished
        );
        // finish after a goal-step tool, no closing text → stop, NOT
        // Incomplete (the #100 root cause: an undetected finish here used to
        // auto-continue with the "continue toward the goal" nudge).
        assert_eq!(
            classify_turn(true, false, true, true, false),
            TurnOutcome::Finished
        );
        // BARE finish — no text, no other tool, nothing visible → Finished,
        // NOT Empty (the #101 root cause: a missed finish dead-ended as a
        // "(empty response)" bubble).
        assert_eq!(
            classify_turn(true, false, false, false, false),
            TurnOutcome::Finished
        );
    }

    /// The contrast case to `finish_stops_the_loop_in_every_shape`: a turn that
    /// ran a goal-step tool but did NOT call finish keeps going. This is the
    /// behavior that must be PRESERVED — the finish fix can't kill legitimate
    /// multi-step auto-continuation.
    #[test]
    fn pure_tool_without_finish_continues() {
        assert_eq!(
            classify_turn(false, false, true, true, false),
            TurnOutcome::Incomplete
        );
    }

    #[test]
    fn pure_text_reply_is_final_answer() {
        assert_eq!(
            classify_turn(false, false, false, true, false),
            TurnOutcome::FinalAnswer
        );
    }

    #[test]
    fn nothing_visible_is_empty() {
        assert_eq!(
            classify_turn(false, false, false, false, false),
            TurnOutcome::Empty
        );
        // No-visible takes precedence over a stray tool flag (can't have run a
        // tool with nothing rendered, but the ordering must be deterministic).
        assert_eq!(
            classify_turn(false, false, true, false, false),
            TurnOutcome::Empty
        );
    }

    /// A TRUNCATED empty turn (model ran out of output budget mid-answer) is
    /// `EmptyTruncated` — RETRYABLE — not a flat `Empty` dead-end. This is the
    /// core fix for "(empty response) on hard tasks".
    #[test]
    fn truncated_empty_is_retryable_not_dead_end() {
        assert_eq!(
            classify_turn(false, false, false, false, true),
            TurnOutcome::EmptyTruncated
        );
        // finish/question still win over a truncated-empty flag (defensive —
        // can't really co-occur, but precedence must be deterministic).
        assert_eq!(
            classify_turn(true, false, false, false, true),
            TurnOutcome::Finished
        );
    }

    // --- Empty-turn cause classification (drives message + retry) -----------

    #[test]
    fn classify_empty_max_tokens_note_is_truncated() {
        // The finish-reason note from either backend ("stopped at max tokens").
        assert_eq!(
            classify_empty(Some("stopped at max tokens"), false),
            EmptyKind::Truncated
        );
    }

    #[test]
    fn classify_empty_all_thinking_no_note_is_truncated() {
        // The exact bug: the model reasoned the whole budget away and emitted no
        // final text, and the finish-reason wasn't surfaced — `saw_thinking`
        // alone classifies it as truncated (so it's retried).
        assert_eq!(classify_empty(None, true), EmptyKind::Truncated);
    }

    #[test]
    fn classify_empty_safety_note_is_blocked() {
        assert_eq!(
            classify_empty(Some("stopped by safety policy"), true),
            EmptyKind::Blocked
        );
        assert_eq!(
            classify_empty(Some("stopped by blocklist"), false),
            EmptyKind::Blocked
        );
        assert_eq!(
            classify_empty(Some("stopped by refusal"), false),
            EmptyKind::Blocked
        );
    }

    /// REGRESSION: Gemini RECITATION (`"stopped to avoid recitation"`) and OpenAI
    /// `"stopped by content filter"` are CONTENT blocks — they were falling
    /// through to `Blank`/`Truncated`, so the user saw "(check your balance)" or
    /// the turn was uselessly retried. They must be `Blocked` (terminal,
    /// rephrase), even when the model reasoned first.
    #[test]
    fn classify_empty_recitation_and_content_filter_are_blocked() {
        assert_eq!(
            classify_empty(Some("stopped to avoid recitation"), false),
            EmptyKind::Blocked
        );
        // Even with prior reasoning — a content block is NOT retryable.
        assert_eq!(
            classify_empty(Some("stopped to avoid recitation"), true),
            EmptyKind::Blocked
        );
        assert_eq!(
            classify_empty(Some("stopped by content filter"), false),
            EmptyKind::Blocked
        );
    }

    #[test]
    fn classify_empty_nothing_at_all_is_blank() {
        // No note, no thinking → a genuine blank (likely credits/session).
        assert_eq!(classify_empty(None, false), EmptyKind::Blank);
        assert_eq!(classify_empty(Some(""), false), EmptyKind::Blank);
    }

    /// The outcomes that auto-continue are `Incomplete` and `EmptyTruncated`.
    /// This guards the loop-termination invariant: every OTHER classification
    /// breaks the continuous-execution loop, so the loop can only spin via these
    /// two — both hard-bounded by the SAME `MAX_AUTO_CONTINUATIONS` counter.
    #[test]
    fn only_incomplete_or_truncated_continues() {
        let continues =
            |o: TurnOutcome| o == TurnOutcome::Incomplete || o == TurnOutcome::EmptyTruncated;
        assert!(!continues(classify_turn(true, false, false, true, false))); // Finished
        assert!(!continues(classify_turn(false, true, false, true, false))); // FinalAnswer (question)
        assert!(!continues(classify_turn(false, false, false, true, false))); // FinalAnswer (text)
        assert!(!continues(classify_turn(false, false, false, false, false))); // Empty (blank)
        assert!(continues(classify_turn(false, false, true, true, false))); // Incomplete
        assert!(continues(classify_turn(false, false, false, false, true))); // EmptyTruncated
    }

    /// Mirrors the loop's increment/break: an always-`Incomplete` turn can fire
    /// at most `MAX_AUTO_CONTINUATIONS` auto-continuations, then the cap stops
    /// it. Proves no infinite loop when a confused model never finishes.
    #[test]
    fn auto_continuation_is_bounded() {
        let mut auto: u32 = 0;
        let mut iterations = 0u32;
        loop {
            iterations += 1;
            // Always Incomplete (the worst case for the loop).
            if matches!(
                classify_turn(false, false, true, true, false),
                TurnOutcome::Incomplete
            ) {
                if auto >= MAX_AUTO_CONTINUATIONS {
                    break;
                }
                auto += 1;
            } else {
                break;
            }
            // Safety net for the test itself.
            assert!(iterations < MAX_AUTO_CONTINUATIONS + 5, "loop did not terminate");
        }
        // First turn + MAX_AUTO_CONTINUATIONS continuations, then the cap break.
        assert_eq!(auto, MAX_AUTO_CONTINUATIONS);
        assert_eq!(iterations, MAX_AUTO_CONTINUATIONS + 1);
    }

    /// A repeatedly-truncated turn (`EmptyTruncated` every time — the model
    /// keeps running out of budget) must ALSO terminate, bounded by the same
    /// `MAX_AUTO_CONTINUATIONS` cap. Proves the retry path can't loop forever.
    #[test]
    fn truncated_retry_is_bounded() {
        let mut auto: u32 = 0;
        let mut iterations = 0u32;
        loop {
            iterations += 1;
            if matches!(
                classify_turn(false, false, false, false, true),
                TurnOutcome::EmptyTruncated
            ) {
                if auto >= MAX_AUTO_CONTINUATIONS {
                    break;
                }
                auto += 1;
            } else {
                break;
            }
            assert!(iterations < MAX_AUTO_CONTINUATIONS + 5, "truncated retry did not terminate");
        }
        assert_eq!(auto, MAX_AUTO_CONTINUATIONS);
        assert_eq!(iterations, MAX_AUTO_CONTINUATIONS + 1);
    }
}
