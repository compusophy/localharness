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
    pub fn from_wire(steps: &[String], completed: &[i64]) -> Self {
        let steps: Vec<Step> = steps
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .take(MAX_STEPS)
            .enumerate()
            .map(|(i, s)| Step {
                text: truncate(s, MAX_STEP_LEN),
                done: completed.contains(&(i as i64)),
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
