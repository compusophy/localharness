//! Turn-status indicator painter: a thin DOM shim over the pure
//! [`crate::turn_stage::StagePipeline`] (GitHub #19 — the $LH-payment →
//! execution transition was an opaque pause). The active phase surfaces as
//! ONE pulsing lucide glyph in a FIXED header slot (`#turn-status`, right of
//! the brand) — brain (thinking) / waves (streaming) / wrench (tools). The
//! slot is empty (button GONE) whenever no phase is active or between turns;
//! [`enter`] repaints only when the pipeline's current stage changes, and
//! [`end`] empties it when the turn completes.

use std::cell::{Cell, RefCell};

use crate::turn_stage::{Stage, StagePipeline};

use super::super::{dom, templates};

/// The fixed header slot id the active-phase glyph paints into (see
/// `templates::site_header` / `templates::stage_status_slot`). ONE slot,
/// reused every turn — no per-turn id.
const SLOT: &str = "turn-status";

thread_local! {
    /// The in-flight turn's pipeline. Reset by [`begin`] / [`end`]; only one
    /// turn streams at a time (`TURN_ACTIVE` in `chat::mod`), so one slot.
    static PIPELINE: RefCell<StagePipeline> = RefCell::new(StagePipeline::new());
    /// True between [`begin`] and [`end`] — [`enter`] is a no-op otherwise, so
    /// a stray stage event between turns never paints the header.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Arm the painter for a fresh turn: empty pipeline, empty slot. Nothing is
/// painted yet (no phase has happened), so the header button stays GONE until
/// the first thinking/streaming/tools event.
pub(crate) fn begin() {
    PIPELINE.with(|p| *p.borrow_mut() = StagePipeline::new());
    ARMED.with(|a| a.set(true));
    dom::swap_inner(SLOT, "");
}

/// Record that `stage` is happening NOW and repaint the header glyph iff the
/// pipeline's current stage changed. After `enter`, `stage` IS the current
/// phase, so it maps straight to a glyph — `Paying`/`Starting` resolve to an
/// empty fragment ([`templates::stage_status_button`]), which collapses the
/// slot (button gone). No-op when no turn is armed.
pub(crate) fn enter(stage: Stage) {
    if !ARMED.with(Cell::get) {
        return;
    }
    let changed = PIPELINE.with(|p| p.borrow_mut().enter(stage));
    if changed {
        dom::swap_inner(SLOT, &templates::stage_status_button(stage).into_string());
    }
}

/// The turn completed (done, errored, or cancelled): empty the slot (button
/// gone) and disarm. Idempotent; called from `mark_turn_done` and
/// (belt-and-braces) the run's `TurnGuard`.
pub(crate) fn end() {
    ARMED.with(|a| a.set(false));
    PIPELINE.with(|p| *p.borrow_mut() = StagePipeline::new());
    dom::swap_inner(SLOT, "");
}
