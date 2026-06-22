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
    /// The pending assistant turn's BODY id (`turn-body-{id}`), so [`enter`] can
    /// mirror the current phase word onto its `data-stage` attribute — the
    /// in-stream "starting → thinking → …" cue (`::before { content: attr(...) }`,
    /// styles.css) tracks the phase, not a static word. Set by [`begin`], cleared
    /// by [`end`].
    static BODY_ID: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Arm the painter for a fresh turn: empty pipeline, empty slot. Nothing is
/// painted yet (no phase has happened), so the header button stays GONE until
/// the first starting/thinking/streaming/tools event. `body_id` is the pending
/// assistant turn's body (`turn-body-{id}`); [`enter`] writes the phase word to
/// its `data-stage` so the under-message cue tracks the phase.
pub(crate) fn begin(body_id: &str) {
    PIPELINE.with(|p| *p.borrow_mut() = StagePipeline::new());
    BODY_ID.with(|b| *b.borrow_mut() = body_id.to_string());
    ARMED.with(|a| a.set(true));
    dom::swap_inner(SLOT, "");
    // Paint an immediate "starting" word on the pending bubble so it is NEVER a
    // blank bubble before the first real stage (#58/T25: on mobile the gap to the
    // first token left an empty bubble for 100ms+, across the payment/session
    // awaits below). This sets ONLY the display attribute, not the pipeline, so
    // the canonical paying→starting→thinking→streaming ordering that the first
    // real `enter` records is unaffected (it overwrites this placeholder word).
    dom::set_attr(body_id, "data-stage", Stage::Starting.word());
}

/// Record that `stage` is happening NOW and repaint the header glyph iff the
/// pipeline's current stage changed. After `enter`, `stage` IS the current
/// phase, so it maps straight to a glyph — `Paying` resolves to an empty
/// fragment ([`templates::stage_status_button`]), which collapses the slot
/// (button gone). Also mirrors the phase word onto the pending body's
/// `data-stage` so the in-stream cue tracks it. No-op when no turn is armed.
pub(crate) fn enter(stage: Stage) {
    if !ARMED.with(Cell::get) {
        return;
    }
    let changed = PIPELINE.with(|p| p.borrow_mut().enter(stage));
    if changed {
        dom::swap_inner(SLOT, &templates::stage_status_button(stage).into_string());
        BODY_ID.with(|b| dom::set_attr(&b.borrow(), "data-stage", stage.word()));
    }
}

/// The turn completed (done, errored, or cancelled): empty the slot (button
/// gone), clear the body's `data-stage`, and disarm. Idempotent; called from
/// `mark_turn_done` and (belt-and-braces) the run's `TurnGuard`.
pub(crate) fn end() {
    ARMED.with(|a| a.set(false));
    PIPELINE.with(|p| *p.borrow_mut() = StagePipeline::new());
    BODY_ID.with(|b| {
        let mut b = b.borrow_mut();
        dom::set_attr(&b, "data-stage", "");
        b.clear();
    });
    dom::swap_inner(SLOT, "");
}
