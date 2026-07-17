//! The live `update_plan` checklist for the current session.
//!
//! Thread-local (the browser is single-threaded) and deliberately NOT persisted:
//! a plan is scoped to the objective in flight, not to the agent's identity. Its
//! one structural job is telling `turn_flow::classify_turn` that a text-only
//! turn is mid-plan narration rather than a conversational goodbye — see
//! `crate::plan`.

use std::cell::RefCell;

use crate::plan::Plan;

thread_local! {
    static ACTIVE: RefCell<Option<Plan>> = const { RefCell::new(None) };
}

/// Replace the plan (`update_plan` is idempotent — the model re-sends the whole
/// list). An empty step list clears it.
pub(crate) fn set(plan: Plan) {
    ACTIVE.with(|p| {
        *p.borrow_mut() = if plan.steps.is_empty() { None } else { Some(plan) };
    });
}

/// Does a plan with OPEN steps exist? This is the signal that keeps the turn
/// loop alive through a text-only planning turn.
pub(crate) fn is_active() -> bool {
    ACTIVE.with(|p| p.borrow().as_ref().is_some_and(|plan| plan.is_active()))
}

/// Drop the plan. Called when the model calls `finish` and when the context is
/// cleared — a stale plan must never hold a later turn open.
pub(crate) fn clear() {
    ACTIVE.with(|p| *p.borrow_mut() = None);
}
