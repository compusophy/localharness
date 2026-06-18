//! Pure turn-DIFFICULTY classification — the in-tab "difficulty router" core.
//!
//! One static session model answers everything: a greeting and a multi-file
//! refactor both run the coding-tier model at `ThinkingLevel::High`. That's
//! slow on "hi" and wasteful on the common case. This module classifies each
//! turn's prompt into a [`TurnTier`] and maps it to a (model preference,
//! [`ThinkingLevel`]) so the router can spend the expensive tier only on hard
//! turns: cheap + minimal-thinking for greetings / short reads, the premium
//! tier + high thinking reserved for build/debug.
//!
//! Native-testable, no DOM, no state, no async — the same pattern as
//! [`crate::turn_flow`] / [`crate::skills`]. The browser wiring
//! (`app::chat::session` / `app::chat::run_send`) picks the model + thinking
//! from this core; the heuristic + the tier→budget mapping run under
//! `cargo test` here.

use crate::types::ThinkingLevel;

/// How hard a single turn looks, from a cheap heuristic over its prompt + the
/// prior turn's tool activity. Drives the model + thinking budget the router
/// picks for the turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnTier {
    /// Greetings, acknowledgements, very short prompts, simple questions — no
    /// reasoning needed. Cheapest model, minimal thinking. Faster on "hi".
    Light,
    /// The common case — an ordinary request that isn't trivially light and
    /// shows no build/debug signal. Mid thinking on the session model.
    Standard,
    /// Build / debug / fix / compile work, code fences, multi-file references,
    /// or a continuation right after the model used tools — the turns that
    /// actually need deep reasoning. Premium tier, high thinking.
    Heavy,
}

/// A model-tier PREFERENCE the router would like for a turn, independent of
/// which concrete backend the user selected. The browser maps this onto a real
/// model id while honoring the user's pick as a CEILING (a Light turn on a
/// Claude-Opus session never upgrades; it only ever DOWNGRADES toward cheaper).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPreference {
    /// The cheapest available model (e.g. Gemini flash / Claude Haiku).
    Cheap,
    /// Whatever the session's default model is — no preference either way.
    Default,
    /// The most capable available model (e.g. Claude Opus / Sonnet) — only
    /// ever a HINT; the router clamps it to the user's selected ceiling.
    Premium,
}

/// The routing decision for a turn: the model tier to prefer + the thinking
/// budget to apply. Produced by [`route`]; the browser applies the thinking
/// per-turn and uses the preference (clamped to the user's model) to pick a
/// model where per-turn model switching is wired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnRoute {
    /// The difficulty tier this turn classified into.
    pub tier: TurnTier,
    /// Which model tier to prefer (clamped to the user's ceiling downstream).
    pub model: ModelPreference,
    /// The thinking budget to apply for this turn.
    pub thinking: ThinkingLevel,
}

/// Max prompt length (chars) that can still be `Light`. A short message with
/// no heavy signal is a greeting / quick question; anything longer is at least
/// `Standard` (it's carrying real content even if it lacks a heavy keyword).
const LIGHT_MAX_CHARS: usize = 80;

/// Verbs / phrases that mark a turn as build/debug/engineering work — the turns
/// that warrant the premium tier + high thinking. Matched case-insensitively as
/// substrings (so "debugging", "compiles", "refactored" all hit).
const HEAVY_KEYWORDS: &[&str] = &[
    "build", "compile", "debug", "fix", "error", "bug", "refactor", "implement",
    "rustlite", "cartridge", "wasm", "publish", "deploy", "stack trace", "panic",
    "exception", "failing", "broken", "optimize", "algorithm", "architect",
    "diagnose", "investigate", "trace", "regression", "edit_file", "create_file",
];

/// Greeting / acknowledgement tokens. A prompt that, once trimmed + lowercased,
/// IS one of these (or starts with one followed by punctuation) is `Light`
/// regardless of any heavy keyword landing inside the greeting word.
const GREETINGS: &[&str] = &[
    "hi", "hey", "hello", "yo", "sup", "thanks", "thank you", "ty", "ok", "okay",
    "cool", "nice", "great", "gm", "good morning", "good night", "bye", "lol",
];

/// True if `prompt` (already trimmed + lowercased) is a bare greeting /
/// acknowledgement: exactly a greeting token, or a greeting token followed only
/// by punctuation / whitespace (e.g. "hi!", "thanks."). Avoids matching
/// "thanks, now fix the build" — that has trailing content.
fn is_bare_greeting(lower: &str) -> bool {
    GREETINGS.iter().any(|g| {
        if lower == *g {
            return true;
        }
        if let Some(rest) = lower.strip_prefix(g) {
            // The next char must be a boundary (not a letter) so "hint" doesn't
            // match "hi", and the remainder must be only punctuation/space.
            let next_is_boundary = rest
                .chars()
                .next()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true);
            next_is_boundary
                && rest
                    .chars()
                    .all(|c| c.is_whitespace() || c.is_ascii_punctuation())
        } else {
            false
        }
    })
}

/// True if the prompt contains a fenced code block (```), which always implies
/// real code work → `Heavy`.
fn has_code_fence(prompt: &str) -> bool {
    prompt.contains("```")
}

/// Rough multi-file signal: two or more file-path-looking tokens (a word with a
/// code/file extension, or a `src/...`-style path). Multi-file work is `Heavy`.
fn references_multiple_files(lower: &str) -> bool {
    const EXTS: &[&str] = &[
        ".rs", ".ts", ".js", ".sol", ".rl", ".toml", ".json", ".html", ".css",
        ".md", ".sh", ".wasm",
    ];
    let hits = lower
        .split_whitespace()
        .filter(|tok| {
            EXTS.iter().any(|e| tok.contains(e)) || tok.contains("src/")
        })
        .count();
    hits >= 2
}

/// Classify a turn's difficulty from its prompt and whether the PRIOR turn used
/// tools. Pure → unit-testable without a browser.
///
/// Heuristic, in precedence order:
/// 1. A bare greeting / acknowledgement → [`TurnTier::Light`] (always; never
///    sticky, so a chat reply after a build doesn't burn the premium tier).
/// 2. A code fence, any [`HEAVY_KEYWORDS`] verb, multiple file references, OR a
///    continuation right after tool use (`last_turn_used_tools`) → [`TurnTier::Heavy`].
/// 3. A short prompt (`<= LIGHT_MAX_CHARS`) with no heavy signal → [`TurnTier::Light`].
/// 4. Everything else → [`TurnTier::Standard`].
///
/// `last_turn_used_tools` makes the router STICKY through a multi-step task: the
/// auto-continue turns of a build keep the premium tier instead of dropping to
/// Light because the nudge text happens to be short.
pub fn classify_turn(prompt: &str, last_turn_used_tools: bool) -> TurnTier {
    let trimmed = prompt.trim();
    let lower = trimmed.to_lowercase();

    // A bare greeting is ALWAYS light (even "thanks!" — short, no real ask) and
    // never sticky, so a chat reply after a build doesn't burn the premium tier.
    if is_bare_greeting(&lower) {
        return TurnTier::Light;
    }

    // Heavy signals: explicit build/debug content, OR a continuation mid
    // tool-task (the nudge is short but the work is hard).
    let heavy_signal = has_code_fence(trimmed)
        || HEAVY_KEYWORDS.iter().any(|k| lower.contains(k))
        || references_multiple_files(&lower);
    if heavy_signal || last_turn_used_tools {
        return TurnTier::Heavy;
    }

    // Short and no heavy signal → light (a quick question / one-liner).
    if trimmed.chars().count() <= LIGHT_MAX_CHARS {
        return TurnTier::Light;
    }

    TurnTier::Standard
}

/// Map a [`TurnTier`] to a [`TurnRoute`] — the model preference + thinking
/// budget for the turn. This is the policy the router applies:
///
/// | Tier     | Model      | Thinking            |
/// |----------|------------|---------------------|
/// | Light    | Cheap      | `Minimal`           |
/// | Standard | Default    | `Medium`            |
/// | Heavy    | Premium    | `High`              |
///
/// The model preference is a HINT only — the browser clamps it to the user's
/// selected model as a CEILING (Premium never upgrades past the user's pick;
/// Cheap only downgrades). The thinking budget is applied per-turn directly.
pub fn route_tier(tier: TurnTier) -> TurnRoute {
    let (model, thinking) = match tier {
        TurnTier::Light => (ModelPreference::Cheap, ThinkingLevel::Minimal),
        TurnTier::Standard => (ModelPreference::Default, ThinkingLevel::Medium),
        TurnTier::Heavy => (ModelPreference::Premium, ThinkingLevel::High),
    };
    TurnRoute { tier, model, thinking }
}

/// Classify + route in one call — the convenience the browser uses per turn.
pub fn route(prompt: &str, last_turn_used_tools: bool) -> TurnRoute {
    route_tier(classify_turn(prompt, last_turn_used_tools))
}

/// Clamp a thinking budget to a CEILING. The router only ever LOWERS thinking
/// below the session baseline for routine turns; it never raises it above what
/// the session was built with. Ordering: `Minimal < Low < Medium < High`.
pub fn clamp_thinking(desired: ThinkingLevel, ceiling: ThinkingLevel) -> ThinkingLevel {
    fn rank(t: ThinkingLevel) -> u8 {
        match t {
            ThinkingLevel::Minimal => 0,
            ThinkingLevel::Low => 1,
            ThinkingLevel::Medium => 2,
            ThinkingLevel::High => 3,
        }
    }
    if rank(desired) <= rank(ceiling) {
        desired
    } else {
        ceiling
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify_turn -------------------------------------------------------

    #[test]
    fn greetings_are_light() {
        for g in ["hi", "Hey", "hello", "yo", "thanks", "Thank you", "ok", "gm"] {
            assert_eq!(classify_turn(g, false), TurnTier::Light, "{g:?}");
        }
        // Greeting + trailing punctuation is still a bare greeting.
        assert_eq!(classify_turn("hi!", false), TurnTier::Light);
        assert_eq!(classify_turn("thanks.", false), TurnTier::Light);
        assert_eq!(classify_turn("  Hello?  ", false), TurnTier::Light);
    }

    #[test]
    fn greeting_word_then_real_ask_is_not_light_greeting() {
        // "thanks, now fix the build" has trailing content + a heavy keyword.
        assert_eq!(
            classify_turn("thanks, now fix the build", false),
            TurnTier::Heavy
        );
        // "hint" must not match the "hi" greeting prefix (word boundary).
        // It's short + no heavy signal → Light, but via the short-prompt path,
        // not the greeting path — either way Light here; assert it lands Light.
        assert_eq!(classify_turn("hint", false), TurnTier::Light);
    }

    #[test]
    fn short_simple_questions_are_light() {
        assert_eq!(classify_turn("what is pricing?", false), TurnTier::Light);
        assert_eq!(classify_turn("who are you", false), TurnTier::Light);
        assert_eq!(classify_turn("how much do you charge", false), TurnTier::Light);
    }

    #[test]
    fn build_debug_verbs_are_heavy() {
        for p in [
            "fix the failing test",
            "debug this panic",
            "compile the cartridge",
            "implement a new facet",
            "refactor the session module",
            "why is the build broken",
            "optimize this algorithm",
            "investigate the regression",
        ] {
            assert_eq!(classify_turn(p, false), TurnTier::Heavy, "{p:?}");
        }
    }

    #[test]
    fn code_fence_is_heavy() {
        let p = "what does this do?\n```rust\nfn main() {}\n```";
        assert_eq!(classify_turn(p, false), TurnTier::Heavy);
    }

    #[test]
    fn multiple_file_refs_are_heavy() {
        assert_eq!(
            classify_turn("compare src/app/chat/mod.rs and session.rs behavior", false),
            TurnTier::Heavy
        );
        // A single file ref alone (no other heavy signal) is NOT heavy on count.
        assert_eq!(
            classify_turn("open notes.md please", false),
            TurnTier::Light // short + single file + no verb
        );
    }

    #[test]
    fn tool_use_last_turn_makes_short_prompt_heavy() {
        // The auto-continue nudge is short, but mid-tool-task it must stay Heavy.
        assert_eq!(classify_turn("continue", true), TurnTier::Heavy);
        // The SAME short prompt with no prior tool use is Light.
        assert_eq!(classify_turn("continue", false), TurnTier::Light);
    }

    #[test]
    fn greeting_after_tool_use_stays_light() {
        // A genuine acknowledgement after a build shouldn't burn the premium
        // tier — a bare greeting overrides the sticky tool-use signal.
        assert_eq!(classify_turn("thanks!", true), TurnTier::Light);
    }

    #[test]
    fn long_neutral_prompt_is_standard() {
        let p = "Please summarize the overall design philosophy behind this \
                 platform and how the pieces fit together at a high level for me.";
        assert!(p.chars().count() > LIGHT_MAX_CHARS);
        assert_eq!(classify_turn(p, false), TurnTier::Standard);
    }

    #[test]
    fn empty_prompt_is_light() {
        assert_eq!(classify_turn("", false), TurnTier::Light);
        assert_eq!(classify_turn("   ", false), TurnTier::Light);
    }

    // --- route_tier / route --------------------------------------------------

    #[test]
    fn tier_maps_to_expected_route() {
        let l = route_tier(TurnTier::Light);
        assert_eq!(l.model, ModelPreference::Cheap);
        assert_eq!(l.thinking, ThinkingLevel::Minimal);

        let s = route_tier(TurnTier::Standard);
        assert_eq!(s.model, ModelPreference::Default);
        assert_eq!(s.thinking, ThinkingLevel::Medium);

        let h = route_tier(TurnTier::Heavy);
        assert_eq!(h.model, ModelPreference::Premium);
        assert_eq!(h.thinking, ThinkingLevel::High);
    }

    #[test]
    fn route_combines_classify_and_map() {
        assert_eq!(route("hi", false).thinking, ThinkingLevel::Minimal);
        assert_eq!(route("fix the build", false).thinking, ThinkingLevel::High);
        let standard = "Please walk me through the high-level economy ladder \
                        design in some reasonable amount of detail thank you.";
        assert_eq!(route(standard, false).tier, TurnTier::Standard);
    }

    // --- clamp_thinking ------------------------------------------------------

    #[test]
    fn clamp_never_exceeds_ceiling() {
        // Heavy wants High, but a Haiku-tier session ceiling of Medium caps it.
        assert_eq!(
            clamp_thinking(ThinkingLevel::High, ThinkingLevel::Medium),
            ThinkingLevel::Medium
        );
        // Below the ceiling passes through unchanged (the routine-downgrade case).
        assert_eq!(
            clamp_thinking(ThinkingLevel::Minimal, ThinkingLevel::High),
            ThinkingLevel::Minimal
        );
        // Equal passes through.
        assert_eq!(
            clamp_thinking(ThinkingLevel::High, ThinkingLevel::High),
            ThinkingLevel::High
        );
        // The router only DOWNGRADES: a High ceiling never lifts a Low
        // routine turn.
        assert_eq!(
            clamp_thinking(ThinkingLevel::Low, ThinkingLevel::High),
            ThinkingLevel::Low
        );
    }

    /// The invariant the wiring relies on: for ANY tier, the applied thinking is
    /// never above the session ceiling — so the router can only make routine
    /// turns cheaper, never escalate past the user's pick.
    #[test]
    fn routed_thinking_respects_ceiling_for_every_tier() {
        for ceiling in [
            ThinkingLevel::Minimal,
            ThinkingLevel::Low,
            ThinkingLevel::Medium,
            ThinkingLevel::High,
        ] {
            for tier in [TurnTier::Light, TurnTier::Standard, TurnTier::Heavy] {
                let desired = route_tier(tier).thinking;
                let applied = clamp_thinking(desired, ceiling);
                // Idempotent + never exceeds ceiling.
                assert_eq!(clamp_thinking(applied, ceiling), applied);
            }
        }
    }
}
