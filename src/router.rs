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
    /// SET the light theme (no-op if already light) — precise, not a blind
    /// toggle: "light mode" must never turn the lights off.
    ThemeLight,
    /// SET the dark theme (no-op if already dark).
    ThemeDark,
    /// SET the desktop (unframed) view.
    ViewDesktop,
    /// SET the mobile (9:16 framed) view.
    ViewMobile,
}

/// An admin surface rendered INLINE in the transcript as an interactive card
/// (telemetry #36 — admin is chat-native, not only a header panel). Each
/// topic reuses the settings sheet's own section templates + `data-action`
/// handlers, so the card IS the admin control, not a description of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminTopic {
    /// The index card: what you can ask for in chat + a door to the full sheet.
    Settings,
    /// Who am I — name / owner address / balance.
    Identity,
    /// The LLM model picker.
    Model,
    /// The public-face picker (directory / publish app / publish html).
    PublicFace,
    /// Funds — redeem a code, buy `$LH`, invite a friend.
    Funds,
    /// Device linking (QR seed-adoption) + device sync.
    Devices,
}

impl AdminTopic {
    /// Every topic — the retire sweep (`events::admin::retire_admin_cards`)
    /// iterates this, so a new variant that's missing here would leak
    /// duplicate element ids. Guarded by `admin_topic_slugs_are_id_safe`.
    pub const ALL: &'static [AdminTopic] = &[
        AdminTopic::Settings,
        AdminTopic::Identity,
        AdminTopic::Model,
        AdminTopic::PublicFace,
        AdminTopic::Funds,
        AdminTopic::Devices,
    ];

    /// Element-id suffix for the card wrapper (`#admin-card-<slug>`).
    pub fn slug(self) -> &'static str {
        match self {
            AdminTopic::Settings => "settings",
            AdminTopic::Identity => "identity",
            AdminTopic::Model => "model",
            AdminTopic::PublicFace => "public-face",
            AdminTopic::Funds => "funds",
            AdminTopic::Devices => "devices",
        }
    }

    /// Human card title (the small uppercase label on the card).
    pub fn title(self) -> &'static str {
        match self {
            AdminTopic::Settings => "settings",
            AdminTopic::Identity => "identity",
            AdminTopic::Model => "model",
            AdminTopic::PublicFace => "public face",
            AdminTopic::Funds => "funds",
            AdminTopic::Devices => "devices",
        }
    }
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
    /// Mount an interactive admin card inline in the transcript (#36).
    /// Mounting is FREE and takes no action — the card's buttons drive the
    /// same (owner-/confirm-gated) handlers as the settings sheet.
    AdminCard(AdminTopic),
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
    // ── render prefs → precise SET commands (never blind toggles) ──
    ("light mode", FreeAction::UiCommand(UiCommand::ThemeLight)),
    ("light theme", FreeAction::UiCommand(UiCommand::ThemeLight)),
    ("switch to light mode", FreeAction::UiCommand(UiCommand::ThemeLight)),
    ("dark mode", FreeAction::UiCommand(UiCommand::ThemeDark)),
    ("dark theme", FreeAction::UiCommand(UiCommand::ThemeDark)),
    ("switch to dark mode", FreeAction::UiCommand(UiCommand::ThemeDark)),
    ("desktop view", FreeAction::UiCommand(UiCommand::ViewDesktop)),
    ("desktop mode", FreeAction::UiCommand(UiCommand::ViewDesktop)),
    ("mobile view", FreeAction::UiCommand(UiCommand::ViewMobile)),
    ("mobile mode", FreeAction::UiCommand(UiCommand::ViewMobile)),
    // ── admin intents → inline admin cards (#36 chat-native admin) ──
    ("settings", FreeAction::AdminCard(AdminTopic::Settings)),
    ("open settings", FreeAction::AdminCard(AdminTopic::Settings)),
    ("show settings", FreeAction::AdminCard(AdminTopic::Settings)),
    ("admin", FreeAction::AdminCard(AdminTopic::Settings)),
    ("admin panel", FreeAction::AdminCard(AdminTopic::Settings)),
    ("open admin", FreeAction::AdminCard(AdminTopic::Settings)),
    ("open the admin panel", FreeAction::AdminCard(AdminTopic::Settings)),
    ("who am i", FreeAction::AdminCard(AdminTopic::Identity)),
    ("whoami", FreeAction::AdminCard(AdminTopic::Identity)),
    ("identity", FreeAction::AdminCard(AdminTopic::Identity)),
    ("my identity", FreeAction::AdminCard(AdminTopic::Identity)),
    ("my address", FreeAction::AdminCard(AdminTopic::Identity)),
    ("my wallet", FreeAction::AdminCard(AdminTopic::Identity)),
    ("show my wallet", FreeAction::AdminCard(AdminTopic::Identity)),
    ("show my address", FreeAction::AdminCard(AdminTopic::Identity)),
    ("my account", FreeAction::AdminCard(AdminTopic::Identity)),
    ("account", FreeAction::AdminCard(AdminTopic::Identity)),
    ("model", FreeAction::AdminCard(AdminTopic::Model)),
    ("models", FreeAction::AdminCard(AdminTopic::Model)),
    ("change model", FreeAction::AdminCard(AdminTopic::Model)),
    ("switch model", FreeAction::AdminCard(AdminTopic::Model)),
    ("change the model", FreeAction::AdminCard(AdminTopic::Model)),
    ("switch the model", FreeAction::AdminCard(AdminTopic::Model)),
    ("set model", FreeAction::AdminCard(AdminTopic::Model)),
    ("which model", FreeAction::AdminCard(AdminTopic::Model)),
    ("what model", FreeAction::AdminCard(AdminTopic::Model)),
    ("which model am i using", FreeAction::AdminCard(AdminTopic::Model)),
    ("what model is this", FreeAction::AdminCard(AdminTopic::Model)),
    ("public face", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("my public face", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("set public face", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("change public face", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("publish", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("publish app", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("publish my app", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("publish html", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("publish my html", FreeAction::AdminCard(AdminTopic::PublicFace)),
    ("funds", FreeAction::AdminCard(AdminTopic::Funds)),
    ("add funds", FreeAction::AdminCard(AdminTopic::Funds)),
    ("top up", FreeAction::AdminCard(AdminTopic::Funds)),
    ("topup", FreeAction::AdminCard(AdminTopic::Funds)),
    ("redeem", FreeAction::AdminCard(AdminTopic::Funds)),
    ("redeem code", FreeAction::AdminCard(AdminTopic::Funds)),
    ("redeem a code", FreeAction::AdminCard(AdminTopic::Funds)),
    ("buy lh", FreeAction::AdminCard(AdminTopic::Funds)),
    ("buy $lh", FreeAction::AdminCard(AdminTopic::Funds)),
    ("buy credits", FreeAction::AdminCard(AdminTopic::Funds)),
    ("invite", FreeAction::AdminCard(AdminTopic::Funds)),
    ("invite a friend", FreeAction::AdminCard(AdminTopic::Funds)),
    ("create invite", FreeAction::AdminCard(AdminTopic::Funds)),
    ("create an invite", FreeAction::AdminCard(AdminTopic::Funds)),
    ("devices", FreeAction::AdminCard(AdminTopic::Devices)),
    ("my devices", FreeAction::AdminCard(AdminTopic::Devices)),
    ("add device", FreeAction::AdminCard(AdminTopic::Devices)),
    ("add a device", FreeAction::AdminCard(AdminTopic::Devices)),
    ("link device", FreeAction::AdminCard(AdminTopic::Devices)),
    ("link a device", FreeAction::AdminCard(AdminTopic::Devices)),
    ("pair device", FreeAction::AdminCard(AdminTopic::Devices)),
    ("pair a device", FreeAction::AdminCard(AdminTopic::Devices)),
    ("sync devices", FreeAction::AdminCard(AdminTopic::Devices)),
    ("sync my devices", FreeAction::AdminCard(AdminTopic::Devices)),
    ("new device", FreeAction::AdminCard(AdminTopic::Devices)),
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

/// The `/router` chat command — the per-session switch for the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterCmd {
    /// `/router on` — re-enable the free tiers for this session (the
    /// default; only needed after a `/router off`).
    On,
    /// `/router off` — opt this session OUT: every message goes to the model.
    Off,
    /// `/router` / `/router status` — report the current state.
    Status,
}

/// The gate's enable decision from the stored per-session flag (sessionStorage
/// `lh_router` in the browser wiring). **Default ON** (browser paths tab-E2E'd
/// 2026-07-05) — only an explicit `"0"` (written by `/router off`) disables the
/// gate; unset/anything else leaves it on. Pure so the default is pinned
/// natively.
pub fn router_enabled(opt_out_flag: Option<&str>) -> bool {
    !matches!(opt_out_flag, Some("0"))
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
    fn admin_intents_route_to_their_card() {
        for (s, topic) in [
            ("settings", AdminTopic::Settings),
            ("Settings", AdminTopic::Settings),
            ("open the admin panel", AdminTopic::Settings),
            ("admin", AdminTopic::Settings),
            ("who am I?", AdminTopic::Identity),
            ("whoami", AdminTopic::Identity),
            ("my wallet", AdminTopic::Identity),
            ("account", AdminTopic::Identity),
            ("model", AdminTopic::Model),
            ("switch model", AdminTopic::Model),
            ("Which model am I using?", AdminTopic::Model),
            ("public face", AdminTopic::PublicFace),
            ("publish my app", AdminTopic::PublicFace),
            ("publish", AdminTopic::PublicFace),
            ("redeem a code", AdminTopic::Funds),
            ("buy $LH", AdminTopic::Funds),
            ("top up", AdminTopic::Funds),
            ("invite a friend", AdminTopic::Funds),
            ("add a device", AdminTopic::Devices),
            ("sync my devices", AdminTopic::Devices),
            ("devices", AdminTopic::Devices),
        ] {
            assert_eq!(
                classify(s),
                Route::Free(FreeAction::AdminCard(topic)),
                "{s:?}"
            );
        }
    }

    #[test]
    fn pref_commands_route_to_precise_setters() {
        for (s, cmd) in [
            ("light mode", UiCommand::ThemeLight),
            ("Dark mode", UiCommand::ThemeDark),
            ("switch to dark mode", UiCommand::ThemeDark),
            ("desktop view", UiCommand::ViewDesktop),
            ("mobile view", UiCommand::ViewMobile),
        ] {
            assert_eq!(classify(s), Route::Free(FreeAction::UiCommand(cmd)), "{s:?}");
        }
    }

    #[test]
    fn admin_near_misses_route_metered() {
        // Allowlist words in real sentences MUST still reach the model.
        for s in [
            "who am i kidding",
            "my account is locked, why?",
            "what model of car should i buy",
            "train a model on my notes",
            "publish my app to the app store",
            "add a device for my mom",
            "sync my devices and then publish",
            "settings for my cartridge",
            "change the model of my cartridge",
            "redeem my promise",
            "invite bob to the guild",
            "dark mode is ugly, fix the css",
            "make the light mode background whiter",
        ] {
            assert_eq!(classify(s), Route::Metered, "{s:?} must be metered");
        }
    }

    #[test]
    fn admin_topic_slugs_are_id_safe() {
        // Slugs become element ids (`#admin-card-<slug>`): unique, lowercase,
        // no spaces. ALL must cover every variant (retire sweep completeness).
        let mut seen = std::collections::HashSet::new();
        for t in AdminTopic::ALL {
            let slug = t.slug();
            assert!(seen.insert(slug), "duplicate slug {slug:?}");
            assert!(
                slug.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
                "{slug:?} not id-safe"
            );
            assert!(!t.title().is_empty());
        }
        assert_eq!(seen.len(), 6, "AdminTopic::ALL missing a variant");
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

    #[test]
    fn router_default_is_on_opt_out_only() {
        // DEFAULT ON (tab-E2E'd): only an explicit "0" (written by
        // `/router off`) disables the gate; unset/anything else leaves it on.
        assert!(router_enabled(None));
        assert!(router_enabled(Some("")));
        assert!(router_enabled(Some("1")));
        assert!(router_enabled(Some("true")));
        assert!(!router_enabled(Some("0")));
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
