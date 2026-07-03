//! Pure INTENT-ROUTER core — the free/metered cost gate in front of the
//! browser chat's metered model call (native-testable, the [`crate::turn_flow`]
//! hoisting pattern: no DOM, no state, no async).
//!
//! Every browser message costs ~1 `$LH` the instant it hits the credit proxy —
//! including "show my balance" and "open files", which never needed a model.
//! This module classifies a user message into [`Route::Free`] (answered
//! locally, zero `$LH`) or [`Route::Metered`] (the normal model turn).
//!
//! **CONSERVATISM IS THE SPEC.** Only EXACT matches against a tight allowlist
//! of command phrasings route Free — anything ambiguous, long, multi-clause,
//! or not letter-for-letter on the list goes to the model untouched. A wrongly
//! free-routed message costs the user a real answer; a wrongly metered one
//! costs 1 `$LH`. We bias hard toward the second. Escape hatches: a leading
//! `'!'` ALWAYS forces Metered, and every Free answer carries
//! [`FREE_ROUTE_FOOTER`] telling the user how to reach the model.
//!
//! The classifier is a TRAIT ([`IntentClassifier`]) so the heuristic can be
//! swapped for a real local model later (the in-browser Gemma behind
//! `browser-app-local` — same seam, `backends/local`) without touching the
//! chat wiring.

/// A UI action a free-routed message dispatches — each maps 1:1 onto an
/// existing `app::events` handler (the same toggles the header buttons drive).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiCommand {
    /// The OPFS file-browser modal (`Action::ToggleFiles`).
    OpenFiles,
    /// The fullscreen display overlay (`Action::ToggleDisplay`).
    OpenDisplay,
    /// The CLI-sandbox terminal overlay (`Action::ToggleTerminal`).
    OpenTerminal,
}

/// A docs-FAQ topic answered from the embedded self-docs facts
/// (`app::self_docs::RUNTIME_SUMMARY`) — see [`docs_answer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocsTopic {
    /// "what does a message cost" — the 1 `$LH`/message meter fact.
    Pricing,
    /// "how do i get $LH" — the funding paths (redeem / send_lh / invite / buy).
    Funding,
    /// "what is this" — the one-paragraph platform summary.
    WhatIsThis,
}

/// What a [`Route::Free`] message resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeAction {
    /// Render the same `$LH` total the admin credits pill shows
    /// (wallet + meter, live RPC reads — no model).
    BalanceQuery,
    /// Dispatch an existing UI toggle.
    UiCommand(UiCommand),
    /// Render a canned fact card from [`docs_answer`].
    DocsAnswer(DocsTopic),
}

/// The routing decision for one user message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// Answer locally for zero `$LH`.
    Free(FreeAction),
    /// The normal metered model turn — the default for EVERYTHING not on the
    /// exact allowlist.
    Metered,
}

/// The classifier seam. [`HeuristicClassifier`] is the shipped exact-match
/// implementation; a local-model classifier (in-browser Gemma,
/// `browser-app-local`) can implement the same trait later and slot into the
/// identical chat wiring.
pub trait IntentClassifier {
    /// Route one raw user message. MUST be conservative: when in any doubt,
    /// return [`Route::Metered`].
    fn classify(&self, input: &str) -> Route;
}

/// Exact-allowlist heuristic classifier — see the module docs for the
/// conservatism contract. Stateless unit struct.
#[derive(Debug, Default, Clone, Copy)]
pub struct HeuristicClassifier;

/// Appended (by the chat wiring) to EVERY free-routed answer so the free tier
/// is always visibly escapable.
pub const FREE_ROUTE_FOOTER: &str =
    "(routed free — no $LH spent. Prefix with '!' or rephrase to ask the model.)";

/// A normalized message longer than this can never be a command phrasing —
/// cheap short-circuit before the allowlist scan. The longest allowlist entry
/// is well under this.
const MAX_FREE_CHARS: usize = 40;

/// The ENTIRE free surface: exact normalized phrasings only. Adding an entry
/// here is the ONLY way to widen the free tier — never add fuzzy/substring
/// matching (that's how "balance my argument" gets eaten).
const FREE_PHRASES: &[(&str, FreeAction)] = &[
    // ── balance / credits queries → the credits-pill data ──
    ("balance", FreeAction::BalanceQuery),
    ("my balance", FreeAction::BalanceQuery),
    ("show balance", FreeAction::BalanceQuery),
    ("show my balance", FreeAction::BalanceQuery),
    ("check balance", FreeAction::BalanceQuery),
    ("check my balance", FreeAction::BalanceQuery),
    ("what is my balance", FreeAction::BalanceQuery),
    ("whats my balance", FreeAction::BalanceQuery),
    ("what's my balance", FreeAction::BalanceQuery),
    ("credits", FreeAction::BalanceQuery),
    ("my credits", FreeAction::BalanceQuery),
    ("show credits", FreeAction::BalanceQuery),
    ("show my credits", FreeAction::BalanceQuery),
    ("check my credits", FreeAction::BalanceQuery),
    ("lh balance", FreeAction::BalanceQuery),
    ("$lh balance", FreeAction::BalanceQuery),
    ("credit balance", FreeAction::BalanceQuery),
    ("how much lh do i have", FreeAction::BalanceQuery),
    ("how much $lh do i have", FreeAction::BalanceQuery),
    ("how many credits do i have", FreeAction::BalanceQuery),
    // ── UI commands → existing Action toggles ──
    ("files", FreeAction::UiCommand(UiCommand::OpenFiles)),
    ("open files", FreeAction::UiCommand(UiCommand::OpenFiles)),
    ("show files", FreeAction::UiCommand(UiCommand::OpenFiles)),
    ("show my files", FreeAction::UiCommand(UiCommand::OpenFiles)),
    ("open the files", FreeAction::UiCommand(UiCommand::OpenFiles)),
    ("file browser", FreeAction::UiCommand(UiCommand::OpenFiles)),
    ("open file browser", FreeAction::UiCommand(UiCommand::OpenFiles)),
    ("display", FreeAction::UiCommand(UiCommand::OpenDisplay)),
    ("open display", FreeAction::UiCommand(UiCommand::OpenDisplay)),
    ("show display", FreeAction::UiCommand(UiCommand::OpenDisplay)),
    ("open the display", FreeAction::UiCommand(UiCommand::OpenDisplay)),
    ("toggle display", FreeAction::UiCommand(UiCommand::OpenDisplay)),
    ("terminal", FreeAction::UiCommand(UiCommand::OpenTerminal)),
    ("open terminal", FreeAction::UiCommand(UiCommand::OpenTerminal)),
    ("show terminal", FreeAction::UiCommand(UiCommand::OpenTerminal)),
    ("open the terminal", FreeAction::UiCommand(UiCommand::OpenTerminal)),
    // ── docs FAQ → canned facts from self_docs ──
    ("what does a message cost", FreeAction::DocsAnswer(DocsTopic::Pricing)),
    ("how much does a message cost", FreeAction::DocsAnswer(DocsTopic::Pricing)),
    ("how much is a message", FreeAction::DocsAnswer(DocsTopic::Pricing)),
    ("what does the meter cost", FreeAction::DocsAnswer(DocsTopic::Pricing)),
    ("price per message", FreeAction::DocsAnswer(DocsTopic::Pricing)),
    ("cost per message", FreeAction::DocsAnswer(DocsTopic::Pricing)),
    ("how do i get lh", FreeAction::DocsAnswer(DocsTopic::Funding)),
    ("how do i get $lh", FreeAction::DocsAnswer(DocsTopic::Funding)),
    ("how do i get credits", FreeAction::DocsAnswer(DocsTopic::Funding)),
    ("how do i fund my wallet", FreeAction::DocsAnswer(DocsTopic::Funding)),
    ("what is localharness", FreeAction::DocsAnswer(DocsTopic::WhatIsThis)),
    ("what is this", FreeAction::DocsAnswer(DocsTopic::WhatIsThis)),
    ("what is this app", FreeAction::DocsAnswer(DocsTopic::WhatIsThis)),
];

/// The canned answer for a docs-FAQ topic. Facts mirror
/// `app::self_docs::RUNTIME_SUMMARY` (1 `$LH`/message, $1 = 100 `$LH`, funding
/// paths, the platform one-liner) — update BOTH if a fact changes.
pub fn docs_answer(topic: DocsTopic) -> &'static str {
    match topic {
        DocsTopic::Pricing => {
            "Platform-credit messages cost 1 $LH each, debited from your meter \
             per message (premium models are tiered higher). Fiat on-ramp: \
             $1 = 100 $LH."
        }
        DocsTopic::Funding => {
            "Fund an agent with $LH via a redeem code, a send_lh transfer from \
             another agent, an ?invite= link (refundable escrow), or a card buy \
             ($1 = 100 $LH). The daily free claim is disabled."
        }
        DocsTopic::WhatIsThis => {
            "localharness — a self-sovereign, browser-resident agent platform: \
             one Rust crate compiled to wasm running entirely in your tab. Your \
             agent is an ERC-721 name with an ERC-6551 wallet on Tempo; the only \
             server is the $LH credit proxy. Ask the model (metered) for detail, \
             or have it call read_self_docs."
        }
    }
}

/// Strip ONE leading `'!'` (the force-metered escape) so the model never sees
/// the router's own syntax. No-op when the message isn't `'!'`-prefixed.
pub fn strip_bang(input: &str) -> &str {
    let t = input.trim_start();
    match t.strip_prefix('!') {
        Some(rest) => rest.trim_start(),
        None => input,
    }
}

/// The `/router` chat command — the per-session kill switch for the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterCmd {
    /// `/router on` — (re-)enable the free tiers (the default).
    On,
    /// `/router off` — every message goes to the model for this session.
    Off,
    /// `/router` / `/router status` — report the current state.
    Status,
}

/// Parse a `/router …` chat command. `None` for anything else (including
/// `/router <garbage>` — unknown args are NOT swallowed; they go to the model
/// like any other text).
pub fn parse_router_cmd(input: &str) -> Option<RouterCmd> {
    let t = input.trim().to_ascii_lowercase();
    match t.as_str() {
        "/router on" => Some(RouterCmd::On),
        "/router off" => Some(RouterCmd::Off),
        "/router" | "/router status" => Some(RouterCmd::Status),
        _ => None,
    }
}

/// Lowercase, collapse whitespace runs to single spaces, and strip trailing
/// terminal punctuation (`?` `!` `.`) — so "Show my balance?" and
/// "show   my balance" both hit the allowlist. Apostrophes and every interior
/// character are preserved (interior punctuation is a Metered signal).
fn normalize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for word in input.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(&word.to_lowercase());
    }
    while out.ends_with(['?', '!', '.']) {
        out.pop();
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

impl IntentClassifier for HeuristicClassifier {
    fn classify(&self, input: &str) -> Route {
        let raw = input.trim();
        // Empty/whitespace → Metered (the chat wiring rejects empties before
        // classifying anyway; this is the safe default for direct callers).
        if raw.is_empty() {
            return Route::Metered;
        }
        // '!' ALWAYS forces the model — the user's unconditional escape hatch.
        if raw.starts_with('!') {
            return Route::Metered;
        }
        let norm = normalize(raw);
        // Belt-and-braces guards (the exact-match scan below already excludes
        // these, but they make the conservatism explicit and keep any FUTURE
        // matcher change safe by construction): long, multi-clause, or
        // interior-question messages are never free.
        if norm.is_empty()
            || norm.len() > MAX_FREE_CHARS
            || norm.contains(['\n', ',', ';', ':', '?'])
            || norm.contains(" and ")
            || norm.contains(" then ")
        {
            return Route::Metered;
        }
        for (phrase, action) in FREE_PHRASES {
            if norm == *phrase {
                return Route::Free(*action);
            }
        }
        Route::Metered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify(s: &str) -> Route {
        HeuristicClassifier.classify(s)
    }

    // ── the allowlist routes Free ────────────────────────────────────────

    #[test]
    fn balance_phrasings_route_free() {
        for s in [
            "balance",
            "Balance",
            "  balance  ",
            "balance?",
            "show my balance",
            "Show my balance?",
            "what's my balance",
            "What is my balance?",
            "check my balance",
            "credits",
            "my credits",
            "how much $LH do I have?",
            "how many credits do i have",
            "lh balance",
        ] {
            assert_eq!(classify(s), Route::Free(FreeAction::BalanceQuery), "{s:?}");
        }
    }

    #[test]
    fn ui_commands_route_free() {
        assert_eq!(
            classify("open files"),
            Route::Free(FreeAction::UiCommand(UiCommand::OpenFiles))
        );
        assert_eq!(
            classify("Files"),
            Route::Free(FreeAction::UiCommand(UiCommand::OpenFiles))
        );
        assert_eq!(
            classify("open the display"),
            Route::Free(FreeAction::UiCommand(UiCommand::OpenDisplay))
        );
        assert_eq!(
            classify("show terminal"),
            Route::Free(FreeAction::UiCommand(UiCommand::OpenTerminal))
        );
    }

    #[test]
    fn docs_faq_routes_free() {
        assert_eq!(
            classify("what does the meter cost?"),
            Route::Free(FreeAction::DocsAnswer(DocsTopic::Pricing))
        );
        assert_eq!(
            classify("How much does a message cost?"),
            Route::Free(FreeAction::DocsAnswer(DocsTopic::Pricing))
        );
        assert_eq!(
            classify("how do I get $lh"),
            Route::Free(FreeAction::DocsAnswer(DocsTopic::Funding))
        );
        assert_eq!(
            classify("what is localharness?"),
            Route::Free(FreeAction::DocsAnswer(DocsTopic::WhatIsThis))
        );
    }

    #[test]
    fn docs_answers_carry_the_pricing_fact() {
        // The pricing fact mirrored from self_docs must survive edits.
        assert!(docs_answer(DocsTopic::Pricing).contains("1 $LH"));
        assert!(docs_answer(DocsTopic::Funding).contains("$1 = 100 $LH"));
        assert!(!docs_answer(DocsTopic::WhatIsThis).is_empty());
    }

    #[test]
    fn free_footer_names_the_escape_hatch() {
        assert!(FREE_ROUTE_FOOTER.contains("'!'"));
        assert!(FREE_ROUTE_FOOTER.to_lowercase().contains("model"));
    }

    // ── real prompts ALL route Metered (the conservatism contract) ──────

    #[test]
    fn realistic_prompts_route_metered() {
        // 25+ realistic messages, adversarial near-misses included: allowlist
        // words as verbs/nouns-in-context, questions about content, multi-clause
        // commands. EVERY one must pass through to the model untouched.
        for s in [
            // plain model work
            "write me a poem",
            "why did my tx fail?",
            "summarize my notes file",
            "translate hello to french",
            "help me plan a birthday party",
            "what should I build next?",
            "refactor my cartridge to use fewer state slots",
            "explain how the diamond proxy pattern works",
            "draft a persona for my agent",
            "generate an image of a lighthouse",
            // "balance" as a verb / other senses
            "balance my argument",
            "balance the equation 2x + 3 = 7",
            "how do I balance work and life?",
            "is my ledger balanced?",
            "balance transfer to bob", // value-adjacent — MUST hit the model
            // "credits" in a sentence / other senses
            "credits to the team for shipping this",
            "add film credits to my app",
            "why are my credits draining so fast?",
            "send 5 credits to alice",
            // files/display/terminal as content, not commands
            "show me how to open files in rust",
            "what files does my agent create?",
            "display a chart of my spending",
            "my display code is broken",
            "explain terminal velocity",
            "delete all my files", // destructive — MUST hit the model + confirm gate
            // multi-clause / compound
            "check my balance and then send 5 lh to bob",
            "open files, then edit app.rl",
            "show my balance please and thank you",
            // near-miss phrasings NOT on the list
            "gimme balance",
            "balance now now now",
            "what is my balance in usd",
            "how much is a message in dollars",
        ] {
            assert_eq!(classify(s), Route::Metered, "{s:?} must be metered");
        }
    }

    #[test]
    fn long_messages_never_route_free() {
        // Even one that CONTAINS an allowlisted phrase verbatim.
        let s = "balance ".repeat(20);
        assert_eq!(classify(&s), Route::Metered);
        assert_eq!(
            classify("show my balance after applying the pending bounty payouts"),
            Route::Metered
        );
    }

    // ── '!' forces Metered ───────────────────────────────────────────────

    #[test]
    fn bang_prefix_always_forces_metered() {
        for s in ["!balance", "! balance", "!credits", "  !open files", "!what is this"] {
            assert_eq!(classify(s), Route::Metered, "{s:?}");
        }
    }

    #[test]
    fn strip_bang_removes_only_the_router_escape() {
        assert_eq!(strip_bang("!balance"), "balance");
        assert_eq!(strip_bang("! balance"), "balance");
        assert_eq!(strip_bang("  !balance"), "balance");
        // Only ONE leading bang — "!!" keeps the second (user content).
        assert_eq!(strip_bang("!!balance"), "!balance");
        // Untouched when not prefixed (mid-string bangs are content).
        assert_eq!(strip_bang("balance!"), "balance!");
        assert_eq!(strip_bang("write a poem"), "write a poem");
    }

    // ── empty / whitespace ───────────────────────────────────────────────

    #[test]
    fn empty_and_whitespace_route_metered() {
        assert_eq!(classify(""), Route::Metered);
        assert_eq!(classify("   "), Route::Metered);
        assert_eq!(classify("\n\t"), Route::Metered);
        assert_eq!(classify("!"), Route::Metered);
        assert_eq!(classify("???"), Route::Metered);
    }

    // ── /router command parsing ──────────────────────────────────────────

    #[test]
    fn router_cmd_parses() {
        assert_eq!(parse_router_cmd("/router off"), Some(RouterCmd::Off));
        assert_eq!(parse_router_cmd("/router on"), Some(RouterCmd::On));
        assert_eq!(parse_router_cmd("/router"), Some(RouterCmd::Status));
        assert_eq!(parse_router_cmd("/router status"), Some(RouterCmd::Status));
        assert_eq!(parse_router_cmd("/Router OFF"), Some(RouterCmd::Off));
        assert_eq!(parse_router_cmd("  /router off  "), Some(RouterCmd::Off));
        // Unknown args / other text are NOT swallowed.
        assert_eq!(parse_router_cmd("/router maybe"), None);
        assert_eq!(parse_router_cmd("router off"), None);
        assert_eq!(parse_router_cmd("turn the /router off"), None);
    }

    // ── allowlist hygiene: every entry must be its own normalized form ───

    #[test]
    fn allowlist_entries_are_normalized_and_short() {
        for (phrase, _) in FREE_PHRASES {
            assert_eq!(*phrase, normalize(phrase), "{phrase:?} not normalized");
            assert!(phrase.len() <= MAX_FREE_CHARS, "{phrase:?} exceeds MAX_FREE_CHARS");
            // And each must actually route free (guards can't eat the list).
            assert!(
                matches!(classify(phrase), Route::Free(_)),
                "{phrase:?} on the list but classifies Metered"
            );
        }
    }
}
