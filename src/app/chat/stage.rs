//! Per-turn stage-line painter: a thin DOM shim over the pure
//! [`crate::turn_stage::StagePipeline`] (GitHub #19 — the $LH-payment →
//! execution transition was an opaque pause). ONE swap target per turn
//! (`#stage-{turn_id}`, the first child of the pending assistant body);
//! [`enter`] repaints it only when the pipeline actually changed, and
//! [`end`] empties it when the turn completes — the final text / error
//! stays, the pipeline line disappears.

use std::cell::RefCell;

use crate::turn_stage::{Stage, StagePipeline};

use super::super::{dom, templates};

thread_local! {
    /// The in-flight turn's pipeline. Reset by [`begin`] / [`end`]; only one
    /// turn streams at a time (`TURN_ACTIVE` in `chat::mod`), so one slot.
    static PIPELINE: RefCell<StagePipeline> = RefCell::new(StagePipeline::new());
    /// The swap target id (`stage-{turn_id}`) for the in-flight turn, or
    /// `None` between turns — [`enter`] is a no-op then.
    static TARGET: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Arm the painter for a fresh turn: empty pipeline, swap target
/// `#stage-{turn_id}` (the container is already in the turn's body —
/// `templates::stage_container`). Nothing paints until the first [`enter`].
pub(crate) fn begin(turn_id: u32) {
    PIPELINE.with(|p| *p.borrow_mut() = StagePipeline::new());
    TARGET.with(|t| *t.borrow_mut() = Some(format!("stage-{turn_id}")));
}

/// Record that `stage` is happening NOW and repaint the line iff it changed
/// (first entry of a word, or the current pointer walking across the trail).
/// No-op when no turn is armed.
pub(crate) fn enter(stage: Stage) {
    let Some(target) = TARGET.with(|t| t.borrow().clone()) else {
        return;
    };
    let changed = PIPELINE.with(|p| p.borrow_mut().enter(stage));
    if changed {
        let slots = PIPELINE.with(|p| p.borrow().slots());
        dom::swap_inner(&target, &templates::stage_line(&slots).into_string());
    }
}

/// The turn completed (done, errored, or cancelled): empty the line —
/// `.stage-line:empty` hides it — and disarm. Idempotent; called from
/// `mark_turn_done` and (belt-and-braces) the run's `TurnGuard`.
pub(crate) fn end() {
    if let Some(target) = TARGET.with(|t| t.borrow_mut().take()) {
        dom::swap_inner(&target, "");
    }
    PIPELINE.with(|p| *p.borrow_mut() = StagePipeline::new());
}
