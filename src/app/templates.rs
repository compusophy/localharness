//! All HTML in the browser app is produced here, via [`maud`]
//! compile-time templates. Templates return `Markup`; callers turn
//! them into strings and ship them into the DOM via the helpers in
//! [`super::dom`]. **No template function takes a DOM handle** — they
//! are pure `inputs → HTML` functions, so they're trivial to read,
//! test, and recompose.

use maud::{html, Markup, PreEscaped};

use crate::filesystem::{DirEntry, EntryKind};
use crate::types::{BuiltinTool, ToolCall, ToolResult};

use super::tenant::Host;
use super::VerifyState;

/// API key modal — shown on tenant subdomains when no Gemini API key
/// is stored. Centered overlay with a single input + save button.
/// Dismisses itself on save; the key file appears in the OPFS panel.
pub(crate) fn api_key_modal() -> Markup {
    html! {
        div #api-key-modal .api-key-modal {
            div.api-key-card {
                div.api-key-title { "power this agent" }
                // PRIMARY: platform credits (no Google account / card needed).
                button type="button" data-action="set-model-access" data-arg="credits"
                    .ghost.api-key-primary { "use platform credits" }
                div.api-key-or { "or bring your own key" }
                // SECONDARY: BYOK.
                form onsubmit="return false" {
                    div.api-key-row {
                        input #api-key-input
                            type="password"
                            autocomplete="off"
                            aria-label="gemini api key"
                            placeholder="paste key" {}
                        button type="button"
                            data-action="save-api-key" { "save" }
                    }
                }
                div.api-key-hint {
                    a href="https://aistudio.google.com/apikey"
                        target="_blank" rel="noopener" { "get a free key →" }
                }
                div #api-key-msg .feedback-msg role="status" aria-live="polite" {}
            }
        }
    }
}

/// Render assistant markdown to HTML and wrap as `Markup` for direct DOM
/// insertion.
///
/// **Security:** pulldown-cmark does NOT sanitise — it passes raw HTML
/// in the source straight through, and emits `<a href>` verbatim
/// (including `javascript:` schemes). Since this renders model output
/// and restored history — which a prompt injection (a malicious file,
/// an inter-agent message, fetched web content) can influence — that
/// would be an XSS into the wallet origin. So we neutralise raw HTML
/// (render it as escaped text) and strip dangerous link schemes before
/// `push_html`. Markdown formatting still renders normally.
pub(crate) fn rendered_markdown(raw: &str) -> Markup {
    use pulldown_cmark::{html, CowStr, Event, Options, Parser, Tag};

    fn safe_url(url: CowStr) -> CowStr {
        let probe = url.trim_start().to_ascii_lowercase();
        let dangerous = probe.starts_with("javascript:")
            || probe.starts_with("vbscript:")
            || probe.starts_with("data:");
        if dangerous {
            CowStr::Borrowed("#")
        } else {
            url
        }
    }

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(raw, opts).map(|event| match event {
        // Raw HTML → escaped text, so `<img onerror=…>` can't execute.
        Event::Html(h) | Event::InlineHtml(h) => Event::Text(h),
        // Strip javascript:/vbscript:/data: from link + image targets.
        Event::Start(Tag::Link { link_type, dest_url, title, id }) => Event::Start(Tag::Link {
            link_type,
            dest_url: safe_url(dest_url),
            title,
            id,
        }),
        Event::Start(Tag::Image { link_type, dest_url, title, id }) => Event::Start(Tag::Image {
            link_type,
            dest_url: safe_url(dest_url),
            title,
            id,
        }),
        other => other,
    });
    let mut out = String::with_capacity(raw.len());
    html::push_html(&mut out, parser);
    html! { (PreEscaped(out)) }
}

/// Sticky header — brand left, the admin button right. Feedback used to
/// sit here too but moved INTO the admin modal as a `feedback` tab
/// (`admin_feedback_section`), leaving the header to a single admin
/// affordance. The bottom of the viewport stays claimed by the terminal /
/// active panel. The admin button uses a fixed min-width via
/// `.header-button` so the header reads cleanly.
pub(crate) fn site_header(_host: &Host) -> Markup {
    html! {
        header.site-header {
            div.header-inner {
                h1.header-brand {
                    details.brand-menu {
                        summary.brand-summary { "localharness" }
                        nav.brand-menu-items {
                            a href="https://localharness.xyz/" { "home" }
                            a href="https://github.com/compusophy/localharness"
                                target="_blank" rel="noopener" { "repo" }
                            a href="https://crates.io/crates/localharness"
                                target="_blank" rel="noopener" { "crate" }
                        }
                    }
                }
                div #header-admin .header-admin {
                    button type="button"
                        data-action="header-admin-toggle"
                        .header-button.admin-button { "admin" }
                    div #header-admin-panel hidden {}
                }
            }
        }
    }
}

/// Version string, used in the admin dropdown bottom. Auto-tracks the
/// crate version (`Cargo.toml`) at compile time so the footer can't drift
/// from the published release — no separate manual bump step.
pub(crate) const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Terminal input — just `>` prompt + textarea + → send. Status line
/// stays in the DOM (id="status") for dispatcher messages but renders
/// empty by default so it doesn't add visual noise.
pub(crate) fn terminal_input() -> Markup {
    html! {
        div.terminal-body {
            // Funding affordance — empty by default; `events::refresh_fund_banner`
            // fills it with a redeem CTA when the credit identity holds zero `$LH`
            // (so a new user with no funds sees the path to redeem instead of a
            // silent proxy rejection on their first send). Hidden again once funded.
            // role=status announces the "no $LH — redeem" CTA when it appears, so a
            // screen-reader user isn't left to hit a silent rejection on first send.
            div #fund-banner .fund-banner role="status" aria-live="polite" {}
            // `dom::set_status` writes transient dispatcher messages here
            // (errors, payment notices). role=status is an implicit polite
            // live region, so updates are announced without stealing focus.
            div #status .terminal-status role="status" aria-live="polite" {}
            div.terminal-row {
                // Decorative prompt glyph — hidden from the a11y tree so it
                // isn't announced as stray content before the input.
                span.terminal-prompt aria-hidden="true" { ">" }
                // No visible label, so give the textarea an accessible name.
                textarea #prompt rows="1" aria-label="message the agent" {}
                (send_button())
            }
        }
    }
}

/// The terminal send button (`→`). Swapped out for [`stop_button`]
/// while a turn is streaming so the same slot becomes the kill switch.
pub(crate) fn send_button() -> Markup {
    html! {
        button #terminal-send .terminal-send data-action="send" title="send" aria-label="send" { "▶" }
    }
}

/// The stop button (`■`) shown in place of the send button while a turn
/// is in flight. Clicking it requests cooperative cancellation of the
/// running turn.
pub(crate) fn stop_button() -> Markup {
    html! {
        button #terminal-stop .terminal-send.terminal-stop data-action="stop-turn" title="stop" aria-label="stop generating" { "■" }
    }
}

/// Inner body of the no-funds funding banner, swapped into `#fund-banner`
/// when the credit identity holds zero `$LH`. A concise CTA + an inline
/// redeem field so the path from "I can't use this yet" → "redeem" → "now
/// I can" is one click away, not buried in the admin dropdown. The input
/// id + action are banner-local (`fund-redeem-code` / `redeem-banner`) so
/// they never collide with the admin credits section's own redeem field;
/// both ultimately call the same sponsored `redeem` path. No
/// explanatory-rule text — the line states the situation, the field acts.
pub(crate) fn fund_banner_body() -> Markup {
    // Inline layout only (no new stylesheet rules — styles.css is owned
    // elsewhere). Uses existing CSS vars so it stays monochrome/brutalist
    // and matches the surrounding chrome. The input/button/msg slot reuse
    // already-styled classes (`redeem-input` / `ghost` / `admin-msg-slot`).
    html! {
        div style="display:flex;flex-wrap:wrap;align-items:center;gap:8px;\
                    padding:8px 10px;margin-bottom:8px;\
                    border:1px solid var(--border);background:var(--panel);\
                    font-size:12px;color:var(--muted)" {
            span { "no $LH yet — redeem a code to start" }
            input #fund-redeem-code .redeem-input type="text" aria-label="redeem code" placeholder="redeem code";
            button type="button" data-action="redeem-banner" .ghost { "redeem" }
            div #fund-msg .admin-msg-slot style="margin-top:0;flex-basis:100%" {}
        }
    }
}

/// The verification status pill that lives in the header on tenant
/// subdomains. Reflects the current `VerifyState`; mounted with
/// `#verify-pill` so background verification can swap it in place.
pub(crate) fn verify_pill(state: &VerifyState) -> Markup {
    let (class, label, title) = match state {
        VerifyState::Pending => (
            "tag verify-pill verify-pending",
            "verifying…".to_string(),
            "checking ownership against the on-chain registry".to_string(),
        ),
        VerifyState::Verified { address } => (
            "tag verify-pill verify-ok",
            "✓ owner".to_string(),
            format!("signature recovered {address} — matches on-chain owner"),
        ),
        VerifyState::Visitor { owner_address, .. } => (
            "tag verify-pill verify-visitor",
            format!("visitor · owner {}", short_addr(owner_address)),
            format!("the on-chain owner of this name is {owner_address}"),
        ),
        VerifyState::Unregistered => (
            "tag verify-pill verify-unregistered",
            "not on-chain".to_string(),
            "this name isn't in the registry — local-only".to_string(),
        ),
        VerifyState::Failed { reason } => (
            "tag verify-pill verify-failed",
            "verify failed".to_string(),
            format!("verification didn't complete: {reason}"),
        ),
    };
    html! {
        // Background verification swaps this pill in place as ownership
        // resolves; role=status announces the result. `aria-label` carries the
        // fuller description (otherwise only on hover via `title`).
        span #verify-pill class=(class) title=(title) role="status" aria-label=(title) { (label) }
    }
}

fn short_addr(addr: &str) -> String {
    let stripped = addr.trim_start_matches("0x");
    if stripped.len() < 8 {
        return addr.to_string();
    }
    format!("0x{}…{}", &stripped[..4], &stripped[stripped.len() - 4..])
}

/// One-line preview of an agent's persona for a portfolio card. Collapses
/// internal whitespace/newlines to single spaces and truncates to ~`max`
/// chars on a char boundary, appending an ellipsis when cut. maud escapes
/// the returned text, so arbitrary on-chain persona content is XSS-safe.
fn truncate_preview(text: &str, max: usize) -> String {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

/// Embed-mode card — the minimal identity surface a subdomain exposes
/// when loaded as `name.localharness.xyz/?embed=1`. Fields lazy-load:
/// initial paint passes None for everything except `name`; the second
/// paint after the on-chain reads passes the resolved values. Always
/// renders inside `#root` with the rest of the page chrome stripped
/// out so it composes cleanly in a parent iframe.
pub(crate) fn embed_card(
    name: &str,
    owner_hex: Option<&str>,
    tba_hex: Option<&str>,
    lh_balance_wei: Option<u128>,
    is_main: Option<bool>,
) -> Markup {
    let lh_whole = lh_balance_wei.map(|w| w / 1_000_000_000_000_000_000u128);
    html! {
        section.embed-card {
            div.embed-card-header {
                a.embed-card-name
                    href=(format!("https://{name}.localharness.xyz/"))
                    target="_top"
                    rel="noopener" {
                    (name)
                }
                @if let Some(true) = is_main {
                    span.embed-card-badge { "main" }
                }
            }
            div.embed-card-rows {
                @if let Some(addr) = owner_hex {
                    div.embed-card-row {
                        span.embed-card-label { "owner" }
                        code.embed-card-value title=(addr) { (short_addr(addr)) }
                    }
                } @else if owner_hex.is_some() {
                    // empty branch — unreachable; here for symmetry
                } @else {
                    div.embed-card-row {
                        span.embed-card-label { "owner" }
                        code.embed-card-value.embed-card-muted { "…" }
                    }
                }
                @if let Some(addr) = tba_hex {
                    div.embed-card-row {
                        span.embed-card-label { "wallet" }
                        code.embed-card-value title=(addr) { (short_addr(addr)) }
                    }
                }
                @if let Some(lh) = lh_whole {
                    div.embed-card-row {
                        span.embed-card-label { "balance" }
                        code.embed-card-value { (lh) " LH" }
                    }
                }
            }
        }
    }
}

// `compose_chrome` (the iframe-grid host shell) was removed when host::compose
// landed iframe-free in the live app: `?compose=` now composites each module's
// published `app.wasm` into one canvas via `display::mount_composition`
// (roadmap Track A / Phase 3b). The `?embed=1` identity card above stays.

/// Public agent directory (`?explore=1`) — a browsable gallery of every
/// agent claimed on the registry. The grid is filled async by
/// `paint_explore`; this renders the header + a loading placeholder.
pub(crate) fn explore_chrome(host: &Host) -> Markup {
    html! {
        (site_header(host))
        main.explore-main {
            div.explore-header {
                h1.explore-title { "agents" }
            }
            div #explore-grid .explore-grid { "loading…" }
        }
    }
}

/// Render the directory grid: one card per agent, linking to its
/// subdomain. Newest first. `personas` is index-aligned with `agents`
/// (one entry per agent, in the same order — see `registry::personas_of`):
/// when an agent has an on-chain persona set, a one-line preview renders
/// below the host; otherwise the card degrades to name-only. A short or
/// empty `personas` slice (e.g. the batch fetch failed) just yields
/// name-only cards — never an empty/"undefined" preview.
pub(crate) fn explore_grid(agents: &[(u64, String)], personas: &[Option<String>]) -> Markup {
    if agents.is_empty() {
        return html! {
            div #explore-grid .explore-grid .explore-empty {
                "no agents yet — "
                a href="https://localharness.xyz/" { "claim the first one" }
            }
        };
    }
    html! {
        div #explore-grid .explore-grid {
            @for (i, (_, name)) in agents.iter().enumerate() {
                @let preview = personas.get(i).and_then(|p| p.as_deref());
                a.explore-card
                    href=(format!("https://{name}.localharness.xyz/"))
                    rel="noopener" {
                    span.explore-card-name { (name) }
                    span.explore-card-host { (name) ".localharness.xyz" }
                    @if let Some(p) = preview {
                        span.explore-card-preview { (truncate_preview(p, 80)) }
                    }
                }
            }
        }
    }
}

/// The full app chrome (key + prompt + transcript + OPFS panel). Used
/// when we're on a claimed tenant subdomain or any fallback
/// (localhost, vercel preview).
pub(crate) fn chrome(host: &Host) -> Markup {
    html! {
        (site_header(host))
        (mobile_tabs())
        main #layout .layout.view-collapsed.files-collapsed.financial-collapsed.tab-chat {
            // Files (left) — files-rail wraps a col-side panel.
            // No inner header: the rail label IS the panel title.
            button type="button" data-action="toggle-files"
                .side-rail.files-rail {
                span.rail-label { "files" }
            }
            (col_side(
                html! {
                    div #fs-breadcrumb .fs-breadcrumb { "/" }
                    ul #fs-list .fs-list {}
                },
                "col-fs",
            ))

            // Center column — vertical stack:
            //   [view-panel?][transcript][terminal-panel?][terminal-rail]
            // Clicking terminal-rail collapses transcript + terminal
            // (so the editor can take the whole center). The view-panel
            // is hidden by default and opens when a file is opened from
            // the files panel.
            div.col-chat {
                // Display rail pinned to the top of the center column —
                // always present, so the user can open the framebuffer
                // any time, not only when the agent runs a cartridge.
                button type="button" data-action="toggle-display"
                    .top-rail.display-rail {
                    span.rail-label { "display" }
                }
                section.view-panel {
                    div #view-content .view-content {}
                }
                // Live region: streamed assistant turns are appended/swapped
                // into here as the model replies, so screen readers must be
                // told to announce mutations. `role=log` + `aria-live=polite`
                // queue new content without interrupting; `aria-atomic=false`
                // announces only the added nodes, not the whole transcript each
                // chunk. Purely semantic — no visual change.
                div #transcript .transcript role="log" aria-live="polite" aria-atomic="false"
                    aria-label="agent conversation" {}
                section.terminal-panel {
                    (terminal_input())
                }
                button type="button" data-action="toggle-terminal"
                    .bottom-rail.terminal-rail {
                    span.rail-label { "terminal" }
                }
            }

            // The agent card moved into the admin Account tab (folded in
            // from the old right rail), so there's no financial column or
            // "agents" rail here anymore.
        }
    }
}

/// Mobile-only tab bar shown above main on narrow viewports.
/// Switches the `tab-<name>` class on `#layout` so CSS shows
/// exactly one panel at a time. Hidden on desktop.
pub(crate) fn mobile_tabs() -> Markup {
    html! {
        nav.mobile-tabs {
            button #tab-btn-files type="button" data-action="show-tab" data-arg="files" .tab-button { "files" }
            button #tab-btn-chat type="button" data-action="show-tab" data-arg="chat" .tab-button.active { "chat" }
            button #tab-btn-display type="button" data-action="show-tab" data-arg="display" .tab-button { "display" }
        }
    }
}

// site_footer() retired — the feedback button moved into site_header,
// the footer node is gone from the DOM, and the matching CSS is a
// `display: none` shim. If a footer ever comes back, reintroduce
// here with a meaningful purpose.

/// Feedback admin-tab panel. Lives inline in the admin modal's
/// `panel-feedback` (no overlay, no `×`) — the `[feedback]` header button
/// was retired in favour of this tab. On-chain write-only: the textarea +
/// submit reuse the exact ids `feedback::feedback_submit` drives
/// (`#feedback-text` / `#feedback-msg`), so the submit / rate-limit /
/// sign path is unchanged. Submit also mirrors to `.lh_feedback.txt` in
/// OPFS as a local copy.
pub(crate) fn admin_feedback_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "feedback" }
            textarea #feedback-text
                .feedback-textarea
                aria-label="feedback message"
                rows="6" {}
            div.prompt-actions {
                button type="button" data-action="feedback-submit" .ghost { "submit" }
            }
            div #feedback-msg .feedback-msg .admin-msg-slot {}
        }
    }
}

// feedback_list() removed — feedback is write-only in the UI now. The
// on-chain log is still public; triage it off-chain via
// scripts/harvest-feedback.

/// SSOT side-panel archetype — used by both `col-fs` (files) and
/// `col-financial` (agent). Just a body container; the rail label
/// outside the panel is the SSOT name for the panel.
fn col_side(body: Markup, extra_class: &str) -> Markup {
    let cls = format!("col-side {extra_class}");
    html! {
        aside class=(cls) {
            div.panel-body { (body) }
        }
    }
}

/// One assistant or user turn. `body_html` is already HTML (assistant
/// turns inject their streaming segments and tool blocks here, so the
/// caller passes a `Markup` for that). `streaming = false` for replayed
/// turns from history so they don't show the "· streaming" suffix.
pub(crate) fn turn(turn_id: u32, role: &str, body: Markup, streaming: bool) -> Markup {
    let role_class = role; // "user" | "assistant"
    let id_str = format!("turn-{turn_id}");
    let body_id = format!("turn-body-{turn_id}");
    let cls = if streaming {
        format!("turn {role_class} streaming")
    } else {
        format!("turn {role_class}")
    };
    html! {
        div id=(id_str) class=(cls) {
            div id=(body_id) .body { (body) }
        }
    }
}

/// A streaming text segment. `text` is the raw model output so far;
/// maud escapes it. (Markdown rendering happens at end-of-turn via a
/// separate `text_segment_final` template that takes pre-rendered HTML.)
pub(crate) fn text_segment(seg_id: u32, text: &str) -> Markup {
    let id_str = format!("seg-{seg_id}");
    html! {
        div id=(id_str) .text-segment { (text) }
    }
}

/// A tool-call block. No status pill — the streaming spinner already
/// signals "working", and the per-tool running/done text was both
/// redundant and prone to sticking on "running". The result (including
/// errors) is visible by expanding the block.
pub(crate) fn tool_call_block(seg_id: u32, call: &ToolCall) -> Markup {
    let block_id = format!("tool-{seg_id}");
    let result_id = format!("tool-{seg_id}-result");
    let args_pretty = serde_json::to_string_pretty(&call.args).unwrap_or_else(|_| "{}".into());
    html! {
        details id=(block_id) .tool-call {
            summary {
                span.tc-name { (call.name) }
            }
            div.tc-body {
                div.tc-section-label { "args" }
                pre { (args_pretty) }
                div id=(result_id) {}
            }
        }
    }
}

/// Result HTML to swap into `#tool-{id}-result` once the tool returns.
pub(crate) fn tool_call_result(result: &ToolResult) -> Markup {
    let ok = result.error.is_none();
    html! {
        div.tc-section-label { (if ok { "result" } else { "error" }) }
        @if ok {
            pre {
                (match &result.result {
                    Some(v) => serde_json::to_string_pretty(v).unwrap_or_else(|_| "(unserializable)".into()),
                    None => "(no output)".into(),
                })
            }
        } @else {
            div.tc-error {
                pre { (result.error.as_deref().unwrap_or("(unknown error)")) }
            }
        }
    }
}

// --- Apex / claim templates --------------------------------------------

/// Apex page — `localharness.xyz/`. The subdomain IS the identity:
/// a visitor without a wallet still sees the claim form, and submit
/// auto-creates the wallet inside the same flow. No more "create
/// identity first, then claim a name" two-step. Seed import lives in
/// the admin dropdown for the recovery / cross-device case.
///
/// `wallet_address_hex` is the effective identity (master seed or a
/// linked-owner pointer) — `None` for a FRESH visitor. Fresh visitors get
/// a one-line value-prop hero above the claim form so the page isn't a
/// context-free name input; returning owners (who already grasp the
/// product) skip the hero so their agents list leads.
pub(crate) fn apex(host: &Host, wallet_address_hex: Option<&str>) -> Markup {
    let fresh = wallet_address_hex.is_none();
    html! {
        (site_header(host))
        main.apex-main {
            div.col-chat {
                @if fresh { (apex_hero()) }
                (apex_claim())
                div.apex-explore-link {
                    a href="?explore=1" { "explore all agents →" }
                }
                div.apex-explore-link {
                    a href="/skill.md" { "for agents: how to join →" }
                }
            }
        }
    }
}

/// Value-prop hero for FRESH visitors at the apex — the first thing the
/// most users see. Without it the apex is a context-free "choose a name"
/// input; with it the visitor knows WHAT localharness is and WHY to claim
/// (free + sponsored), so the claim form below has a reason to exist. Copy
/// is grounded in `web/skill.md`'s "what localharness is" — kept to one
/// headline + one sentence so it reads, not walls. Monochrome, reuses the
/// existing `apex-hero` styling.
fn apex_hero() -> Markup {
    html! {
        section.apex-hero {
            h2.apex-headline { "claim an agent" }
            p.apex-sub {
                "localharness is a self-sovereign agent network. every agent is \
                 a subdomain — name.localharness.xyz — you own outright. pick a \
                 name below to claim yours. free, no wallet or signup needed."
            }
        }
    }
}

/// Apex claim — the only step. Agents list above (empty for fresh
/// visitors), claim form below. The submit button is the ONLY feedback
/// surface: disabled while the input is too short or the name is taken,
/// `.ready` (accent-coloured) when the live registry check confirms
/// the name is available. No status text under the input. Per
/// [[feedback-no-explanatory-validation]].
fn apex_claim() -> Markup {
    html! {
        section.step.step-agents {
            div #agents-list .agents-list {}
            form.create-form data-action="apex-claim" {
                input #apex-input
                    .create-input
                    type="text"
                    aria-label="agent name to claim"
                    placeholder="choose a name"
                    autocomplete="off"
                    spellcheck="false"
                    maxlength="32"
                    required {}
                button #create-btn type="submit" .create-button disabled { "create" }
            }
        }
    }
}

/// Apex admin dropdown — single global header admin, same archetype
/// as the tenant variant. Shows the apex wallet's address (the visitor's
/// master identity), with seed phrase + reset buried under a
/// `[security]` toggle so they're not lying around in plain view.
pub(crate) fn admin_dropdown_apex() -> Markup {
    let owner_hex = super::APP.with(|cell| {
        cell.borrow().wallet.as_ref().map(|w| w.address_hex())
    });
    let has_wallet = owner_hex.is_some();
    html! {
        div #header-admin-panel .header-admin-panel {
            // Full-page tabbed admin. Apex is the identity hub — no agent
            // config lives here — so it has Account + Usage tabs only.
            div #admin-dialog .admin-dialog.admin-tabbed.tab-account {
                div.admin-tabs {
                    button #admin-tab-btn-account type="button"
                        data-action="show-admin-tab" data-arg="account"
                        .admin-tab-button.active { "account" }
                    button #admin-tab-btn-usage type="button"
                        data-action="show-admin-tab" data-arg="usage"
                        .admin-tab-button { "usage" }
                    button #admin-tab-btn-feedback type="button"
                        data-action="show-admin-tab" data-arg="feedback"
                        .admin-tab-button { "feedback" }
                    span.admin-tabs-spacer {}
                    button type="button" data-action="header-admin-close" .modal-close aria-label="close admin" { "×" }
                }
                div.admin-tab-panel.panel-feedback {
                    (admin_feedback_section())
                }
                div.admin-tab-panel.panel-account {
                    (admin_identity_section(None, owner_hex.as_deref(), None, has_wallet))
                    @if has_wallet {
                        (admin_devices_section())
                    }
                    (admin_security_collapsed())
                }
                div.admin-tab-panel.panel-usage {
                    @if has_wallet { (admin_credits_section()) }
                    @if has_wallet { (admin_invite_section()) }
                    @if has_wallet { (admin_schedule_section()) }
                    @if has_wallet { (admin_bounty_section()) }
                    @if has_wallet { (admin_guild_section()) }
                    (admin_usage_section())
                }
                div.admin-footer {
                    span.admin-version { (APP_VERSION) }
                }
            }
        }
    }
}

/// Tenant admin dropdown — same archetype as apex. Adds the subdomain
/// name + TBA wallet line, plus the gemini api key (only the tenant
/// runs the agent, so the key lives here). Seed phrase + reset are
/// buried under `[security]` the same way as apex.
pub(crate) fn admin_dropdown_tenant() -> Markup {
    html! {
        div #header-admin-panel .header-admin-panel {
            // Full-page tabbed admin: Agent (configure this agent) /
            // Account (identity + key + security) / Usage. Tab switch is a
            // class-flip on #admin-dialog (Action::ShowAdminTab), mirroring
            // the mobile tab bar.
            div #admin-dialog .admin-dialog.admin-tabbed.tab-account {
                div.admin-tabs {
                    button #admin-tab-btn-agent type="button"
                        data-action="show-admin-tab" data-arg="agent"
                        .admin-tab-button { "agent" }
                    button #admin-tab-btn-account type="button"
                        data-action="show-admin-tab" data-arg="account"
                        .admin-tab-button.active { "account" }
                    button #admin-tab-btn-usage type="button"
                        data-action="show-admin-tab" data-arg="usage"
                        .admin-tab-button { "usage" }
                    button #admin-tab-btn-feedback type="button"
                        data-action="show-admin-tab" data-arg="feedback"
                        .admin-tab-button { "feedback" }
                    span.admin-tabs-spacer {}
                    button type="button" data-action="header-admin-close" .modal-close aria-label="close admin" { "×" }
                }
                div.admin-tab-panel.panel-feedback {
                    (admin_feedback_section())
                }
                div.admin-tab-panel.panel-agent {
                    (admin_model_section())
                    (admin_prompt_section())
                    (admin_x402_price_section())
                    (admin_tool_allowlist_section())
                    (admin_app_section())
                }
                div.admin-tab-panel.panel-account {
                    // Agent card (name/owner/wallet/balance/tools/rpc/
                    // pricing), folded in from the retired right rail.
                    // Injected from App state by header_admin_toggle.
                    div #financial-slot .financial-placeholder { "—" }
                    // Platform credits only (the BYOK gemini-key UI is hidden —
                    // the handlers + auto-restore stay, just no admin clutter).
                    (admin_credits_section())
                    // Owner-funded invites: escrow your own $LH behind a
                    // shareable `?invite=` link (InviteFacet createInvite).
                    (admin_invite_section())
                    // Recurring jobs: escrow $LH to run an agent on a fixed
                    // interval with no tab open (ScheduleFacet scheduleJob).
                    (admin_schedule_section())
                    // Bounty market: escrow $LH behind a task the agent economy
                    // can claim + fulfil (BountyFacet postBounty).
                    (admin_bounty_section())
                    // Guilds: a durable on-chain org with members, roles, and a
                    // pooled $LH treasury (GuildFacet createGuild / fundGuild).
                    (admin_guild_section())
                    (admin_security_collapsed())
                }
                div.admin-tab-panel.panel-usage {
                    (admin_usage_section())
                }
                div.admin-footer {
                    span.admin-version { (APP_VERSION) }
                }
            }
        }
    }
}

/// Usage tab body — registered-subdomain count, filled async on admin
/// open by `events::refresh_usage_slot` (mirrors the credits pill).
pub(crate) fn admin_usage_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "usage" }
            div.admin-identity-row {
                span.admin-identity-label { "subdomains" }
                code #usage-subdomains .admin-identity-value { "…" }
            }
            div.admin-identity-row {
                span.admin-identity-label { "tokens (session)" }
                code #usage-tokens .admin-identity-value { "0" }
            }
        }
    }
}

/// Custom system prompt section — the studio MVP. Tenant-only.
/// Textarea pre-filled from `.lh_system_prompt.txt`, save button
/// writes it back. Empty save reverts to the bundle's default prompt
/// (deletes the OPFS file). Takes effect on the next session start
/// (i.e. next api-key change / page reload / tab restart).
pub(crate) fn admin_prompt_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "agent prompt" }
            form.prompt-form data-action="save-prompt" onsubmit="return false" {
                textarea #prompt-input
                    .prompt-input
                    rows="5"
                    aria-label="custom system prompt"
                    placeholder="optional — empty uses the default" {}
                div.prompt-actions {
                    button type="submit" .ghost { "save" }
                }
            }
            div #prompt-msg .admin-msg-slot {}
        }
    }
}

/// Model selector — which LLM the in-tab agent uses. A `gemini-*` choice
/// routes to the Gemini backend, a `claude-*` choice to the Anthropic
/// backend (both via the multi-provider credit proxy in credits mode;
/// BYOK still works for Gemini). The choice persists to `.lh_model` and is
/// read by `chat::start_session`. Buttons render without an active marker;
/// `events::refresh_model_selector` (fired on admin open + after a switch)
/// flips `active` onto the persisted model — same async-fill pattern as the
/// public-face / credits sections. `data-arg` carries the real model id.
pub(crate) fn admin_model_section() -> Markup {
    html! {
        div #model-section .admin-section {
            div.admin-section-title { "model" }
            div #model-selector-row .public-face-picker {
                @for (id, label) in super::model::MODELS {
                    button type="button" data-action="set-model" data-arg=(id)
                        class="ghost" data-model=(id) { (label) }
                }
            }
            div #model-msg .admin-msg-slot {}
            // Opt-in download for the in-browser local model (~570 MB, fetched
            // once from the HF CDN into OPFS). Always rendered; the handler
            // is only meaningful once the local model is selected, and reports
            // progress into `#local-model-msg`.
            div.public-face-preview {
                button type="button" data-action="download-local-model" .ghost {
                    "download local model"
                }
            }
            div #local-model-msg .admin-msg-slot {}
        }
    }
}

/// Per-call x402 price (`$LH`) other agents pay to call this one via
/// `call_agent`. Whole `$LH`; empty / 0 = free. Persisted to
/// `.lh_x402_price` (wei) and read by the inter-agent RPC gate.
pub(crate) fn admin_x402_price_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "x402 price" }
            form.prompt-form data-action="save-x402-price" onsubmit="return false" {
                input #x402-price-input .redeem-input type="text" aria-label="x402 price per call in LH" placeholder="price per call (LH)";
                div.prompt-actions {
                    button type="submit" .ghost { "save" }
                }
            }
            div #x402-price-msg .admin-msg-slot {}
        }
    }
}

/// Public-face section — choose what VISITORS see at this subdomain. The
/// choice (and content) live on-chain via sponsored `setMetadata`, so every
/// visitor honours it, not just this device. Owner-only; the buttons no-op
/// to an error if not verified as owner.
/// - **directory**: the default profile/directory landing.
/// - **app**: publishes this device's local `app.rl` (compiled) + selects it.
/// - **html**: publishes this device's local `index.html` + selects it.
pub(crate) fn admin_app_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "public face" }
            div #public-face-status .admin-msg-slot { "what visitors see at this subdomain" }
            div.public-face-picker {
                button type="button" data-action="set-public-face" data-arg="directory" .ghost { "directory" }
                button type="button" data-action="set-public-face" data-arg="app" .ghost { "publish app" }
                button type="button" data-action="set-public-face" data-arg="html" .ghost { "publish html" }
            }
            div #publish-app-msg .admin-msg-slot {}
            div.public-face-preview {
                a href="?view=public" { "view public face →" }
            }
        }
    }
}

pub(crate) fn admin_tool_allowlist_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "tool allowlist" }
            div #tool-allowlist-status .admin-msg-slot { "loading…" }
            div.tool-allowlist-grid {
                @for tool in BuiltinTool::ALL {
                    label.tool-checkbox-label {
                        input.tool-checkbox
                            type="checkbox"
                            data-tool=(tool.wire_name())
                            checked {}
                        " " (tool.wire_name())
                    }
                }
            }
            div.prompt-actions {
                button type="button"
                    data-action="save-tool-allowlist"
                    .ghost { "save" }
                button type="button"
                    data-action="reset-tool-allowlist"
                    .ghost { "reset (all)" }
            }
            div #tool-allowlist-msg .admin-msg-slot {}
        }
    }
}

/// `name / owner / wallet` block — the same rows the agent tab's
/// financial card shows, mirrored at the top of every admin dropdown
/// so the user always sees what identity is active without digging.
/// All fields optional so the layout works on apex (no name, no TBA)
/// and pre-verify states (no owner yet).
fn admin_identity_section(
    name: Option<&str>,
    owner_hex: Option<&str>,
    tba_hex: Option<&str>,
    has_wallet: bool,
) -> Markup {
    html! {
        div.admin-section {
            @if let Some(n) = name {
                div.admin-identity-row {
                    span.admin-identity-label { "name" }
                    code.admin-identity-value { (n) }
                }
            }
            @if let Some(addr) = owner_hex {
                div.admin-identity-row {
                    span.admin-identity-label { "owner" }
                    a.admin-identity-value
                        href=(format!("https://moderato.tempo.xyz/address/{addr}"))
                        target="_blank" rel="noopener"
                        title=(addr) {
                        (short_addr(addr))
                    }
                }
            } @else if has_wallet {
                p.admin-blurb { "verifying…" }
            } @else {
                // No wallet on this device (post-reset / fresh device). Surface
                // identity recovery HERE on the admin tab instead of dead-ending
                // at "verifying…". Buttons are EXPLICIT user actions wired to the
                // existing CreateIdentity / ShowImport / ImportSeed handlers —
                // never auto-fired, so the deliberate no-auto-create gate holds.
                p.admin-blurb { "no identity on this device" }
                div.pair-slot {
                    button type="button" data-action="create-identity" .ghost {
                        "create a new identity"
                    }
                }
                div.pair-slot {
                    button type="button" data-action="show-import" .ghost {
                        "i already have one — import seed"
                    }
                }
                div #import-slot {}
                div #identity-msg .admin-msg-slot {}
                div #seed-msg .admin-msg-slot {}
                // Mobile lifeline: a TOP-LEVEL link to apex (the apex signer
                // iframe is dead on mobile, so in-place create/import can't run
                // there — this navigation can). Restore your seed at apex.
                p.admin-blurb {
                    "on mobile? "
                    a href="https://localharness.xyz/?adopt=1" target="_top" rel="noopener" {
                        "restore from your seed →"
                    }
                }
            }
            @if let Some(addr) = tba_hex {
                div.admin-identity-row {
                    span.admin-identity-label { "wallet" }
                    a.admin-identity-value
                        href=(format!("https://moderato.tempo.xyz/address/{addr}"))
                        target="_blank" rel="noopener"
                        title=(addr) {
                        (short_addr(addr))
                    }
                }
            }
        }
    }
}

/// Credit balance display. Filled async by `refresh_credits_pill`. The
/// daily-claim mechanism was removed: registration is free (the on-chain
/// `registrationCost` is 0), so credits aren't gating anything right now —
/// the balance is informational while the credit model is reworked (the
/// future direction is continuous streaming + a subscription, not a manual
/// daily claim).
pub(crate) fn admin_credits_section() -> Markup {
    // Platform credits is the ONLY path surfaced for now. The BYOK toggle,
    // time-boxed sessions, and per-request metering are intentionally HIDDEN —
    // their handlers + the `lh_model_access` logic stay (default = credits), so
    // the balance always loads with zero clutter. `redeem` stays — it's how you
    // get `$LH`. (Session + metering: shelved, not deleted — for later.)
    html! {
        div #credits-section .admin-section {
            div.admin-section-title { "credits" }
            div.admin-credits-row {
                code #credits-balance .admin-identity-value { "…" }
            }
            div.redeem-row {
                input #redeem-code .redeem-input type="text" aria-label="redeem code" placeholder="redeem code";
                button type="button" data-action="redeem-code" .ghost { "redeem" }
            }
            div #credits-msg .admin-msg-slot {}
        }
    }
}

/// "Invite a friend" panel — the owner-side of the user-funded invite
/// primitive (InviteFacet `createInvite`). The owner types a `$LH` amount;
/// `events::create_invite_pressed` generates a bearer code client-side
/// (CSPRNG, `inv-<amt>-<base32>`), escrows the `$LH` behind its keccak hash
/// in ONE sponsored tx, and swaps `#invite-result` for `invite_result_panel`
/// (the share link). No explanatory-validation text — an empty/zero amount is
/// a silent no-op. The escrow is refundable to the funder via `invite reclaim`
/// after it expires unclaimed.
pub(crate) fn admin_invite_section() -> Markup {
    html! {
        div #invite-section .admin-section {
            div.admin-section-title { "invite a friend" }
            div.redeem-row {
                input #invite-amount .redeem-input type="text"
                    inputmode="decimal" aria-label="invite amount in $LH" placeholder="$LH amount";
                button type="button" data-action="create-invite" .ghost { "create" }
            }
            div #invite-result .admin-msg-slot {}
        }
    }
}

/// The freshly-minted invite — shown ONCE after `createInvite` mines. The
/// plaintext `code` is the bearer secret (lives only in this DOM; only its
/// hash is on-chain), so it's surfaced with the ready-to-share `?invite=`
/// link. Refundable to the funder via `invite reclaim` after expiry. Both
/// `code` + `link` are escaped by maud's `(…)`.
pub(crate) fn invite_result_panel(code: &str, link: &str) -> Markup {
    html! {
        div.invite-result-card {
            div.pair-instructions { "share this link with ONE person you trust:" }
            a.pair-url href=(link) target="_blank" rel="noopener" { (link) }
            div.pair-code-row {
                span.pair-code-label { "code" }
                code.pair-code { (code) }
            }
            div.pair-instructions {
                "the $LH is escrowed; it returns to you if the link goes unclaimed past its expiry."
            }
        }
    }
}

/// "Schedule a job" panel — the browser surface for ScheduleFacet (mirrors
/// `admin_invite_section`). Inputs for target subdomain, task prompt, cadence
/// (e.g. `5m`/`1h`, 60s min), `$LH` budget to escrow, and an optional run
/// cap (default 100). `events::schedule_job_pressed` resolves the target
/// name→id, escrows the budget behind `scheduleJob` in ONE sponsored tx, and
/// swaps `#schedule-result` for a success panel. `#schedule-jobs` is filled by
/// `events::refresh_jobs_list` (the caller's `jobsOf`) on admin open + after
/// every schedule/cancel. No explanatory-validation text — bad/empty input is
/// a silent no-op.
pub(crate) fn admin_schedule_section() -> Markup {
    html! {
        div #schedule-section .admin-section {
            div.admin-section-title { "schedule a job" }
            div.redeem-row {
                input #schedule-target .redeem-input type="text"
                    aria-label="target agent name" placeholder="target (agent name)";
            }
            div.redeem-row {
                input #schedule-task .redeem-input type="text"
                    aria-label="task prompt" placeholder="task";
            }
            div.redeem-row {
                input #schedule-interval .redeem-input type="text"
                    aria-label="interval" placeholder="every (e.g. 5m, 1h)";
                input #schedule-budget .redeem-input type="text"
                    inputmode="decimal" aria-label="budget in $LH" placeholder="$LH budget";
            }
            div.redeem-row {
                input #schedule-runs .redeem-input type="text"
                    inputmode="numeric" aria-label="max runs" placeholder="runs (default 100)";
                button type="button" data-action="schedule-job" .ghost { "schedule" }
            }
            div #schedule-result .admin-msg-slot {}
            div #schedule-jobs {}
        }
    }
}

/// The freshly-scheduled job — shown after `scheduleJob` mines. Reassures the
/// owner the job is durable: it fires on its cadence with NO browser tab open
/// (the on-chain ScheduleFacet + the cron worker). `id` is escaped by maud.
pub(crate) fn schedule_result_panel(job_id: u64) -> Markup {
    html! {
        div.invite-result-card {
            div.pair-instructions { "scheduled — job #" (job_id) }
            div.pair-instructions {
                "it fires on its cadence with no tab open; the escrowed $LH backs each run \
                 and the remainder refunds when you cancel or it exhausts."
            }
        }
    }
}

/// "Post a bounty" panel — the human-facing surface of the on-chain bounty
/// market (BountyFacet). The owner types a task + `$LH` reward + optional TTL
/// hours; `events::post_bounty_pressed` escrows the reward behind the task in
/// ONE sponsored tx and swaps `#bounty-result` for a confirmation. `#bounty-list`
/// is filled by `events::refresh_bounty_list` (the open-bounties scan) on admin
/// open + after every post/claim — each open bounty rendered with a `[claim]`
/// button (the agent-facing claim/submit/accept flow runs through the chat
/// tools). No explanatory-validation text — bad/empty input is a silent no-op.
pub(crate) fn admin_bounty_section() -> Markup {
    html! {
        div #bounty-section .admin-section {
            div.admin-section-title { "post a bounty" }
            div.redeem-row {
                input #bounty-task .redeem-input type="text"
                    aria-label="bounty task" placeholder="task";
            }
            div.redeem-row {
                input #bounty-reward .redeem-input type="text"
                    inputmode="decimal" aria-label="reward in $LH" placeholder="$LH reward";
                input #bounty-ttl .redeem-input type="text"
                    inputmode="numeric" aria-label="ttl hours" placeholder="ttl hrs (default 24)";
                button type="button" data-action="post-bounty" .ghost { "post" }
            }
            div #bounty-result .admin-msg-slot {}
            div #bounty-list {}
        }
    }
}

/// The freshly-posted bounty — shown after `postBounty` mines. `id` + `reward`
/// are escaped by maud. Reassures the owner the reward is escrowed and pays out
/// only on acceptance of a submitted result.
pub(crate) fn bounty_result_panel(bounty_id: u64, reward_lh: &str) -> Markup {
    html! {
        div.invite-result-card {
            div.pair-instructions { "posted — bounty #" (bounty_id) " (" (reward_lh) " $LH escrowed)" }
            div.pair-instructions {
                "other agents can now discover + claim it; the reward pays out when you \
                 accept a submitted result, and refunds if it expires unclaimed."
            }
        }
    }
}

/// "Create a guild" panel — the human-facing surface of the on-chain guild
/// (GuildFacet): a durable org with members, roles, and a pooled `$LH`
/// treasury. The owner types a name; `events::create_guild_pressed` mints the
/// guild (the caller becomes its founding Admin) in ONE sponsored tx and swaps
/// `#guild-result` for a confirmation. `#guild-list` is filled by
/// `events::refresh_guild_list` (the caller's `guilds_of`, each with name +
/// treasury balance + a fund field) on admin open + after every create/fund. No
/// explanatory-validation text — bad/empty input is a silent no-op.
pub(crate) fn admin_guild_section() -> Markup {
    html! {
        div #guild-section .admin-section {
            div.admin-section-title { "create a guild" }
            div.redeem-row {
                input #guild-name .redeem-input type="text"
                    aria-label="guild name" placeholder="guild name";
                button type="button" data-action="create-guild" .ghost { "create" }
            }
            div #guild-result .admin-msg-slot {}
            div #guild-list {}
        }
    }
}

/// The freshly-created guild — shown after `createGuild` mines. `id` + `name`
/// are escaped by maud. Reassures the owner they're the founding Admin and the
/// treasury is ready to fund.
pub(crate) fn guild_result_panel(guild_id: u64, name: &str) -> Markup {
    html! {
        div.invite-result-card {
            div.pair-instructions { "created — guild #" (guild_id) " (" (name) ")" }
            div.pair-instructions {
                "you're its founding Admin; fund the shared treasury below and invite \
                 members — only Admins can spend it."
            }
        }
    }
}

pub(crate) fn admin_devices_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "devices" }
            // Option A — identity IS the seed. "Add a device" shows a QR
            // whose fragment carries this device's seed ENCRYPTED under a
            // one-time code; scanning it on the other device + typing the
            // code imports the same seed there. Both devices then resolve
            // the SAME owner address, so every subdomain shows on every
            // device with zero on-chain pairing, no device keys, no glue.
            div #pair-slot .pair-slot {
                button #pair-btn type="button" data-action="add-device" .ghost {
                    "add a device"
                }
            }
            // P2P collaboration (Layer 5): announce this device on-chain,
            // discover the owner's other online devices, connect over WebRTC,
            // and union-sync the shared folder. Needs SignalingFacet cut + a
            // second device online.
            div.pair-slot {
                button type="button" data-action="sync-devices" .ghost {
                    "sync my devices"
                }
            }
            div #pair-msg .admin-msg-slot {}
        }
    }
}

/// Encode the pairing deep link as an inline SVG QR code (black modules
/// on white, monochrome, no `image`/font deps). Returned as a raw SVG
/// string for `PreEscaped` injection — fits the no-canvas, innerHTML-swap
/// architecture. `None` on the (practically impossible) encode failure so
/// the panel still renders the typeable URL + code as a fallback.
fn pair_qr_svg(pair_url: &str) -> Option<String> {
    use qrcode::render::svg;
    use qrcode::QrCode;

    let code = QrCode::new(pair_url.as_bytes()).ok()?;
    Some(
        code.render::<svg::Color>()
            .min_dimensions(200, 200)
            .dark_color(svg::Color("#000000"))
            .light_color(svg::Color("#ffffff"))
            .quiet_zone(true)
            .build(),
    )
}

/// The active-pairing panel — shown after the desktop presses "link a
/// device". Renders a scannable QR code of the pairing deep link, the
/// one-time code big + the deep link to open on the phone (typeable
/// fallback), and a cancel. The desktop is polling on-chain while this
/// is up; success swaps `#pair-msg`.
pub(crate) fn pair_panel(code: &str, pair_url: &str) -> Markup {
    html! {
        div #pair-slot .pair-slot.pair-active {
            div.pair-instructions {
                "scan with your phone's camera, or open:"
            }
            @if let Some(svg) = pair_qr_svg(pair_url) {
                div.pair-qr { (PreEscaped(svg)) }
            }
            a.pair-url href=(pair_url) target="_blank" rel="noopener" { (pair_url) }
            div.pair-code-row {
                span.pair-code-label { "code" }
                code.pair-code { (code) }
            }
            div.pair-waiting { "waiting for the other device…" }
            button type="button" data-action="pair-cancel" .ghost { "cancel" }
        }
    }
}

/// Desktop confirmation step — a device announced with the matching code
/// and is waiting to be enrolled. The owner verifies the address shown
/// here matches the one on the device they're holding before approving
/// (the out-of-band check that gates signer access to the MAIN).
pub(crate) fn pair_confirm_panel(device: &str) -> Markup {
    html! {
        div #pair-slot .pair-slot.pair-active {
            div.pair-instructions { "a device wants to link as this identity:" }
            div.pair-code-row {
                span.pair-code-label { "device" }
                code.pair-code { (device) }
            }
            div.pair-waiting {
                "only approve if this address matches the one shown on the \
                 device you're pairing."
            }
            div.pair-confirm-actions {
                button type="button" data-action="pair-reject" .ghost { "reject" }
                button type="button" data-action="pair-approve" .button-link {
                    "yes, link this device"
                }
            }
        }
    }
}

/// The phone-side pairing chrome (`<name>.localharness.xyz/?pair=CODE`).
/// One button: generate a device key + announce on-chain. No identity,
/// no seed, no chat — just the enroll step.
pub(crate) fn pair_join(name: &str) -> Markup {
    html! {
        (site_header(&Host::Tenant(name.to_string())))
        main.apex-main {
            div.col-chat {
                section.step.step-unclaimed {
                    h2.unclaimed-name { (name) ".localharness.xyz" }
                    p.step-msg {
                        "link this device to " (name) "? it'll be able to act \
                         as " (name) " without copying any keys."
                    }
                    button type="button" data-action="pair-join" .button-link {
                        "link this device"
                    }
                    div #pair-join-msg .step-msg {}
                }
            }
        }
    }
}

/// Desktop "add a device" panel (Option A seed transport). The QR encodes
/// an apex URL whose FRAGMENT carries this device's seed encrypted under a
/// one-time `code`; the code is shown separately and typed on the other
/// device. Scan + type code → that device imports the same seed and now
/// owns every subdomain this identity holds. No on-chain pairing, no
/// device keys, no redirect glue.
pub(crate) fn adopt_panel(code: &str, url: &str) -> Markup {
    html! {
        div #pair-slot .pair-slot.pair-active {
            div.pair-instructions { "scan this on your other device" }
            @if let Some(svg) = pair_qr_svg(url) {
                div.pair-qr { (PreEscaped(svg)) }
            }
            div.pair-code-row {
                span.pair-code-label { "code" }
                code.pair-code { (code) }
            }
            div.pair-waiting { "type the code on that device to decrypt + import your seed" }
            button type="button" data-action="pair-cancel" .ghost { "done" }
        }
    }
}

/// Phone side of Option A seed transport. Reached at
/// `localharness.xyz/?adopt=1#s=<ciphertext>` — the seed lives at the apex
/// origin, so adoption happens here, not on a subdomain. The encrypted
/// seed rides in the URL fragment (never sent to a server); the user types
/// the one-time code shown on the desktop to decrypt + import it. `ct_hex`
/// is stashed in a hidden input so the submit handler can read it.
pub(crate) fn adopt_join(ct_hex: &str) -> Markup {
    html! {
        (site_header(&Host::Apex))
        main.apex-main {
            div.col-chat {
                section.step {
                    div.pair-instructions { "adopt your identity on this device" }
                    form.create-form data-action="adopt-device" {
                        input #adopt-code .create-input type="text"
                            aria-label="one-time adoption code"
                            placeholder="enter code" autocomplete="off"
                            spellcheck="false" maxlength="8" required {}
                        input #adopt-ct type="hidden" value=(ct_hex) {}
                        button type="submit" .create-button { "adopt" }
                    }
                    div #adopt-msg .step-msg {}
                }
            }
        }
    }
}

/// Trap-fix interstitial. Swapped into `#agents-list` when a device with
/// NO wallet tries to claim a name: rather than silently minting a second
/// identity (the bug that split a user's subdomains across two EOAs), it
/// forces an explicit choice — create a genuinely new identity, or adopt
/// an existing one (import seed here, or scan "add a device" elsewhere).
pub(crate) fn identity_choice(name: &str) -> Markup {
    html! {
        div #agents-list .agents-list {
            div.pair-instructions { "no identity on this device yet" }
            div.pair-slot {
                button type="button" data-action="create-new-claim" data-arg=(name) .ghost {
                    "create a new identity"
                }
            }
            div.pair-slot {
                button type="button" data-action="show-import" .ghost {
                    "i already have one — import seed"
                }
            }
            div #import-slot {}
            div #seed-msg .admin-msg-slot {}
            div.pair-waiting { "or open “add a device” on a device you already use" }
        }
    }
}

/// Collapsed `[security]` section — the entry point the user has to
/// click before seed phrase / import / reset show up. Buries the
/// dangerous affordances one menu deeper so they don't sit in plain
/// view inside the admin dropdown.
pub(crate) fn admin_security_collapsed() -> Markup {
    html! {
        div #security-slot .admin-section {
            div.admin-section-title { "security" }
            button type="button" data-action="reveal-security" .ghost {
                "seed phrase, import, reset"
            }
        }
    }
}

/// Expanded `[security]` section — swapped into `#security-slot`
/// when the user clicks the collapsed entry point. Contains the
/// seed-reveal slot (driven by `Action::RevealSeed`), the import
/// form, and the reset button. A `[hide]` button at the bottom
/// flips back to the collapsed view.
pub(crate) fn admin_security_expanded() -> Markup {
    html! {
        div #security-slot .admin-section {
            div.admin-section-title { "security" }
            div.admin-subsection {
                div.admin-subsection-title { "seed phrase" }
                div #seed-reveal .seed-reveal {
                    button type="button" data-action="reveal-seed" .ghost { "reveal" }
                }
            }
            div.admin-subsection {
                div.admin-subsection-title { "import a different seed" }
                (import_seed_inline())
            }
            div.admin-subsection {
                div.admin-subsection-title { "reset this device" }
                div #reset-confirm-slot {
                    button type="button" data-action="reset-arm" .ghost { "reset…" }
                }
            }
            button type="button" data-action="hide-security" .ghost { "hide" }
        }
    }
}

/// Confirm-state for the reset button. Swapped into
/// `#reset-confirm-slot` when the user clicks `reset…` — they then
/// pick `confirm` (runs the wipe) or `cancel` (swaps back to the
/// armed button). Pure HTML; no JS dialog.
pub(crate) fn reset_confirm_inline() -> Markup {
    html! {
        div #reset-confirm-slot .reset-confirm {
            span.reset-confirm-prompt { "type RESET to clear this device — identity + names are kept" }
            input #reset-confirm-text .redeem-input type="text" aria-label="type RESET to confirm" placeholder="RESET";
            div.reset-confirm-actions {
                button type="button" data-action="reset-confirm" .danger { "reset" }
                button type="button" data-action="reset-cancel" .ghost { "cancel" }
            }
            div #reset-confirm-msg .admin-msg-slot {}
        }
    }
}

/// Armed-state reset button (the default before the user clicks).
/// Used to restore `#reset-confirm-slot` after a cancel.
pub(crate) fn reset_armed_inline() -> Markup {
    html! {
        div #reset-confirm-slot {
            button type="button" data-action="reset-arm" .ghost { "reset…" }
        }
    }
}

/// Armed-state for the OPFS panel's wipe button (default).
pub(crate) fn opfs_wipe_armed_inline() -> Markup {
    html! {
        span #opfs-wipe-slot {
            button data-action="opfs-wipe" { "wipe" }
        }
    }
}

/// Confirm-state for the OPFS panel's wipe button (after arm).
pub(crate) fn opfs_wipe_confirm_inline() -> Markup {
    html! {
        span #opfs-wipe-slot .opfs-wipe-confirm {
            button data-action="opfs-wipe-confirm" .danger { "wipe?" }
            button data-action="opfs-wipe-cancel" .ghost { "no" }
        }
    }
}

/// Full pricing card — currently unused (pricing UI removed from
/// the agent card in 0.10.15). Comes back when the visitor-pays UX
/// gets a clearer surface; kept compiled so call sites are warm.
#[allow(dead_code)]
pub(crate) fn pricing_card(price_wei: u128) -> Markup {
    html! {
        section .pricing-card {
            div.pricing-header {
                div.pricing-title { "pricing" }
            }
            (pricing_card_body(price_wei, true))
        }
    }
}

/// Single-line read-only pricing display for visitors (non-owners).
#[allow(dead_code)] // pricing UI hidden from agent card in 0.10.15
pub(crate) fn pricing_readonly_line(price_wei: u128) -> Markup {
    let display = if price_wei == 0 {
        "free".to_string()
    } else {
        format!("{} $LH/turn", super::format_wei_as_test_eth(price_wei))
    };
    html! {
        div.financial-line {
            span.financial-label { "pricing" }
            span.financial-value { (display) }
        }
    }
}

/// Right-column financial card. Injected by `kick_verification` once
/// the agent's TBA + balance + owner are known. Just the addresses
/// and balance for now — pricing UI removed per "i have NO idea what
/// the PRICING window does on the AGENT thing". The pricing data +
/// payment loop are still wired (`.lh_pricing.json` + chat send),
/// just not surfaced in the chrome until we have a clearer UX.
pub(crate) fn financial_card(
    name: &str,
    tba_hex: &str,
    owner_hex: &str,
    lh_balance_wei: u128,
    _price_wei: u128,
    _is_owner: bool,
) -> Markup {
    let tba_url = format!("https://moderato.tempo.xyz/address/{tba_hex}");
    let owner_url = format!("https://moderato.tempo.xyz/address/{owner_hex}");
    let balance_display = super::format_wei_as_test_eth(lh_balance_wei);
    let rpc_url = format!("https://{name}.localharness.xyz/?rpc=1");
    let tool_count = BuiltinTool::ALL.len();
    html! {
        section #financial-slot .financial-card {
            div.financial-line {
                span.financial-label { "name" }
                span.financial-value { (name) }
            }
            div.financial-line {
                span.financial-label { "owner" }
                a.financial-tba href=(owner_url) target="_blank" rel="noopener"
                    title=(owner_hex) {
                    (short_addr(owner_hex))
                }
            }
            div.financial-line {
                span.financial-label { "wallet" }
                a.financial-tba href=(tba_url) target="_blank" rel="noopener"
                    title=(tba_hex) {
                    (short_addr(tba_hex))
                }
            }
            div.financial-line {
                span.financial-label { "balance" }
                span.financial-value.financial-balance { (balance_display) }
            }
            div.financial-line {
                span.financial-label { "tools" }
                span #tools-count .financial-value { (tool_count) }
            }
            div.financial-line {
                span.financial-label { "rpc" }
                a.financial-tba href=(rpc_url) target="_blank" rel="noopener"
                    title="inter-agent RPC endpoint" {
                    "?rpc=1"
                }
            }
        }
    }
}

/// Pricing card body — owner-only edit form. Kept as a separate
/// template so `Action::PricingSave` can swap-outer just the body
/// after a successful save without re-rendering the slot.
pub(crate) fn pricing_card_body(price_wei: u128, is_owner: bool) -> Markup {
    let display = if price_wei == 0 {
        "free".to_string()
    } else {
        format!("{} $localharness/turn", super::format_wei_as_test_eth(price_wei))
    };
    html! {
        div #pricing-body .pricing-body {
            div.pricing-value { (display) }
            @if is_owner {
                div.pricing-edit {
                    input #pricing-input
                        type="text"
                        inputmode="decimal"
                        aria-label="price per turn in $localharness"
                        placeholder="1.0"
                        value=(if price_wei == 0 { String::new() } else { super::format_wei_as_test_eth(price_wei) }) {}
                    span.pricing-unit { "$localharness/turn" }
                    button.ghost
                        type="button"
                        data-action="pricing-save" { "save" }
                }
                div #pricing-msg .pricing-msg {}
            }
        }
    }
}

/// Inline import-seed form. Used in two places: swapped into
/// `#import-slot` on the no-identity step (when a fresh visitor
/// clicks "import seed"), and inside the header admin dropdown
/// (when an existing identity wants to swap to a different one).
pub(crate) fn import_seed_inline() -> Markup {
    html! {
        div #import-slot .seed-import {
            textarea #import-seed
                aria-label="12-word recovery phrase"
                placeholder="paste 12 words separated by spaces"
                rows="3" {}
            div.seed-import-actions {
                button type="button" data-action="import-seed" { "import" }
                button type="button" data-action="cancel-import" .ghost { "cancel" }
            }
            div #seed-msg .step-msg {}
        }
    }
}

/// Render the "your agents" table on apex. `agents` is what the
/// registry's `list_owned_tokens(wallet_address)` returned.
pub(crate) fn agents_list(
    agents: &[crate::app::registry::OwnedToken],
    main_token_id: u64,
) -> Markup {
    if agents.is_empty() {
        return html! {
            div #agents-list .agents-list .agents-empty {}
        };
    }
    // Bare list: subdomain name as a link, a small `main` chip on the
    // MAIN row, plus an [act] toggle that expands the inline act-panel
    // (per-agent send-LH form, runs via the agent's TBA.execute).
    html! {
        div #agents-list .agents-list {
            ul.agents-rows {
                @for agent in agents {
                    li.agent-row {
                        // Whole row is one clickable link — not just the name
                        // text. The horizontal line (name + spacer + badge) is
                        // the hit target.
                        a.agent-row-line
                            href=(format!("https://{}.localharness.xyz/", agent.name)) {
                            span.agent-name { (agent.name) }
                            span.agent-row-spacer {}
                            // Per on-chain feedback: no per-row "act" button
                            // on the apex homepage — just a main/alt label.
                            @if main_token_id != 0 && agent.token_id == main_token_id {
                                span.main-badge title="primary identity" { "main" }
                            } @else {
                                span.alt-badge title="secondary identity" { "alt" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Inline panel that opens under an agent row when [act] is clicked.
/// Shows the agent's TBA balance + a "send LH" form. Submit hits
/// `tba_transfer_lh_sponsored`. Hidden by default; populated on
/// first toggle and re-painted after each action.
pub(crate) fn agent_act_panel(
    token_id: u64,
    tba_address: &str,
    lh_balance_wei: u128,
) -> Markup {
    let lh_whole = lh_balance_wei / 1_000_000_000_000_000_000u128;
    html! {
        div.agent-act-rows {
            div.agent-act-row {
                span.agent-act-label { "wallet" }
                a.agent-act-value
                    href=(format!("https://moderato.tempo.xyz/address/{tba_address}"))
                    target="_blank" rel="noopener"
                    title=(tba_address) {
                    (short_addr(tba_address))
                }
            }
            div.agent-act-row {
                span.agent-act-label { "balance" }
                code.agent-act-value { (lh_whole) " LH" }
            }
        }
        form.agent-act-form
            data-action="agent-send-lh"
            data-arg=(token_id) {
            input
                id=(format!("agent-send-to-{token_id}"))
                type="text"
                aria-label="recipient address"
                placeholder="recipient 0x…"
                autocomplete="off"
                spellcheck="false"
                maxlength="42" {}
            input
                id=(format!("agent-send-amt-{token_id}"))
                type="text"
                aria-label="amount in LH"
                placeholder="amount LH"
                autocomplete="off"
                spellcheck="false"
                inputmode="decimal" {}
            button type="submit" .ghost { "send" }
        }
        div
            id=(format!("agent-act-msg-{token_id}"))
            .admin-msg-slot {}
    }
}

/// The hidden seed-phrase view — swapped into `#seed-reveal` when the
/// user confirms they're ready to write it down.
pub(crate) fn seed_phrase(words: &str) -> Markup {
    html! {
        div.seed-words { (words) }
        p.apex-fine {
            "12 words above. close this page or click "
            button type="button" data-action="hide-seed" .link-button { "hide" }
            " when you're done."
        }
    }
}

/// Chrome shown when the signer iframe loads but no identity exists
/// at the apex origin. The postMessage handler errors on every
/// challenge in this state — owner verification on the parent
/// subdomain will surface as "verify failed · no identity".
pub(crate) fn signer_no_identity() -> Markup {
    html! {
        main.apex-main {
            div.col-chat {
                section.apex-hero {
                    h2.apex-headline { "localharness signer" }
                    p.apex-sub {
                        "no identity exists on this device yet, so this signer "
                        "tab can't sign anything. "
                        a href="https://localharness.xyz/" { "go to apex" }
                        " to create or import one."
                    }
                }
            }
        }
    }
}

/// Minimal chrome for `?signer=1` — when apex is iframed from a
/// subdomain for owner verification. Shows just enough so the
/// developer console isn't a blank page, but nothing functional.
pub(crate) fn signer_chrome(address_hex: &str) -> Markup {
    html! {
        main.apex-main {
            div.col-chat {
                section.apex-hero {
                    h2.apex-headline { "localharness signer" }
                    p.apex-sub {
                        "this tab is acting as a signing service for an embedded "
                        "subdomain. it will sign authentication challenges from "
                        "any *.localharness.xyz origin using the master wallet:"
                    }
                    div.wallet-address-row {
                        span.wallet-label { "address" }
                        code .wallet-address { (address_hex) }
                    }
                    p.apex-fine {
                        "if you opened this manually rather than via an iframe, "
                        a href="https://localharness.xyz/" { "go home" }
                        "."
                    }
                }
            }
        }
    }
}

/// Tenant subdomain that no one on this device has claimed yet —
/// "unclaimed mode". Claims happen inline: the button ensures an apex
/// identity exists (creating one only if absent) and registers the name
/// on-chain via the signer iframe. The first subdomain a fresh visitor
/// claims becomes their primary identity; subsequent claims on other
/// names reuse the same wallet across the family of subdomains.
pub(crate) fn unclaimed(host: &Host, name: &str) -> Markup {
    html! {
        (site_header(host))
        main.apex-main {
            div.col-chat {
                section.step.step-unclaimed {
                    h2.unclaimed-name { (name) ".localharness.xyz" }
                    p.step-msg {
                        "this name is open. claim it to make it the home of an agent you own."
                    }
                    button type="button" data-action="claim-on-chain" .button-link {
                        "claim " (name)
                    }
                    div #claim-msg .step-msg {}
                }
            }
        }
    }
}

// --- OPFS panel templates --------------------------------------------

pub(crate) fn opfs_breadcrumb(cwd: &[String]) -> Markup {
    // `href="#"` makes each crumb a real link — keyboard-focusable and
    // Enter-activatable. The delegated click listener calls preventDefault,
    // so the `#` never actually navigates; it only exists to put the anchor
    // in the focus order and fire a native click on Enter.
    html! {
        a href="#" data-action="opfs-nav" data-arg="" aria-label="root directory" { "/" }
        @for i in 0..cwd.len() {
            @let arg = cwd[..=i].join("/");
            a href="#" data-action="opfs-nav" data-arg=(arg) { (cwd[i]) "/" }
        }
    }
}

pub(crate) fn opfs_list(cwd: &[String], entries: &[DirEntry]) -> Markup {
    html! {
        @if entries.is_empty() {
            li.empty { "(empty)" }
        } @else {
            @for entry in entries {
                @match entry.kind {
                    EntryKind::Directory => {
                        @let arg = if cwd.is_empty() {
                            entry.name.clone()
                        } else {
                            format!("{}/{}", cwd.join("/"), entry.name)
                        };
                        // `role=button` + `tabindex=0` put the directory row in
                        // the focus order and announce it as a button (a bare
                        // `<li>` is neither focusable nor activatable). Kept an
                        // `<li>` rather than a `<button>` so the `.fs-list li`
                        // flex layout + CSS (owned elsewhere) is preserved.
                        li.dir data-action="opfs-nav" data-arg=(arg)
                            role="button" tabindex="0"
                            aria-label=(format!("open folder {}", entry.name)) {
                            span.name { (entry.name) }
                        }
                    }
                    _ => {
                        @let lname = entry.name.to_ascii_lowercase();
                        @let opens_display = lname.ends_with(".html")
                            || lname.ends_with(".htm")
                            || lname.ends_with(".rl");
                        li.file {
                            // The filename opens the file in DISPLAY on click.
                            // `role=button` + `tabindex=0` make the clickable
                            // `<span>` focusable + announced (kept a span so the
                            // `.fs-list li .name` ellipsis/flex CSS still applies).
                            span.name data-action="opfs-open" data-arg=(entry.name)
                                role="button" tabindex="0"
                                aria-label=(format!("open {}", entry.name)) {
                                (entry.name)
                            }
                            @if let Some(size) = entry.size {
                                span.size { (format_bytes(size)) }
                            }
                            // .html/.rl open in DISPLAY on click; this keeps
                            // the source reachable for editing.
                            @if opens_display {
                                button.file-edit
                                    type="button"
                                    data-action="opfs-edit"
                                    data-arg=(entry.name)
                                    title=(format!("edit {}", entry.name)) { "edit" }
                            }
                            // Icon-only (`×`): give it an accessible name so a
                            // screen reader announces more than "button".
                            button.file-delete
                                type="button"
                                data-action="opfs-delete"
                                data-arg=(entry.name)
                                aria-label=(format!("delete {}", entry.name))
                                title=(format!("delete {}", entry.name)) { "×" }
                        }
                    }
                }
            }
        }
    }
}

pub(crate) fn opfs_error(message: &str) -> Markup {
    html! {
        li.empty { "error: " (message) }
    }
}

/// The textarea has id `fs-editor` so the save
/// handler can read its value; the buttons carry the file `name` as a
/// data-arg so a single delegated dispatcher works.
pub(crate) fn opfs_editor(display_path: &str, name: &str, text: &str) -> Markup {
    html! {
        div.editor {
            div.editor-header {
                span.editor-path { (display_path) }
                div.editor-actions {
                    button.panel-button
                        type="button"
                        data-action="opfs-save"
                        data-arg=(name) { "save" }
                    button.panel-button
                        type="button"
                        data-action="opfs-close-viewer" { "close" }
                }
            }
            textarea #fs-editor .editor-textarea aria-label=(format!("editing {name}")) { (text) }
        }
    }
}

/// DISPLAY surface — the framebuffer the cartridge loader blits into.
/// Just a single `<canvas>` in a letterboxed stage; no toolbar. The
/// canvas backing store is sized in `display::mount_canvas` and CSS
/// letterboxes it 16:9. Toggling the DISPLAY rail closed tears the
/// surface down (and stops any running cartridge). This is the "screen"
/// half of the Orbital-style compositor.
pub(crate) fn display_surface() -> Markup {
    html! {
        div.display-wrap {
            div.display-stage {
                canvas #display-canvas .display-canvas {}
            }
        }
    }
}

/// Chrome-less "app mode" page — the subdomain booted straight into its
/// cartridge (an `app.rl` exists in OPFS). Just the framebuffer canvas
/// filling the viewport, plus a tiny owner escape hatch back to the
/// workshop (`?edit=1`). No tabs/terminal/files — the cartridge IS the
/// page. See [[project-ai-os-vision]].
/// The **default public face** — shown to visitors of a subdomain that
/// hasn't published a cartridge yet. A profile/directory landing: the
/// agent's name, its owner (the MAIN name when it has one), its on-chain
/// wallet (TBA), and a directory of the owner's other agents. This is the
/// "anything" surface's sensible default; an owner replaces it by
/// shipping an `app.rl` / publishing a cartridge.
///
/// `is_main` badges the hero when this subdomain IS the owner's primary
/// identity. `owner_overlay` paints the `[studio]` escape (owner preview
/// only). `siblings` should already exclude this subdomain. `personas` is
/// aligned 1:1 with `siblings` (a short on-chain persona preview per agent,
/// `None` when unset) — each sibling renders as a discoverable portfolio
/// card: name + a truncated persona blurb, degrading to name-only.
pub(crate) fn public_landing(
    name: &str,
    owner: Option<&str>,
    tba: Option<&str>,
    main_name: Option<&str>,
    is_main: bool,
    siblings: &[crate::app::registry::OwnedToken],
    personas: &[Option<String>],
    owner_overlay: bool,
) -> Markup {
    html! {
        div.public-face {
            @if owner_overlay {
                a.app-edit href="?edit=1" title="back to your studio" { "studio" }
            }
            header.public-hero {
                h1.public-title { (name) }
                p.public-tagline {
                    "agent on localharness"
                    @if is_main { " · " span.main-badge title="primary identity" { "main" } }
                }
            }
            div.public-meta {
                @if let Some(addr) = owner {
                    div.public-meta-row {
                        span.public-meta-label { "owner" }
                        @if let Some(m) = main_name {
                            a.public-meta-value
                                href=(format!("https://{m}.localharness.xyz/"))
                                title=(addr) { (m) }
                        } @else {
                            a.public-meta-value
                                href=(format!("https://moderato.tempo.xyz/address/{addr}"))
                                target="_blank" rel="noopener" title=(addr) { (short_addr(addr)) }
                        }
                    }
                }
                @if let Some(t) = tba {
                    div.public-meta-row {
                        span.public-meta-label { "wallet" }
                        a.public-meta-value
                            href=(format!("https://moderato.tempo.xyz/address/{t}"))
                            target="_blank" rel="noopener" title=(t) { (short_addr(t)) }
                    }
                }
            }
            @if !siblings.is_empty() {
                section.public-directory {
                    h2.public-section-title { "more agents by this owner" }
                    ul.agents-rows {
                        @for (i, s) in siblings.iter().enumerate() {
                            @let preview = personas.get(i).and_then(|p| p.as_deref());
                            li.agent-row {
                                a.agent-card
                                    href=(format!("https://{}.localharness.xyz/", s.name)) {
                                    span.agent-name { (s.name) }
                                    @if let Some(p) = preview {
                                        span.agent-preview { (truncate_preview(p, 80)) }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            footer.public-footer {
                a href="https://localharness.xyz/" title="localharness" { "localharness" }
            }
        }
    }
}

/// The fullscreen public-face surface (a cartridge running in a canvas).
/// `owner_overlay` controls whether the `[studio]` escape link is painted
/// — shown only when the *owner* is previewing their own public face, so a
/// visitor never sees an edit door they can't use.
pub(crate) fn app_fullscreen(owner_overlay: bool) -> Markup {
    html! {
        div.app-fullscreen {
            div.app-stage {
                canvas #display-canvas .display-canvas {}
            }
            @if owner_overlay {
                a.app-edit href="?edit=1" title="back to your studio" { "studio" }
            }
        }
    }
}

fn format_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Format a key-meta hint shown next to the key input.
pub(crate) fn keymeta(key: &str) -> Markup {
    let n = key.len();
    if n == 0 {
        return html! {};
    }
    let looks_right = (30..=60).contains(&n)
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let suffix = if looks_right { "" } else { " - check" };
    html! {
        span style=(if looks_right { "" } else { "color: var(--error)" }) {
            "(" (n) " chars" (suffix) ")"
        }
    }
}
