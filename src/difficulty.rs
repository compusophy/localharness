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

/// Per-turn MODEL selection WITHIN the session's backend family (the #7
/// follow-up to the per-turn thinking budget #2). Given the turn's [`TurnTier`]
/// and the session's selected model id, return the model id to use for THIS
/// turn — or `None` to leave the session model unchanged (the byte-identical
/// no-op default).
///
/// Hard invariants (all unit-tested below):
/// - **Same backend only.** The returned id is ALWAYS in the same provider
///   family as `session_model` (a `claude-*` session never returns a `gemini-*`
///   id and vice-versa). Cross-backend switching is unsafe (different wire
///   format + history shape) and is never attempted — only the Anthropic family
///   ever resolves a different id; everything else returns `None`.
/// - **Ceiling = the session model.** A routine (`Light`/`Standard`) turn may
///   only DOWNGRADE toward a cheaper same-family model; the desired rung is
///   `min`-clamped to the session model's rung so it never exceeds the user's
///   pick. `Heavy` always stays at the session model (the full pick → `None`).
/// - **No-op outside the Anthropic family.** Gemini has a single in-tab flash
///   model, so there is no cheaper same-family id to route to → always `None`
///   (keep the session model). Local/BYOK/unknown ids → `None`.
///
/// Anthropic family ladder (cheap→premium): Haiku < Sonnet < Opus. Mapping,
/// clamped to the session model as the ceiling: `Light`→Haiku, `Standard`→
/// Sonnet, `Heavy`→the session model. Returns `None` whenever the resolved
/// model equals the session model, so an override is only ever SET when it
/// actually changes the model for the turn (keeps the no-op default exact).
#[cfg(feature = "anthropic")]
pub fn route_model(tier: TurnTier, session_model: &str) -> Option<String> {
    // Only the Anthropic family has a same-backend cheaper rung to route to.
    // A non-`claude-*` session (Gemini / local / BYOK / unknown) → no change.
    if !session_model.starts_with("claude-") {
        return None;
    }
    use crate::backends::anthropic::{DEFAULT_MODEL as HAIKU, OPUS_MODEL, SONNET_MODEL};

    // Rank within the Claude ladder (cheap→premium). The session model is the
    // CEILING. Match by family substring so a dated id (the Haiku
    // `…-4-5-20251001`) still classifies; Haiku is the floor / default.
    fn rank(model: &str) -> u8 {
        if model.contains("opus") {
            2
        } else if model.contains("sonnet") {
            1
        } else {
            0 // haiku or any other claude-* id → the cheap floor.
        }
    }

    let ceiling = rank(session_model);
    // Tier → desired rung, then `min`-clamp to the ceiling (NEVER upgrade).
    let desired = match tier {
        TurnTier::Light => 0,       // Haiku
        TurnTier::Standard => 1,    // Sonnet
        TurnTier::Heavy => ceiling, // session model — the full pick, no change
    };
    let chosen = desired.min(ceiling);
    if chosen == ceiling {
        // Heavy, or a session already at/below the desired rung → no change.
        return None;
    }
    let id = match chosen {
        0 => HAIKU,
        _ => SONNET_MODEL, // chosen == 1 (chosen < ceiling <= 2 ⇒ chosen ∈ {0,1})
    };
    // Defensive: `OPUS_MODEL` is referenced so a rename trips here, and we never
    // hand back the session model itself as an "override".
    let _ = OPUS_MODEL;
    if id == session_model {
        None
    } else {
        Some(id.to_string())
    }
}

/// Feature-off shim: without the `anthropic` backend there is no same-family
/// cheaper model to route to, so per-turn model selection is always a no-op.
#[cfg(not(feature = "anthropic"))]
pub fn route_model(_tier: TurnTier, _session_model: &str) -> Option<String> {
    None
}

/// The backend a `consult_model` call routes to, picked PURELY from the
/// requested model id. Hoisted here (the `difficulty`/`turn_flow` pattern) so
/// the model→backend decision is native-testable, independent of the wasm
/// `app::chat::tools::misc::consult_model_tool` that consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsultBackend {
    /// A `gemini-*` id → the Gemini backend.
    Gemini,
    /// A `claude-*` id → the Anthropic backend.
    Anthropic,
}

/// The model ids `consult_model` accepts, as `(id, label)` — the Claude tiers
/// plus the Gemini default. The single allowlist behind both the tool's enum
/// schema and [`select_consult_backend`], so the schema can never advertise an
/// id the router rejects. References the canonical backend consts (no re-typed
/// literal to drift); `anthropic`-gated so the Claude ids resolve, with a
/// Gemini-only fallback when the feature is off (the tool itself only exists in
/// `browser-app`, which always pulls `anthropic`).
#[cfg(feature = "anthropic")]
pub const CONSULT_MODELS: &[(&str, &str)] = &[
    (crate::types::DEFAULT_MODEL, "Gemini (default)"),
    (crate::backends::anthropic::OPUS_MODEL, "Claude Opus"),
    (crate::backends::anthropic::SONNET_MODEL, "Claude Sonnet"),
    (crate::backends::anthropic::DEFAULT_MODEL, "Claude Haiku"),
];

/// Gemini-only fallback allowlist when the `anthropic` backend is absent.
#[cfg(not(feature = "anthropic"))]
pub const CONSULT_MODELS: &[(&str, &str)] = &[(crate::types::DEFAULT_MODEL, "Gemini (default)")];

/// Pick the backend for a `consult_model` request, validated against
/// [`CONSULT_MODELS`]. An id outside the allowlist (unknown, or a model this
/// path can't route — local Gemma, a GPT id, junk) is rejected with a clear
/// error rather than silently routed. PURE — unit-tested natively below.
/// `claude-*` → [`ConsultBackend::Anthropic`]; everything else (the Gemini
/// default) → [`ConsultBackend::Gemini`].
pub fn select_consult_backend(model: &str) -> crate::error::Result<ConsultBackend> {
    if !CONSULT_MODELS.iter().any(|(id, _)| *id == model) {
        let supported = CONSULT_MODELS
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(crate::error::Error::other(format!(
            "consult_model: unsupported model {model:?} — choose one of: {supported}"
        )));
    }
    if model.starts_with("claude-") {
        Ok(ConsultBackend::Anthropic)
    } else {
        Ok(ConsultBackend::Gemini)
    }
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

    // --- route_model ---------------------------------------------------------

    /// Non-Anthropic sessions (Gemini / local / BYOK / unknown) NEVER get a
    /// per-turn model override — there is no cheaper same-family rung to route
    /// to. Works in every feature config (the feature-off shim returns `None`
    /// too), so this is the byte-identical no-op guarantee for those paths.
    #[test]
    fn route_model_is_noop_off_anthropic_family() {
        for session in [
            "gemini-3.5-flash",
            "gemma-3-270m",
            "gpt-4o",
            "",
            "something-weird",
        ] {
            for tier in [TurnTier::Light, TurnTier::Standard, TurnTier::Heavy] {
                assert_eq!(route_model(tier, session), None, "{session:?}/{tier:?}");
            }
        }
    }

    #[cfg(feature = "anthropic")]
    mod anthropic_family {
        use super::*;
        use crate::backends::anthropic::{
            DEFAULT_MODEL as HAIKU, OPUS_MODEL as OPUS, SONNET_MODEL as SONNET,
        };

        /// An Opus session (the top ceiling) downgrades routine turns: Light→
        /// Haiku, Standard→Sonnet, Heavy→no change (stays Opus).
        #[test]
        fn opus_session_downgrades_routine_turns() {
            assert_eq!(route_model(TurnTier::Light, OPUS).as_deref(), Some(HAIKU));
            assert_eq!(route_model(TurnTier::Standard, OPUS).as_deref(), Some(SONNET));
            assert_eq!(route_model(TurnTier::Heavy, OPUS), None);
        }

        /// A Sonnet session: Light→Haiku, Standard→no change (Sonnet IS the
        /// ceiling), Heavy→no change. Standard never UPGRADES to Opus.
        #[test]
        fn sonnet_session_clamps_standard_to_ceiling() {
            assert_eq!(route_model(TurnTier::Light, SONNET).as_deref(), Some(HAIKU));
            assert_eq!(route_model(TurnTier::Standard, SONNET), None);
            assert_eq!(route_model(TurnTier::Heavy, SONNET), None);
        }

        /// A Haiku session (the floor): every tier is already at/below Haiku, so
        /// there is never anything cheaper to route to → always `None`. This is
        /// the no-override default for the cheapest-model session.
        #[test]
        fn haiku_session_never_overrides() {
            for tier in [TurnTier::Light, TurnTier::Standard, TurnTier::Heavy] {
                assert_eq!(route_model(tier, HAIKU), None, "{tier:?}");
            }
        }

        /// The CEILING invariant for the whole ladder: for any Claude session
        /// model and any tier, the resolved model is NEVER more capable than the
        /// session model (only ever equal — `None` — or cheaper).
        #[test]
        fn never_exceeds_ceiling_for_every_claude_session() {
            fn rank(m: &str) -> u8 {
                if m.contains("opus") {
                    2
                } else if m.contains("sonnet") {
                    1
                } else {
                    0
                }
            }
            for session in [HAIKU, SONNET, OPUS] {
                let ceiling = rank(session);
                for tier in [TurnTier::Light, TurnTier::Standard, TurnTier::Heavy] {
                    let resolved = route_model(tier, session);
                    let applied = resolved.as_deref().unwrap_or(session);
                    assert!(
                        rank(applied) <= ceiling,
                        "session {session} tier {tier:?} routed to {applied} (rank {} > ceiling {ceiling})",
                        rank(applied)
                    );
                    // SAME-BACKEND: any override stays a `claude-*` id.
                    if let Some(id) = &resolved {
                        assert!(id.starts_with("claude-"), "crossed backend: {id}");
                    }
                }
            }
        }

        /// An override, when present, is ALWAYS different from the session model
        /// — we never hand back the session model dressed up as an "override"
        /// (so the wiring only ever calls `set_model_override(Some)` on a real
        /// change, keeping the no-op default exact).
        #[test]
        fn override_when_present_is_a_real_change() {
            for session in [HAIKU, SONNET, OPUS] {
                for tier in [TurnTier::Light, TurnTier::Standard, TurnTier::Heavy] {
                    if let Some(id) = route_model(tier, session) {
                        assert_ne!(id, session, "{session}/{tier:?} returned the session model");
                    }
                }
            }
        }
    }

    // --- select_consult_backend (consult_model routing) ----------------------

    #[cfg(feature = "anthropic")]
    mod consult {
        use super::*;
        use crate::backends::anthropic::{
            DEFAULT_MODEL as HAIKU, OPUS_MODEL as OPUS, SONNET_MODEL as SONNET,
        };

        /// Each Claude tier routes to the Anthropic backend; the Gemini default
        /// routes to Gemini. Every advertised id must classify (no allowlisted id
        /// is silently rejected).
        #[test]
        fn known_models_pick_the_right_backend() {
            assert_eq!(
                select_consult_backend(crate::types::DEFAULT_MODEL).unwrap(),
                ConsultBackend::Gemini
            );
            for claude in [HAIKU, SONNET, OPUS] {
                assert_eq!(
                    select_consult_backend(claude).unwrap(),
                    ConsultBackend::Anthropic,
                    "{claude}"
                );
            }
            // Every id in the allowlist must resolve (none rejected).
            for (id, _) in CONSULT_MODELS {
                assert!(select_consult_backend(id).is_ok(), "{id}");
            }
        }

        /// An unknown id, or a known-but-unroutable model (local Gemma, a GPT id,
        /// junk, empty), is REJECTED — never silently routed.
        #[test]
        fn unknown_or_unsupported_models_are_rejected() {
            for bad in [
                "gemma-3-270m",        // local backend — not a consult target
                "gpt-5-nano",          // OpenAI — no consult path
                "claude-imaginary-9",  // claude-shaped but not a real tier
                "gemini-2.5-flash",    // a dead/non-default Gemini id
                "",                    // empty
                "garbage",
            ] {
                let err = select_consult_backend(bad).unwrap_err();
                assert!(
                    err.to_string().contains("unsupported model"),
                    "{bad}: {err}"
                );
            }
        }
    }
}
