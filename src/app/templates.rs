//! All HTML in the browser app is produced here, via [`maud`]
//! compile-time templates. Templates return `Markup`; callers turn
//! them into strings and ship them into the DOM via the helpers in
//! [`super::dom`]. **No template function takes a DOM handle** — they
//! are pure `inputs → HTML` functions, so they're trivial to read,
//! test, and recompose.

use maud::{html, Markup, PreEscaped};

use crate::encoding::short_addr;
use crate::filesystem::{DirEntry, EntryKind};
use crate::types::{BuiltinTool, ToolCall, ToolResult};

use super::tenant::Host;
use super::VerifyState;

/// True on iOS / iPadOS WebKit — the platform localharness can't yet support,
/// because Safari's OPFS write (`createWritable`/`close`) stalls and hangs the
/// single-threaded wasm app mid-onboarding. Used to gate the apex create CTA
/// ([`crate::landing::ios_unavailable`]). UA covers iPhone/iPad/iPod; iPadOS 13+
/// reports as "Macintosh" but has a touchscreen, so include Mac-UA-with-touch.
fn is_ios() -> bool {
    let Some(nav) = web_sys::window().map(|w| w.navigator()) else {
        return false;
    };
    let ua = nav.user_agent().unwrap_or_default();
    if ua.contains("iPhone") || ua.contains("iPad") || ua.contains("iPod") {
        return true;
    }
    ua.contains("Macintosh") && nav.max_touch_points() > 0
}

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

/// The default buy-`$LH` control (amount input + button) shown inside `#buy-area`
/// in the admin credits section. Clicking "buy $LH" swaps `#buy-area` to
/// [`buy_inline_form`] (the inline Stripe card form) IN PLACE — no popup modal.
/// `cancel` restores this. Whole-dollar amount, $2 minimum.
pub(crate) fn buy_area_default() -> Markup {
    html! {
        div.redeem-row {
            input #buy-usd .redeem-input type="number"
                min="2" step="1" value="2" inputmode="numeric" aria-label="amount in USD";
            button type="button" data-action="buy-lh" .ghost { "buy $LH" }
        }
    }
}

/// The INLINE buy-`$LH` checkout — swapped into `#buy-area` (NOT a popup overlay)
/// when the user clicks "buy $LH", mirroring the apex onboarding inline card.
/// Carries the Stripe mount ids the shim needs (`#lh-payment`, `#lh-pay-button`,
/// `#lh-pay-error`, `#buy-modal-done`). On success the shim flips to
/// `#buy-modal-done` (`lhBuySuccess`) and the webhook mints. `cancel` (cancel-buy)
/// restores [`buy_area_default`]. `lh_label` previews the `$LH`.
pub(crate) fn buy_inline_form(lh_label: &str) -> Markup {
    html! {
        div.api-key-hint style="margin:2px 0 8px" { (lh_label) }
        div #buy-checkout-msg .step-msg style="color:var(--muted)" {
            "preparing secure checkout…"
        }
        div #lh-pay-region {
            div #lh-payment style="margin:6px 0" {}
            button #lh-pay-button type="button" .apex-onboard-cta style="display:none;width:100%;margin:8px 0 0" { "pay" }
            div #lh-pay-error role="alert" aria-live="assertive" style="color:#ff6b6b;font-size:12px;min-height:1em;margin:4px 0" {}
        }
        div #buy-modal-done .api-key-hint style="display:none" {
            "✓ payment received — your $LH is being credited and will appear shortly."
        }
        button #buy-cancel type="button" data-action="cancel-buy" .ghost style="margin-top:6px" { "cancel" }
    }
}

/// INLINE onboarding checkout — the pay-first "create agent · $2" flow renders
/// this IN PLACE of `#apex-onboard` (NOT a modal/overlay), so the card appears
/// on the page the instant the button is tapped. Carries the SAME Stripe mount
/// ids the shim needs (`#lh-payment`, `#lh-pay-error`,
/// `#buy-modal-done`) plus an interstitial line (`#onboard-checkout-msg`) the
/// Rust side clears once `lhBuyLh` mounts the form. Same visual family as the
/// apex onboard card; minimal copy.
pub(crate) fn onboard_checkout() -> Markup {
    html! {
        section #apex-onboard .apex-onboard {
            // Keep the offer pitch at the top so "limited time" / the deal does
            // NOT vanish when the user taps create (same pitch as the button card).
            (crate::landing::onboard_pitch())
            div #onboard-checkout-msg .step-msg style="color:var(--muted)" {
                "preparing secure checkout…"
            }
            div #lh-pay-region {
                div #lh-payment style="margin:6px 0" {}
                button #lh-pay-button type="button" .apex-onboard-cta style="display:none;width:100%;margin:8px 0 0" { "pay" }
                div #lh-pay-error role="alert" aria-live="assertive" style="color:#ff6b6b;font-size:12px;min-height:1em;margin:4px 0" {}
            }
            div #buy-modal-done .api-key-hint style="display:none" {
                "✓ payment received — your $LH is minting on-chain and will appear shortly."
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

/// Sticky header — brand (left) + notification bell + settings gear (right).
/// The settings button (`crate::landing::settings_glyph`, the same stroke
/// style as the bell) opens the admin dropdown; both are square icon buttons
/// that opt out of the `.header-button` 96px text min-width via `.admin-button`
/// / `.notif-bell-btn`.
pub(crate) fn site_header(_host: &Host) -> Markup {
    html! {
        header.site-header {
            div.header-inner {
                h1.header-brand {
                    details.brand-menu {
                        summary.brand-summary { "localharness" }
                        nav.brand-menu-items {
                            // Absolute apex URL, NOT "/": a relative home on a
                            // subdomain (esp. an installed PWA scoped to that
                            // subdomain) trapped the user there. Home is the
                            // apex — the user's subdomain list + create (krafto #220).
                            a href="https://localharness.xyz/" { "home" }
                            a href="https://github.com/compusophy/localharness"
                                target="_blank" rel="noopener" { "repo" }
                            a href="https://crates.io/crates/localharness"
                                target="_blank" rel="noopener" { "crate" }
                        }
                    }
                }
                // Header carries ONLY brand + admin (feedback #71: the
                // files + bug buttons cluttered the chrome). Files opens
                // from the admin panel; feedback stays an admin tab.
                div #header-admin .header-admin {
                    (notif_bell())
                    button type="button"
                        data-action="header-admin-toggle"
                        aria-label="settings" title="settings"
                        .header-button.admin-button { (crate::landing::settings_glyph()) }
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
            // (The context-fullness bar `#ctx-bar` moved to the TOP of the
            // chat area — see `chrome` — per feedback #62.)
            // Funding affordance — empty by default; `events::refresh_fund_banner`
            // fills it with a redeem CTA when the credit identity holds zero `$LH`
            // (so a new user with no funds sees the path to redeem instead of a
            // silent proxy rejection on their first send). Hidden again once funded.
            // role=status announces the "no $LH — redeem" CTA when it appears, so a
            // screen-reader user isn't left to hit a silent rejection on first send.
            div #fund-banner .fund-banner role="status" aria-live="polite" {}
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
    // Inline SVG triangle, NOT the "▶" text glyph — IBM Plex Mono has no
    // geometric shapes, so the fallback font drew a misshapen blob. Same
    // square-icon-button treatment as the header bell; centered by CSS.
    let play = maud::PreEscaped(
        "<svg viewBox=\"0 0 16 16\" width=\"12\" height=\"12\" fill=\"currentColor\" \
         aria-hidden=\"true\"><path d=\"M4.5 2.5v11l9-5.5z\"/></svg>",
    );
    html! {
        button #terminal-send .terminal-send data-action="send" title="send" aria-label="send" { (play) }
    }
}

/// The stop slot shown in place of the send button while a turn is in
/// flight: the stop button (`■`, cooperative cancel) plus — on a tenant,
/// where the run can be promoted to an on-chain goal job — a small
/// [⇪ background] button that continues the work HEADLESS via the
/// scheduler worker even after the tab closes. The group carries the
/// `terminal-stop` id so the existing swap lifecycle (`chat::run_send` /
/// `TurnGuard` restoring [`send_button`] by id) removes BOTH buttons in
/// one `swap_outer` when the run ends.
pub(crate) fn stop_button() -> Markup {
    html! {
        span #terminal-stop style="display:flex;align-items:center;flex-shrink:0" {
            button .terminal-send.terminal-stop data-action="stop-turn" title="stop" aria-label="stop generating" {
                (maud::PreEscaped("<svg viewBox=\"0 0 16 16\" width=\"11\" height=\"11\" fill=\"currentColor\" aria-hidden=\"true\"><rect x=\"3\" y=\"3\" width=\"10\" height=\"10\"/></svg>"))
            }
        }
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

/// The friendly out-of-credits card rendered in the transcript when a turn
/// 402s for lack of `$LH`/session — replaces dumping the raw JSON 402 error at
/// the user (on-chain feedback: "better handling than showing this"). Inline-
/// styled, monochrome, reusing already-wired data-actions: buy `$LH`, open the
/// account panel (redeem a code / open a session), or switch to your own key.
/// No raw error text, no rule prose.
pub(crate) fn out_of_credits_card() -> Markup {
    html! {
        div style="display:flex;flex-wrap:wrap;align-items:center;gap:8px;\
                    padding:8px 10px;\
                    border:1px solid var(--border);background:var(--panel);\
                    font-size:12px;color:var(--muted)" {
            span style="flex-basis:100%" { "out of $LH for this origin — top up to keep chatting" }
            button type="button" data-action="buy-lh" .ghost { "buy $LH" }
            button type="button" data-action="header-admin-toggle" .ghost { "redeem / open session" }
            button type="button" data-action="set-model-access" data-arg="byok" .ghost { "use my own key" }
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

/// The full app chrome — UNIFIED STREAM (GitHub #28): chat IS the app.
/// One chronological transcript takes the whole content area on every
/// viewport; files and display surface INLINE (the `inline_result_card`s)
/// and on demand via header-[files] → [`files_modal`] and ToggleDisplay →
/// [`display_overlay`]. No mobile tab bar, no side panels. The two
/// `hidden` divs are the swap targets the modal/overlay open into
/// (admin-modal pattern: `swap_outer` by fixed id).
pub(crate) fn chrome(host: &Host) -> Markup {
    html! {
        (site_header(host))
        main #layout .layout {
            div.col-chat {
                // (Removed the top context/token bar entirely per repeated user
                // feedback (krafto) — it read as clutter; the chat workspace is
                // maximized. Auto-compaction still runs silently.)
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
            }
        }
        div #files-modal hidden {}
        div #display-overlay hidden {}
    }
}

/// The OPFS file browser as a modal overlay (header [files] /
/// `Action::ToggleFiles`) — same overlay machinery as the admin modal.
/// `opfs::refresh` paints into `#fs-breadcrumb` / `#fs-list`; the editor
/// (`opfs::edit_file`) swaps into `#fs-viewer` below the list. Closing
/// swaps the whole thing back to the `hidden` placeholder.
pub(crate) fn files_modal() -> Markup {
    html! {
        div #files-modal .files-modal {
            div.files-dialog {
                div.files-head {
                    span.files-title { "files" }
                    button type="button" data-action="toggle-files"
                        .modal-close aria-label="close files" { "×" }
                }
                div.files-body {
                    div #fs-breadcrumb .fs-breadcrumb { "/" }
                    ul #fs-list .fs-list {}
                    div #fs-viewer .fs-viewer {}
                }
            }
        }
    }
}

/// The closed state of the files modal — the hidden swap target.
pub(crate) fn files_modal_closed() -> Markup {
    html! { div #files-modal hidden {} }
}

/// The DISPLAY framebuffer as a fullscreen overlay (ToggleDisplay /
/// the inline display card's [show] / mounted by `display::mount_canvas`
/// when a cartridge or HTML render starts). Dismissable via `×`, which
/// also stops a running cartridge. The cartridge keeps running in its
/// Web Worker exactly as before — only the surface placement changed.
pub(crate) fn display_overlay() -> Markup {
    html! {
        div #display-overlay .display-overlay {
            button type="button" data-action="toggle-display"
                .modal-close.display-close aria-label="close display" { "×" }
            (display_surface())
        }
    }
}

/// The closed state of the display overlay — the hidden swap target.
pub(crate) fn display_overlay_closed() -> Markup {
    html! { div #display-overlay hidden {} }
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

/// Per-turn swap target for the turn-stage micro-pipeline (GitHub #19).
/// Rendered as the FIRST child of a pending assistant body; `chat::stage`
/// swaps [`stage_line`] fragments into it while the turn streams and
/// empties it when the turn completes (`.stage-line:empty` hides it).
pub(crate) fn stage_container(turn_id: u32) -> Markup {
    let id_str = format!("stage-{turn_id}");
    // `role=status` + aria-live=polite so a screen-reader user hears the turn
    // lifecycle (paying → thinking → streaming) instead of silence while the
    // agent works (GitHub #61/#65). polite (not assertive) so the frequent
    // stage swaps queue behind, rather than interrupt, the transcript output.
    html! {
        div id=(id_str) .stage-line role="status" aria-live="polite" {}
    }
}

/// The stage pipeline line itself: a SINGLE persistent monochrome indicator
/// (`.st-dot`, the one animated cue for every stage — no per-stage spinner)
/// followed by the lowercase stage words joined by `→`, the CURRENT one
/// emphasized (`st-now`, static weight/color), crossed ones muted (`st-past`),
/// re-walked ones dim (`st-dim`). Rendered even with no slots yet (the dot
/// alone) so the line occupies a stable height the moment the turn begins and
/// every state change is an in-place text/class swap — never an appear/vanish
/// reflow of the transcript below. Monochrome, terse, no prose.
pub(crate) fn stage_line(slots: &[(crate::turn_stage::Stage, crate::turn_stage::Slot)]) -> Markup {
    use crate::turn_stage::Slot;
    html! {
        span.st-dot aria-hidden="true" {}
        @for (i, (stage, slot)) in slots.iter().enumerate() {
            @if i > 0 { span.st-sep { " → " } }
            span class=(match slot {
                Slot::Past => "st-past",
                Slot::Current => "st-now",
                Slot::Idle => "st-dim",
            }) { (stage.word()) }
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
///
/// Followed by an empty card slot (`#tool-{id}-card`) that fills with an
/// [`inline_result_card`] when the result warrants one (file / directory /
/// display outputs), so the transcript shows what a tool produced inline,
/// chronologically, without tab-hopping. Empty for every other tool.
pub(crate) fn tool_call_block(seg_id: u32, call: &ToolCall) -> Markup {
    let block_id = format!("tool-{seg_id}");
    let result_id = format!("tool-{seg_id}-result");
    let card_id = format!("tool-{seg_id}-card");
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
        div id=(card_id) {}
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

// --- Inline result cards -------------------------------------------------
//
// Compact transcript cards under a tool pill for file / directory / display
// tool outputs (GitHub #28). With the unified stream these ARE the primary
// surface: a card is a chronological anchor whose [open]/[show] jumps into
// the files modal / display overlay.

/// Cap on lines shown inside an inline result card; the rest is summarized
/// by a "… +N more lines" trailer and reachable via [open] (files modal).
const CARD_MAX_LINES: usize = 40;

/// First [`CARD_MAX_LINES`] lines of `content` plus how many lines were cut.
fn card_snippet(content: &str) -> (String, usize) {
    let total = content.lines().count();
    if total <= CARD_MAX_LINES {
        (content.trim_end_matches('\n').to_string(), 0)
    } else {
        let shown = content
            .lines()
            .take(CARD_MAX_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        (shown, total - CARD_MAX_LINES)
    }
}

/// Normalize a tool-supplied path into an `opfs-open`/`opfs-nav` data-arg.
/// The panel actions resolve cwd-relative names; tool paths are
/// OPFS-root-relative, so strip any leading slash (the panel default cwd is
/// the root, where the two coincide).
fn opfs_arg(path: &str) -> &str {
    path.trim_start_matches('/')
}

/// Compact inline card for a SUCCESSFUL tool result, rendered into the
/// `#tool-{id}-card` slot under the tool pill — `None` for tools / results
/// that don't warrant one (then the slot stays empty). Shared by the live
/// stream (`chat::stream_turn`) and history replay (`history::paint_entries`)
/// so both paths paint identically. `display_thumb` is a data-URL snapshot of
/// the framebuffer, live-path-only — replay can't reproduce pixels and
/// passes `None` (marker card only).
pub(crate) fn inline_result_card(
    name: &str,
    args: &serde_json::Value,
    result: &ToolResult,
    display_thumb: Option<&str>,
) -> Option<Markup> {
    if result.error.is_some() {
        return None;
    }
    let value = result.result.as_ref()?;
    match name {
        "view_file" => {
            let content = value.get("content")?.as_str()?;
            let path = value
                .get("path")
                .and_then(|v| v.as_str())
                .or_else(|| args.get("path").and_then(|v| v.as_str()))?;
            Some(file_card(path, content))
        }
        // The result carries only `{ok, path, ...}`; the written content
        // lives in the call args (create: the whole file, edit: the
        // replacement text).
        "create_file" => {
            let path = args.get("path").and_then(|v| v.as_str())?;
            let content = args.get("content").and_then(|v| v.as_str())?;
            Some(file_card(path, content))
        }
        "edit_file" => {
            let path = args.get("path").and_then(|v| v.as_str())?;
            let content = args.get("new_string").and_then(|v| v.as_str())?;
            Some(file_card(path, content))
        }
        "list_directory" => {
            let entries = value.get("entries")?.as_array()?;
            let path = value.get("path").and_then(|v| v.as_str()).unwrap_or("");
            Some(dir_card(path, entries))
        }
        "run_cartridge" | "render_html" => {
            // These return Ok-with-`error` on compile/run failure and a
            // `status` field only on the browser success shape — gate on
            // both so a failed run never gets a "rendered" marker.
            if value.get("error").is_some() || value.get("status").is_none() {
                return None;
            }
            Some(display_card(display_thumb))
        }
        "embed_app" => {
            // The tool only emits `embedded: true` on success (else it errors,
            // which short-circuits above). The card carries a live
            // `#embed-canvas` that `chat::stream_turn` launches the stashed
            // cartridge into right after this swaps in. Replay (no stashed
            // bytes) paints the same canvas, which simply stays black.
            if value.get("embedded").and_then(|v| v.as_bool()) != Some(true) {
                return None;
            }
            let name = value.get("name").and_then(|v| v.as_str()).unwrap_or("app");
            Some(embed_app_card(name))
        }
        _ => None,
    }
}

/// Live inline card for an `embed_app` result: a header (the embedded
/// subdomain's name, linking out) over a 16:9 canvas the cartridge renders
/// into. The canvas id is UNIQUE per card (`display::next_embed_canvas_id`) —
/// live and replayed cards coexist in one transcript, and a shared id made
/// the launch resolve the OLDEST card's canvas (the blank-embed bug). The
/// backing store is sized by `display::run_in_canvas` (the cartridge's
/// declared dims); CSS scales the ELEMENT to the card box with
/// `image-rendering: pixelated`, like the fullscreen display. Pointer input
/// routes here while it's the active cartridge (see `events::mod`). v1: one
/// LIVE embed at a time (single worker).
fn embed_app_card(name: &str) -> Markup {
    html! {
        div.inline-card.embed-app-card {
            div.ic-head {
                span.ic-title { "▶ " (name) }
                a.ghost href=(format!("https://{name}.localharness.xyz/"))
                    target="_blank" rel="noopener" { "open" }
            }
            div.embed-app-stage {
                canvas id=(crate::app::display::next_embed_canvas_id()) .embed-app-canvas {}
            }
        }
    }
}

/// Filename header + capped monospace body + [open] into the files modal
/// (reuses the browser's own `opfs-open` action, which opens the modal).
fn file_card(path: &str, content: &str) -> Markup {
    let (shown, cut) = card_snippet(content);
    html! {
        div.inline-card {
            div.ic-head {
                span.ic-title { (path) }
                button.ghost data-action="opfs-open" data-arg=(opfs_arg(path)) { "open" }
            }
            pre.ic-body { (shown) }
            @if cut > 0 {
                div.ic-more { "… +" (cut) " more lines" }
            }
        }
    }
}

/// One-line-per-entry directory card. Directory rows navigate the files
/// modal (`opfs-nav`); file rows open via the same `opfs-open` its rows
/// use. `role=button` + `tabindex=0` match the panel's a11y convention
/// (the delegated keydown handler activates them on Enter/Space).
fn dir_card(path: &str, entries: &[serde_json::Value]) -> Markup {
    let base = opfs_arg(path).trim_end_matches('/');
    let base = if base == "." { "" } else { base };
    html! {
        div.inline-card {
            div.ic-head {
                span.ic-title { (if base.is_empty() { "/" } else { base }) }
                span.ic-meta { (entries.len()) " entries" }
            }
            div.ic-rows {
                @for entry in entries {
                    @let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    @let is_dir = entry.get("kind").and_then(|v| v.as_str()) == Some("directory");
                    @let arg = if base.is_empty() { name.to_string() } else { format!("{base}/{name}") };
                    @if is_dir {
                        div.ic-row role="button" tabindex="0"
                            data-action="opfs-nav" data-arg=(arg) {
                            (name) "/"
                        }
                    } @else {
                        div.ic-row role="button" tabindex="0"
                            data-action="opfs-open" data-arg=(arg) {
                            (name)
                        }
                    }
                }
                @if entries.is_empty() { div.ic-more { "(empty)" } }
            }
        }
    }
}

/// Marker card for a successful display render. The display overlay holds
/// the live surface; this anchors the event in the transcript with a [show]
/// jump (reuses `toggle-display`). `thumb` is a live-path framebuffer
/// snapshot — `None` on replay, where only the marker paints.
fn display_card(thumb: Option<&str>) -> Markup {
    html! {
        div.inline-card {
            div.ic-head {
                span.ic-title { "▶ rendered to display" }
                button.ghost data-action="toggle-display" { "show" }
            }
            @if let Some(url) = thumb {
                img.ic-thumb src=(url) alt="display framebuffer snapshot";
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
        // `apex-front` (fresh only) vertically centers the single create CTA;
        // the claim page keeps the default top-aligned block flow.
        main.apex-main.apex-front[fresh] {
            div.col-chat {
                // Dispatcher/status messages (invite auto-redeem lands here —
                // without this node `dom::set_status` is silently dropped on
                // the apex and $LH moves with zero acknowledgment).
                div #status .terminal-status role="status" aria-live="polite" {}
                // Identity gate: a FRESH visitor (no wallet, no credits) leads
                // with invite-code redemption — claiming a name before they're
                // funded stranded them. The claim-a-name form only appears once
                // an identity exists (after redeem / create / import).
                // Volatile-storage (incognito) warning lands here — empty by
                // default, filled async by `paint_apex` when the browser refuses
                // durable storage (kit-qa #). Above the onboarding so a fresh
                // visitor sees it before minting a key they could lose on close.
                div #storage-warn-slot {}
                @if fresh && is_ios() {
                    // iOS/iPadOS Safari hangs the wasm app on its first OPFS
                    // write (createWritable stalls the single-thread executor),
                    // so onboarding can't complete there. Gate it off honestly
                    // instead of shipping a flow that freezes mid-checkout.
                    (crate::landing::ios_unavailable())
                } @else if fresh {
                    // ONE front door: create a wallet (paid entry that creates
                    // AND funds it, so a 0-$LH visitor never exists). Invited
                    // users skip this — `?invite=` links auto-redeem on mount.
                    // Redeem + import live in admin; explore is post-auth only.
                    (crate::landing::create_wallet_cta())
                } @else {
                    (apex_claim())
                    (crate::landing::apex_links(fresh))
                }
            }
        }
        // The "for agents →" pointer sits in a page footer on the fresh front
        // door (the claim page keeps it inline above).
        @if fresh {
            footer.apex-footer { (crate::landing::apex_links(true)) }
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
                    autocapitalize="none"
                    autocorrect="off"
                    spellcheck="false"
                    maxlength="32"
                    required {}
                button #create-btn type="submit" .create-button disabled { "create" }
            }
            // Funding affordance slot — empty unless a claim hits `__NEED_LH__`
            // (registration costs `$LH` and this 0-balance apex wallet can't
            // cover it). Filled by `claim::run_apex_claim` with `buy_to_claim`.
            // Stays empty when registration is free (today), so nothing shows.
            div #claim-fund-slot {}
        }
    }
}

/// Pre-claim funding affordance — shown only when a first claim hits
/// `__NEED_LH__` (registration costs `$LH`, this fresh apex wallet has 0).
/// One click opens the SAME Stripe Elements buy modal as admin
/// (`buy-lh` → `credits::buy_lh_pressed`, fixed $2 with no `#buy-usd` input —
/// $2 because Stripe fees net only ~0.67 $LH on $1, below the 1 $LH cost),
/// minting `$LH` to the apex wallet so the user can re-click create.
pub(crate) fn buy_to_claim() -> Markup {
    html! {
        div style="display:flex;flex-wrap:wrap;align-items:center;gap:8px;\
                    margin-top:8px;font-size:12px;color:var(--muted)" {
            button type="button" data-action="buy-lh" .ghost { "buy $2 to claim" }
            div #fund-msg .admin-msg-slot style="margin-top:0;flex-basis:100%" {}
        }
    }
}



/// Volatile-storage warning (kit-qa #): the identity seed lives in OPFS, which
/// a private / incognito window can WIPE on tab close — so a newly-minted
/// subdomain key would be lost forever. Surfaced (non-blocking) when
/// `wallet_store::storage_is_volatile()` reports the browser refused durable
/// storage. The link banks the seed off-device via the QR `?adopt=1` flow.
/// `role="alert"` so a screen reader announces it; everything is maud-escaped.
pub(crate) fn volatile_storage_warning() -> Markup {
    html! {
        div .volatile-storage-warn role="alert" {
            "this looks like a private / incognito window — your identity key "
            "may NOT survive closing this tab. back it up via "
            a href="https://localharness.xyz/?adopt=1" target="_top" rel="noopener" {
                "add a device"
            }
            " or use a normal window."
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
                    // The tab arg/class stays "usage" (one CSS/dispatch
                    // surface for both panels); the label says what the
                    // panel actually holds — the $LH economy controls.
                    button #admin-tab-btn-usage type="button"
                        data-action="show-admin-tab" data-arg="usage"
                        .admin-tab-button { "economy" }
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
                    // Notifications: permission + Web Push subscription,
                    // published on-chain for the tab-closed scheduler pushes.
                    (admin_notify_section())
                    (admin_security_collapsed())
                }
                div.admin-footer {
                    span.admin-version { (APP_VERSION) }
                }
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
                // Volatile-storage (incognito) warning target — filled async by
                // the CreateIdentity handler when durable storage is refused.
                div #storage-warn-slot {}
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
            div.admin-section-title { "model credits" }
            // A label:value row like every other stat — the bare centered
            // number read as orphaned from its section title.
            div.admin-identity-row {
                span.admin-identity-label { "balance" }
                code #credits-balance .admin-identity-value { "…" }
            }
            div.redeem-row {
                input #redeem-code .redeem-input type="text" aria-label="redeem code" placeholder="redeem code";
                button type="button" data-action="redeem-code" .ghost { "redeem" }
            }
            // Buy $LH with a card. "buy $LH" swaps #buy-area to the INLINE Stripe
            // card form (buy_inline_form) IN PLACE — no popup modal (issue: the
            // user asked for this repeatedly). $2 min, whole dollars.
            div #buy-area {
                (buy_area_default())
            }
            div #buy-msg .admin-msg-slot {}
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
/// hash is on-chain), so it's surfaced as a scannable QR of the
/// ready-to-share `?invite=` link (white module background so a phone
/// camera reads it off a dark screen) with the link text + [copy] below.
/// Refundable to the funder via `invite reclaim` after expiry. Both
/// `code` + `link` are escaped by maud's `(…)`; the SVG is our own
/// generated bytes, safe under `PreEscaped`.
pub(crate) fn invite_result_panel(code: &str, link: &str) -> Markup {
    html! {
        div.invite-result-card {
            div.pair-instructions { "share this link with ONE person you trust:" }
            @if let Some(svg) = pair_qr_svg(link) {
                div.invite-qr { (PreEscaped(svg)) }
            }
            div.share-line {
                a.pair-url href=(link) target="_blank" rel="noopener" { (link) }
                button .ghost type="button"
                    data-action="copy-share-url" data-arg=(link) { "copy" }
            }
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

/// Notifications — [enable notifications] asks the browser for Notification
/// permission (this click IS the user gesture browsers require), subscribes
/// Web Push against the service worker, and publishes the subscription
/// on-chain (`keccak256("localharness.push_sub")`, MAIN tokenId) so the
/// proxy's scheduler worker can notify the owner when a scheduled job runs —
/// tab closed, app installed or not. Also unlocks the agent's `notify` tool
/// without a mid-turn permission prompt.
pub(crate) fn admin_notify_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "app" }
            div.pair-slot {
                button type="button" data-action="install-app" .ghost {
                    "install app"
                }
                button type="button" data-action="toggle-files" .ghost {
                    "files"
                }
            }
            div #install-msg .admin-msg-slot {}
            div.admin-section-title { "notifications" }
            div.pair-slot {
                button type="button" data-action="enable-notifications" .ghost {
                    "enable notifications"
                }
                button type="button" data-action="test-notification" .ghost {
                    "test"
                }
            }
            div #notify-msg .admin-msg-slot {}
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

/// Encode a share/pairing link as an inline SVG QR code (black modules
/// on white, monochrome, no `image`/font deps). Thin alias over the
/// native-testable core in `crate::qr` — the encode → SVG pipeline and
/// its unit test live THERE, this module is wasm32-only. `None` on the
/// (practically impossible) encode failure so panels still render the
/// typeable URL as a fallback.
fn pair_qr_svg(url: &str) -> Option<String> {
    crate::qr::qr_svg(url)
}

/// Post-publish share moment — swapped into `#publish-app-msg` after a
/// successful app/html publish so the owner immediately sees the
/// shareable URL: the live link, a [copy] button, and a QR (same inline
/// SVG pipeline as device linking) for handing the page to a phone.
pub(crate) fn publish_share_fragment(name: &str) -> Markup {
    let url = format!("https://{name}.localharness.xyz/");
    html! {
        div.share-block {
            div.share-line {
                span { "live at" }
                a href=(url) target="_blank" rel="noopener" { (url) }
                button #share-copy .ghost type="button"
                    data-action="copy-share-url" data-arg=(url) { "copy" }
            }
            @if let Some(svg) = pair_qr_svg(&url) {
                div.pair-qr { (PreEscaped(svg)) }
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
                            autocapitalize="none" autocorrect="off"
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
    // `data-modal-trap` confines Tab to this panel while armed; `data-modal-cancel`
    // routes Escape to the cancel action (which restores focus to the trigger).
    // role/aria-modal/aria-label give screen readers a labelled dialog (a11y #75).
    html! {
        div #reset-confirm-slot .reset-confirm role="dialog" aria-modal="true"
            aria-label="confirm device reset"
            data-modal-trap data-modal-cancel="reset-cancel" {
            span.reset-confirm-prompt { "type RESET to wipe this device — your identity (seed) is DELETED; back it up first" }
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

/// Confirm-state for the OPFS panel's wipe button (after arm). It lives
/// INSIDE the already-focus-trapped files modal, so it carries a labelled
/// `role="group"` (not a nested dialog) and the arm handler pulls focus onto
/// [wipe?] so a keyboard user lands on the action, not the trigger (a11y #75).
pub(crate) fn opfs_wipe_confirm_inline() -> Markup {
    html! {
        span #opfs-wipe-slot .opfs-wipe-confirm role="group" aria-label="confirm wipe all files" {
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
                // The agent TBA's own $LH (x402 earnings) — labelled to
                // distinguish it from the owner's model credits below.
                span.financial-label { "agent $LH" }
                span.financial-value.financial-balance { (balance_display) }
            }
            div.financial-line {
                span.financial-label { "tools" }
                span #tools-count .financial-value { (tool_count) }
            }
            // (No `?rpc=1` row — the inter-agent RPC endpoint is wire
            // plumbing agents discover via llms.txt, not a human control;
            // showing it here read as a broken link.)
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
    // Bare list: subdomain name as a link plus a small main/alt chip.
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

/// The hidden seed-phrase view — swapped into `#seed-reveal` when the
/// user confirms they're ready to write it down. `[copy]` is the mobile
/// lifeline: backgrounding the browser can refresh the tab and dismiss
/// this view, so one tap must be enough to bank the words first.
pub(crate) fn seed_phrase(words: &str) -> Markup {
    // [download] is a NATIVE browser download via a `data:` URL — a plain
    // template anchor, no imperative DOM/JS (the no-createElement rule). BIP-39
    // words are lowercase ASCII separated by single spaces, so the only char
    // needing percent-encoding for the data URL is the space.
    let download_href = format!(
        "data:text/plain;charset=utf-8,{}",
        words.replace(' ', "%20")
    );
    html! {
        div.seed-words { (words) }
        // GitHub #33: mobile backgrounding can refresh the tab and wipe this
        // view before the words are saved. Make the risk explicit AND give a
        // one-tap save (copy or download) so the words are banked first.
        p.seed-warn {
            "Save these words now — copy or download them before you switch apps. "
            "Backgrounding the browser can refresh this tab and lose them; "
            "they are shown once and never leave this device."
        }
        p.apex-fine {
            button #seed-copy type="button" data-action="copy-seed" data-arg=(words)
                .link-button { "copy" }
            " · "
            a.link-button download="localharness-seed.txt" href=(download_href) { "download" }
            " · "
            button type="button" data-action="hide-seed" .link-button { "hide" }
        }
    }
}

/// Post-payment seed backup (owner request): the safest moment to bank the
/// recovery phrase is right after the paid mint persisted it. Shows the words
/// with copy/download (the `seed_phrase` view) + a [continue] to the name-claim,
/// so a device loss / OPFS wipe / a reload in the narrow pay→persist window
/// can't strand the just-paid identity. Rendered into `#root` by
/// `credits::poll_and_finalize` on a confirmed onboarding mint.
pub(crate) fn onboard_seed_backup(words: &str) -> Markup {
    html! {
        (site_header(&Host::Apex))
        main.apex-main {
            div.col-chat {
                div #status .terminal-status role="status" aria-live="polite" {}
                section.step {
                    p { "you're in. save your recovery phrase first — it's the only key to this identity, and it's shown once." }
                    (seed_phrase(words))
                    button type="button" data-action="onboard-continue" .create-button { "I've saved it — continue" }
                }
            }
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
/// letterboxes it 16:9. Lives inside [`display_overlay`]; dismissing the
/// overlay tears the surface down (and stops any running cartridge).
/// This is the "screen" half of the Orbital-style compositor.
pub(crate) fn display_surface() -> Markup {
    html! {
        div.display-wrap {
            div.display-stage {
                canvas #display-canvas .display-canvas {}
            }
            (broadcast_composer_closed())
        }
    }
}

/// The broadcast composer — a cartridge called `host::agent::
/// broadcast_compose(title, default_body)` and the host owes it a text
/// input (a cartridge is pixels-only; only a real `<input>` can summon a
/// mobile keyboard). Swapped in over the canvas at `#broadcast-composer`;
/// [send] broadcasts the typed body under `title`, [cancel] (or Escape)
/// dismisses without sending. The title rides on the send button's
/// `data-arg` so dispatch needs no side-channel state.
pub(crate) fn broadcast_composer(title: &str, default_body: &str) -> Markup {
    html! {
        div #broadcast-composer .broadcast-composer {
            div.broadcast-composer-panel {
                div.broadcast-composer-title { (title) }
                input #broadcast-input
                    type="text"
                    value=(default_body)
                    maxlength="200"
                    autocomplete="off"
                    aria-label="notification message";
                div.prompt-actions {
                    button #broadcast-send-btn type="button"
                        data-action="broadcast-send" data-arg=(title) { "send" }
                    button type="button" data-action="broadcast-cancel"
                        .ghost { "cancel" }
                }
            }
        }
    }
}

/// The closed state of the broadcast composer — the hidden swap target.
/// Present on BOTH cartridge surfaces (the display overlay and the
/// fullscreen public face) so `agent_broadcast_compose` always has a node.
pub(crate) fn broadcast_composer_closed() -> Markup {
    html! { div #broadcast-composer hidden {} }
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
#[allow(clippy::too_many_arguments)] // a flat landing-page render; a struct would just be unpacked here
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
        // A public cartridge/HTML face keeps the site header (feedback: a
        // visitor should still see the platform chrome + a way back to
        // onboard, not a bare canvas). The cartridge fills the column below
        // it, letterboxed to its own `dims()` aspect.
        (public_face_header(owner_overlay))
        div.app-fullscreen {
            div.app-stage {
                canvas #display-canvas .display-canvas {}
            }
            (broadcast_composer_closed())
        }
    }
}

/// The slim header shown above a public-facing cartridge / HTML face: the
/// `localharness` brand menu (with a `home` link to the apex, where a visitor
/// with no identity is onboarded) and — for the owner previewing their own
/// face — a `[studio]` escape back to the workshop. Deliberately NOT the full
/// `site_header` (no admin/files: those are owner-studio tools, not a visitor
/// surface).
pub(crate) fn public_face_header(owner_overlay: bool) -> Markup {
    html! {
        header.site-header.public-face-header {
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
                (notif_bell())
                @if owner_overlay {
                    a.app-edit href="?edit=1" title="back to your studio" { "studio" }
                }
            }
        }
    }
}

/// The header notification bell — a DIRECT-tap affordance (real user gesture,
/// unlike the cartridge subscribe tap) that enables Web Push for this device
/// AND opens the in-app notification panel. `#notif-bell-badge` carries the
/// unread count; `#notif-bell-panel` is the dropdown list (filled by
/// `events::notifications`). One bell, every surface (public face + app header).
pub(crate) fn notif_bell() -> Markup {
    // A bell ICON (monochrome SVG, currentColor) — this is the notification LOG,
    // not a send button. Tap it to see your notifications.
    let bell = maud::PreEscaped(
        "<svg viewBox=\"0 0 16 16\" width=\"15\" height=\"15\" fill=\"none\" \
         stroke=\"currentColor\" stroke-width=\"1.3\" stroke-linecap=\"round\" \
         stroke-linejoin=\"round\" aria-hidden=\"true\">\
         <path d=\"M8 2.2a3 3 0 0 0-3 3c0 3.2-1.4 4.3-1.4 4.3h8.8S11 8.4 11 5.2a3 3 0 0 0-3-3z\"/>\
         <path d=\"M6.6 12.1a1.5 1.5 0 0 0 2.8 0\"/></svg>",
    );
    html! {
        div.notif-bell-wrap {
            button #notif-bell type="button" data-action="notif-bell"
                title="notifications" aria-label="notifications" .header-button.notif-bell-btn {
                (bell)
                span #notif-bell-badge .notif-badge hidden {}
            }
            (notif_list_panel(&[], None, true, false))
        }
    }
}

/// The notification-bell dropdown. Renders the in-app notification log (newest
/// first) plus an optional status `note` at the top (e.g. "notifications on" /
/// an error). `hidden` controls visibility — `push_to_bell` re-renders it
/// closed; the bell tap re-renders it open. All text auto-escaped by maud.
pub(crate) fn notif_list_panel(
    items: &[(String, String)],
    note: Option<&str>,
    hidden: bool,
    confirm_clear: bool,
) -> Markup {
    html! {
        div #notif-bell-panel .notif-panel hidden[hidden] {
            @if let Some(n) = note {
                div.notif-panel-empty { (n) }
            }
            @if items.is_empty() {
                @if note.is_none() {
                    div.notif-panel-empty { "no notifications yet" }
                }
            } @else {
                // Clear-all control with an inline two-step confirm (no JS
                // alert): [clear all] → "clear all? [yes] [cancel]" (feedback).
                div.notif-panel-actions {
                    @if confirm_clear {
                        span.notif-clear-prompt { "clear all?" }
                        button type="button" data-action="notif-clear-confirm" .notif-clear-btn { "yes" }
                        button type="button" data-action="notif-clear-cancel" .notif-clear-btn { "cancel" }
                    } @else {
                        button type="button" data-action="notif-clear-all" .notif-clear-btn { "clear all" }
                    }
                }
                @for (title, body) in items {
                    div.notif-item {
                        div.notif-item-title { (title) }
                        div.notif-item-body { (body) }
                    }
                }
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
