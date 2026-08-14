//! Pure plan/checklist core — the agent's cross-turn memory of "what am I doing".
//!
//! Before this, an agent's ONLY record of a multi-phase objective was the raw
//! transcript, re-read on every auto-continue nudge. Worse, the system prompt
//! tells the model to "PLAN FIRST … post a SHORT plan in plain text" and promises
//! "you auto-continue after each step" — but a text-only turn classified as
//! `FinalAnswer` and BROKE the loop, so the agent posted its plan and silently
//! stopped at step one (telemetry #75/#69/#67).
//!
//! A plan with open steps is what lets `turn_flow::classify_turn` tell a
//! mid-plan narration turn ("here's what I'll do next") from a conversational
//! reply. Pure + native-tested; the browser holds one in a thread-local.

use serde::{Deserialize, Serialize};

/// Max steps in a plan. A plan is a working checklist, not a project tracker —
/// past ~a dozen the model is decomposing too finely to make progress.
pub const MAX_STEPS: usize = 12;
/// Max chars per step. Steps are labels ("wire the pointer routing"), not prose.
pub const MAX_STEP_LEN: usize = 120;

/// One checklist step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub text: String,
    pub done: bool,
}

/// An ordered checklist the agent maintains across turns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub steps: Vec<Step>,
}

impl Plan {
    /// Build from the wire shape: the full ordered step list plus the indices
    /// that are complete. Re-sent whole on every update (idempotent, so a
    /// dropped turn can't desync it) — empty `steps` clears the plan.
    ///
    /// Steps are trimmed, empties dropped, each truncated to `MAX_STEP_LEN` and
    /// the list to `MAX_STEPS`; out-of-range `completed` indices are ignored.
    ///
    /// ⛔ `completed` indexes the array the MODEL SENT, so blanks must be
    /// enumerated BEFORE they are dropped. Filtering first renumbered the
    /// survivors and silently shifted every index past a blank: `["a", "", "b"]`
    /// with `completed: [2]` checked off NOTHING (index 2 no longer existed)
    /// while the model believed "b" was done. Not cosmetic — an OPEN plan is
    /// what keeps a text-only turn from ending the run
    /// (`turn_flow::classify_turn`, telemetry #75/#69/#67), so a desynced
    /// checklist changes when the agent stops.
    pub fn from_wire(steps: &[String], completed: &[i64]) -> Self {
        let steps: Vec<Step> = steps
            .iter()
            .enumerate()
            .map(|(sent_idx, s)| (sent_idx, s.trim()))
            .filter(|(_, s)| !s.is_empty())
            .take(MAX_STEPS)
            .map(|(sent_idx, s)| Step {
                text: truncate(s, MAX_STEP_LEN),
                done: completed.contains(&(sent_idx as i64)),
            })
            .collect();
        Self { steps }
    }

    /// `(done, total)` — the "3/5" the user asked to see.
    pub fn progress(&self) -> (usize, usize) {
        (self.steps.iter().filter(|s| s.done).count(), self.steps.len())
    }

    /// Steps still open. `> 0` is what keeps the turn loop going through a
    /// text-only turn (see `turn_flow::classify_turn`).
    pub fn open(&self) -> usize {
        self.steps.iter().filter(|s| !s.done).count()
    }

    /// A plan with work left. An empty or fully-checked plan is NOT active — it
    /// must not hold the loop open past the last step.
    pub fn is_active(&self) -> bool {
        self.open() > 0
    }

    /// The first open step — what the agent should be doing right now.
    pub fn current(&self) -> Option<&str> {
        self.steps.iter().find(|s| !s.done).map(|s| s.text.as_str())
    }
}

/// Truncate on a char boundary (never panic on multi-byte input).
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(steps: &[&str], completed: &[i64]) -> Plan {
        Plan::from_wire(
            &steps.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            completed,
        )
    }

    #[test]
    fn progress_counts_completed_indices() {
        let p = wire(&["a", "b", "c", "d", "e"], &[0, 2]);
        assert_eq!(p.progress(), (2, 5));
        assert_eq!(p.open(), 3);
        assert!(p.is_active());
    }

    #[test]
    fn current_is_the_first_open_step() {
        let p = wire(&["design", "build", "ship"], &[0]);
        assert_eq!(p.current(), Some("build"));
    }

    /// The loop must NOT be held open by a finished or empty plan.
    #[test]
    fn fully_done_or_empty_plan_is_inactive() {
        assert!(!wire(&["a", "b"], &[0, 1]).is_active());
        assert!(!wire(&[], &[]).is_active());
        assert_eq!(wire(&[], &[]).current(), None);
    }

    /// REGRESSION: `completed` indexes the array the MODEL SENT, so dropping
    /// blanks before enumerating shifted every index past a blank. The old code
    /// renumbered survivors, so the checked step moved (or fell off the end)
    /// and the checklist silently disagreed with the model — which also decides
    /// whether the run keeps going, since an open plan holds the loop.
    #[test]
    fn completed_indices_are_positions_in_the_sent_array() {
        // sent: 0="a" 1=blank 2="b" 3=blank 4="c"; the model checked 2 and 4.
        // Blanks sit on BOTH sides of a checked index here on purpose.
        let p = wire(&["a", "", "b", "  ", "c"], &[2, 4]);
        assert_eq!(
            p.steps
                .iter()
                .map(|s| (s.text.as_str(), s.done))
                .collect::<Vec<_>>(),
            vec![("a", false), ("b", true), ("c", true)],
        );
        assert_eq!(p.progress(), (2, 3));
        assert_eq!(p.current(), Some("a"));

        // An index landing ON a dropped blank checks nothing — the step the
        // model marked did not survive, and no neighbour inherits its tick.
        assert_eq!(wire(&["a", "", "b"], &[1]).progress(), (0, 2));

        // Truncation still counts SENT positions: a leading blank must shift
        // neither the cap nor the indices ("s0" is sent index 1).
        let labels: Vec<String> = (0..MAX_STEPS + 3).map(|i| format!("s{i}")).collect();
        let mut many: Vec<&str> = vec![""];
        many.extend(labels.iter().map(|s| s.as_str()));
        let p = wire(&many, &[1]);
        assert_eq!(p.steps.len(), MAX_STEPS);
        assert_eq!(p.steps[0].text, "s0");
        assert!(p.steps[0].done, "sent index 1 is the first surviving step");

        // No-blank plans are unchanged — sent positions ARE survivor positions.
        let p = wire(&["a", "b", "c"], &[0, 2]);
        assert_eq!(p.progress(), (2, 3));
        assert_eq!(p.current(), Some("b"));
    }

    #[test]
    fn blank_steps_drop_and_text_trims() {
        let p = wire(&["  keep  ", "   ", ""], &[]);
        assert_eq!(p.steps.len(), 1);
        assert_eq!(p.steps[0].text, "keep");
    }

    #[test]
    fn steps_and_length_are_capped() {
        let many: Vec<&str> = vec!["step"; MAX_STEPS + 8];
        assert_eq!(wire(&many, &[]).steps.len(), MAX_STEPS);
        let long = "x".repeat(MAX_STEP_LEN + 50);
        let p = wire(&[&long], &[]);
        assert_eq!(p.steps[0].text.chars().count(), MAX_STEP_LEN);
    }

    /// A dropped/garbled update must not panic or mark the wrong step.
    #[test]
    fn out_of_range_completed_indices_are_ignored() {
        let p = wire(&["a", "b"], &[5, -1, 1]);
        assert_eq!(p.progress(), (1, 2));
        assert!(p.steps[1].done);
        assert!(!p.steps[0].done);
    }

    #[test]
    fn truncate_is_char_safe_on_multibyte() {
        let p = wire(&[&"é".repeat(MAX_STEP_LEN + 10)], &[]);
        assert_eq!(p.steps[0].text.chars().count(), MAX_STEP_LEN);
    }
}
