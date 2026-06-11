//! Pure state machine for the turn-stage micro-pipeline — the terse
//! "paying → thinking → streaming" line shown inside a PENDING assistant
//! turn (GitHub #19: the $LH-payment → execution transition was opaque; a
//! visitor paid and then stared at an unexplained pause).
//!
//! Native-testable, no DOM, no async — hoisted to the crate root like
//! `turn_flow` so its transition tests run under a plain `cargo test`. The
//! wasm side (`app::chat::stage`) is a thin painter that repaints one swap
//! target only when [`StagePipeline::enter`] reports a change, and empties
//! it when the turn completes.

/// One observable phase of a chat turn, in the order a cold paid turn would
/// hit them. Stages are recorded ONLY when they actually happen — a free
/// turn never shows `Paying`, a warm session never shows `Starting`, a
/// no-reasoning reply never shows `Thinking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The visitor payment gate is collecting the per-turn `$LH` price
    /// (signing + settling the sponsored transfer to the agent's TBA).
    Paying,
    /// A model session is booting (first turn, or the identity changed).
    Starting,
    /// Reasoning deltas are streaming (the model is thinking, no text yet).
    Thinking,
    /// Final-answer text is streaming.
    Streaming,
    /// A tool call is executing.
    Tools,
}

impl Stage {
    /// The lowercase word rendered in the pipeline line.
    pub fn word(self) -> &'static str {
        match self {
            Stage::Paying => "paying",
            Stage::Starting => "starting",
            Stage::Thinking => "thinking",
            Stage::Streaming => "streaming",
            Stage::Tools => "tools",
        }
    }
}

/// How a recorded stage renders relative to the CURRENT one: `Past` stages
/// (already crossed, muted), the `Current` stage (emphasized), and `Idle`
/// stages — entered earlier but ahead of a pointer that walked BACK (a
/// streaming ↔ tools ping-pong), dimmest of the three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Crossed already — renders muted.
    Past,
    /// The stage happening NOW — renders emphasized (and pulses).
    Current,
    /// Entered before, but the pointer moved back past it — renders dim.
    Idle,
}

/// First-entry-ordered trail of stages with a movable CURRENT pointer.
///
/// [`enter`](Self::enter) appends a stage the first time it's seen and just
/// moves the pointer on re-entry, so a streaming ↔ tools ping-pong never
/// duplicates words — the emphasis walks back and forth over a stable trail
/// instead. Starts empty: nothing is shown until something actually happens.
#[derive(Debug, Default)]
pub struct StagePipeline {
    trail: Vec<Stage>,
    current: usize,
}

impl StagePipeline {
    /// An empty pipeline (no stages entered, nothing to render).
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `stage` is happening NOW. Returns `true` when the
    /// rendered line changed (the caller repaints only then): a new word was
    /// appended, or the current pointer moved to an already-entered word.
    /// Re-entering the stage that is already current is a no-op.
    pub fn enter(&mut self, stage: Stage) -> bool {
        if let Some(idx) = self.trail.iter().position(|s| *s == stage) {
            if self.current == idx {
                return false;
            }
            self.current = idx;
            return true;
        }
        self.trail.push(stage);
        self.current = self.trail.len() - 1;
        true
    }

    /// True when no stage has been entered yet (render nothing).
    pub fn is_empty(&self) -> bool {
        self.trail.is_empty()
    }

    /// The trail in first-entry order, each word tagged with how it renders
    /// relative to the current pointer.
    pub fn slots(&self) -> Vec<(Stage, Slot)> {
        self.trail
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let slot = match i.cmp(&self.current) {
                    std::cmp::Ordering::Less => Slot::Past,
                    std::cmp::Ordering::Equal => Slot::Current,
                    std::cmp::Ordering::Greater => Slot::Idle,
                };
                (*s, slot)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{Slot, Stage, StagePipeline};

    #[test]
    fn words_are_lowercase_and_stable() {
        // The UI contract: terse lowercase words, no prose.
        assert_eq!(Stage::Paying.word(), "paying");
        assert_eq!(Stage::Starting.word(), "starting");
        assert_eq!(Stage::Thinking.word(), "thinking");
        assert_eq!(Stage::Streaming.word(), "streaming");
        assert_eq!(Stage::Tools.word(), "tools");
        for s in [
            Stage::Paying,
            Stage::Starting,
            Stage::Thinking,
            Stage::Streaming,
            Stage::Tools,
        ] {
            assert_eq!(s.word(), s.word().to_lowercase());
            assert!(!s.word().contains(' '));
        }
    }

    #[test]
    fn fresh_pipeline_renders_nothing() {
        // Stages appear ONLY as they apply — a brand-new turn shows no line.
        let p = StagePipeline::new();
        assert!(p.is_empty());
        assert!(p.slots().is_empty());
    }

    #[test]
    fn first_enter_appends_and_is_current() {
        let mut p = StagePipeline::new();
        assert!(p.enter(Stage::Thinking));
        assert_eq!(p.slots(), vec![(Stage::Thinking, Slot::Current)]);
    }

    #[test]
    fn reentering_the_current_stage_is_a_no_op() {
        // Every Text delta calls enter(Streaming) — only the FIRST repaints.
        let mut p = StagePipeline::new();
        assert!(p.enter(Stage::Streaming));
        assert!(!p.enter(Stage::Streaming));
        assert!(!p.enter(Stage::Streaming));
        assert_eq!(p.slots(), vec![(Stage::Streaming, Slot::Current)]);
    }

    #[test]
    fn paid_cold_session_walks_the_full_pipeline() {
        // paying → starting → thinking → streaming: a visitor's first turn
        // on a priced agent. Everything before the pointer is Past.
        let mut p = StagePipeline::new();
        for s in [Stage::Paying, Stage::Starting, Stage::Thinking, Stage::Streaming] {
            assert!(p.enter(s));
        }
        assert_eq!(
            p.slots(),
            vec![
                (Stage::Paying, Slot::Past),
                (Stage::Starting, Slot::Past),
                (Stage::Thinking, Slot::Past),
                (Stage::Streaming, Slot::Current),
            ]
        );
    }

    #[test]
    fn free_turn_never_shows_paying_or_starting() {
        // Stages shown only as they apply: an owner turn on a warm session
        // starts straight at thinking.
        let mut p = StagePipeline::new();
        p.enter(Stage::Thinking);
        p.enter(Stage::Streaming);
        let words: Vec<&str> = p.slots().iter().map(|(s, _)| s.word()).collect();
        assert_eq!(words, vec!["thinking", "streaming"]);
    }

    #[test]
    fn tools_streaming_ping_pong_never_duplicates_words() {
        // streaming → tools → streaming → tools → … : the trail stays two
        // words; the Current emphasis walks back and forth, and whichever
        // word sits AFTER the pointer renders Idle (dim), not duplicated.
        let mut p = StagePipeline::new();
        p.enter(Stage::Streaming);
        p.enter(Stage::Tools);
        assert!(p.enter(Stage::Streaming)); // pointer walks BACK — repaint
        assert_eq!(
            p.slots(),
            vec![(Stage::Streaming, Slot::Current), (Stage::Tools, Slot::Idle)]
        );
        assert!(p.enter(Stage::Tools));
        assert_eq!(
            p.slots(),
            vec![(Stage::Streaming, Slot::Past), (Stage::Tools, Slot::Current)]
        );
        // Five more rounds: the trail length never grows past 2.
        for _ in 0..5 {
            p.enter(Stage::Streaming);
            p.enter(Stage::Tools);
        }
        assert_eq!(p.slots().len(), 2);
    }

    #[test]
    fn exactly_one_current_at_all_times() {
        let mut p = StagePipeline::new();
        for s in [
            Stage::Paying,
            Stage::Starting,
            Stage::Thinking,
            Stage::Streaming,
            Stage::Tools,
            Stage::Streaming, // walk back
        ] {
            p.enter(s);
            let currents = p
                .slots()
                .iter()
                .filter(|(_, slot)| *slot == Slot::Current)
                .count();
            assert_eq!(currents, 1);
        }
    }
}
