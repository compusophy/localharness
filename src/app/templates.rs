//! All HTML in the browser app is produced here, via [`maud`]
//! compile-time templates. Templates return `Markup`; callers turn
//! them into strings and ship them into the DOM via the helpers in
//! [`super::dom`]. **No template function takes a DOM handle** — they
//! are pure `inputs → HTML` functions, so they're trivial to read,
//! test, and recompose.

use maud::{html, Markup, PreEscaped};

use crate::filesystem::{DirEntry, EntryKind};
use crate::types::{ToolCall, ToolResult};

use super::tenant::Host;
use super::VerifyState;

/// Render assistant markdown to HTML and wrap as `Markup` so callers
/// can swap it straight into the DOM. pulldown-cmark sanitises by
/// default (no raw HTML pass-through), so `PreEscaped` is safe.
pub(crate) fn rendered_markdown(raw: &str) -> Markup {
    use pulldown_cmark::{html, Options, Parser};
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_TASKLISTS);
    let parser = Parser::new_ext(raw, opts);
    let mut out = String::with_capacity(raw.len());
    html::push_html(&mut out, parser);
    html! { (PreEscaped(out)) }
}

/// Three-zone sticky header — brand left, subdomain center, admin
/// right. Version, verify-pill, TBA-pill all moved off the header
/// per the ultra-minimal direction; owner / TBA / balance live in
/// the agent tab now and the version lives at the bottom of the
/// admin dropdown.
pub(crate) fn site_header(host: &Host) -> Markup {
    let center = match host {
        Host::Tenant(name) => name.clone(),
        _ => String::new(),
    };
    html! {
        header.site-header {
            div.header-inner {
                h1.header-brand {
                    a href="https://localharness.xyz/" title="go home" { "localharness" }
                }
                div.header-center { (center) }
                div #header-admin .header-admin {
                    button type="button"
                        data-action="header-admin-toggle"
                        .admin-button { "admin" }
                    div #header-admin-panel hidden {}
                }
            }
        }
    }
}

/// Version string, used in the admin dropdown bottom. Bumped in
/// lockstep with Cargo.toml.
pub(crate) const APP_VERSION: &str = "0.10.16";

/// Terminal input — just `>` prompt + textarea + → send. Status line
/// stays in the DOM (id="status") for dispatcher messages but renders
/// empty by default so it doesn't add visual noise.
pub(crate) fn terminal_input() -> Markup {
    html! {
        div.terminal-body {
            div #status .terminal-status {}
            div.terminal-row {
                span.terminal-prompt { ">" }
                textarea #prompt rows="1" {}
                button.terminal-send data-action="send" title="send" { "→" }
            }
        }
    }
}

/// ERC-6551 token-bound account pill — the agent's wallet address.
/// Lives in the header next to verify-pill on tenant subdomains.
#[allow(dead_code)] // retired in 0.10.14 — TBA shows in the agent tab
pub(crate) fn tba_pill(address: &str) -> Markup {
    let short = short_addr(address);
    let title = format!("agent wallet (ERC-6551): {address}");
    html! {
        a #tba-pill
            class="tag tba-pill"
            href=(format!("https://moderato.tempo.xyz/address/{address}"))
            target="_blank"
            rel="noopener"
            title=(title) {
            "💰 " (short)
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
        span #verify-pill class=(class) title=(title) { (label) }
    }
}

fn short_addr(addr: &str) -> String {
    let stripped = addr.trim_start_matches("0x");
    if stripped.len() < 8 {
        return addr.to_string();
    }
    format!("0x{}…{}", &stripped[..4], &stripped[stripped.len() - 4..])
}

/// The full app chrome (key + prompt + transcript + OPFS panel). Used
/// when we're on a claimed tenant subdomain or any fallback
/// (localhost, vercel preview).
pub(crate) fn chrome(host: &Host) -> Markup {
    html! {
        (site_header(host))
        main #layout .layout.view-collapsed {
            // Files (left) — files-rail wraps a col-side panel.
            button type="button" data-action="toggle-files"
                .side-rail.files-rail {
                span.rail-label { "files" }
            }
            (col_side(
                html! {
                    div.panel-title { "files" }
                    div.panel-actions {
                        button data-action="opfs-refresh" .panel-button { "refresh" }
                        (opfs_wipe_armed_inline())
                    }
                },
                html! {
                    div #fs-breadcrumb .fs-breadcrumb { "/" }
                    ul #fs-list .fs-list {}
                },
                "col-fs",
            ))

            // Center column — vertical stack:
            //   [edit-rail][edit-panel?][transcript][terminal-panel?][terminal-rail]
            // Clicking terminal-rail collapses transcript + terminal
            // (so the editor can take the whole center). Clicking
            // edit-rail collapses just the editor panel.
            div.col-chat {
                button type="button" data-action="toggle-view"
                    .top-rail.view-rail {
                    span.rail-label { "edit" }
                }
                section.view-panel {
                    div #view-content .view-content {}
                }
                div #transcript .transcript {}
                section.terminal-panel {
                    (terminal_input())
                }
                button type="button" data-action="toggle-terminal"
                    .bottom-rail.terminal-rail {
                    span.rail-label { "terminal" }
                }
            }

            // Agent (right) — financial-rail wraps a col-side panel
            // whose body is the financial-slot injected by
            // kick_verification.
            (col_side(
                html! {
                    div.panel-title { "agent" }
                },
                html! {
                    div #financial-slot .financial-placeholder { "—" }
                },
                "col-financial",
            ))
            button type="button" data-action="toggle-financial"
                .side-rail.financial-rail {
                span.rail-label { "agent" }
            }
        }
    }
}

/// SSOT side-panel archetype — used by both `col-fs` (files) and
/// `col-financial` (agent). `extra_class` tags the panel for
/// position-specific styling (border-left vs border-right, the
/// per-column collapse selector). Both columns now look identical
/// because their visual treatment lives entirely in CSS via
/// `.col-side` (and `.panel-header` / `.panel-body` inside).
fn col_side(header: Markup, body: Markup, extra_class: &str) -> Markup {
    let cls = format!("col-side {extra_class}");
    html! {
        aside class=(cls) {
            header.panel-header { (header) }
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
            div.role { (role) }
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

/// A tool-call block in its initial "running" state.
pub(crate) fn tool_call_block(seg_id: u32, call: &ToolCall) -> Markup {
    let block_id = format!("tool-{seg_id}");
    let status_id = format!("tool-{seg_id}-status");
    let result_id = format!("tool-{seg_id}-result");
    let args_pretty = serde_json::to_string_pretty(&call.args).unwrap_or_else(|_| "{}".into());
    html! {
        details id=(block_id) .tool-call {
            summary {
                span.tc-name { (call.name) }
                span id=(status_id) .tc-status.running {}
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

/// Apex page — `localharness.xyz/`. Renders exactly one of two
/// states at a time. Header carries the admin dropdown for seed
/// reveal / import / reset; main body shows only the current step.
pub(crate) fn apex(host: &Host, wallet_address_hex: Option<&str>) -> Markup {
    html! {
        (site_header(host))
        main.apex-main {
            div.col-chat {
                @match wallet_address_hex {
                    None => (apex_step_identity()),
                    Some(_) => (apex_step_agents()),
                }
            }
        }
    }
}

/// Step 1 — no identity yet. Only thing on the page.
fn apex_step_identity() -> Markup {
    html! {
        section.step.step-identity {
            div.identity-actions {
                button type="button" data-action="create-identity" { "create identity" }
                button type="button" data-action="show-import" .ghost { "import seed" }
            }
            div #identity-msg .step-msg {}
            div #import-slot {}
        }
    }
}

/// Step 2 — identity exists. Agents list (async, may be empty) plus
/// the create-agent input. Wallet address is NOT shown here — it
/// lives in the admin dropdown to keep the main flow uncluttered.
fn apex_step_agents() -> Markup {
    html! {
        section.step.step-agents {
            div #agents-list .agents-list {
                p.step-msg { "(loading your agents…)" }
            }

            form.create-form data-action="apex-claim" {
                div.create-input-row {
                    input #apex-input
                        type="text"
                        placeholder="my-agent"
                        autocomplete="off"
                        spellcheck="false"
                        maxlength="32"
                        required {}
                    span.create-suffix { ".localharness.xyz" }
                }
                button type="submit" .create-button { "create" }
                p.create-hint { "3–32 chars, a–z 0–9 dash." }
                div #apex-msg .step-msg {}
            }
        }
    }
}

/// Header admin dropdown — toggled by the "admin" button in the
/// header. Single source of truth for seed reveal, seed import, and
/// reset-local-state on apex; tenant chrome reuses the same dropdown
/// but show different reset copy via [`admin_dropdown_tenant`].
pub(crate) fn admin_dropdown_apex() -> Markup {
    html! {
        div #header-admin-panel .header-admin-panel {
            div.admin-section {
                div.admin-section-title { "seed phrase" }
                div #seed-reveal .seed-reveal {
                    button type="button" data-action="reveal-seed" .ghost { "reveal" }
                }
            }
            div.admin-section {
                div.admin-section-title { "import a different seed" }
                (import_seed_inline())
            }
            div.admin-section {
                div.admin-section-title { "reset" }
                div #reset-confirm-slot {
                    button type="button" data-action="reset-arm" .ghost { "reset…" }
                }
            }
            div.admin-footer {
                button type="button" data-action="header-admin-close" .ghost { "close" }
                span.admin-version { (APP_VERSION) }
            }
        }
    }
}

/// Tenant-variant of the admin dropdown — gemini api key (the chat
/// surface only works when this is set), then reset-local-state.
/// No seed/import section because the wallet lives at apex, not here.
pub(crate) fn admin_dropdown_tenant() -> Markup {
    html! {
        div #header-admin-panel .header-admin-panel {
            div.admin-section {
                div.admin-section-title { "gemini api key " span #keymeta {} }
                form.key-form onsubmit="return false" {
                    div.key-row {
                        input #key
                            type="password"
                            autocomplete="off"
                            placeholder="paste key" {}
                        button.ghost
                            type="button"
                            data-action="clear-key" { "clear" }
                    }
                }
            }
            div.admin-section {
                div.admin-section-title { "reset" }
                div #reset-confirm-slot {
                    button type="button" data-action="reset-arm" .ghost { "reset…" }
                }
            }
            div.admin-footer {
                button type="button" data-action="header-admin-close" .ghost { "close" }
                span.admin-version { (APP_VERSION) }
            }
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
            span.reset-confirm-prompt { "are you sure?" }
            div.reset-confirm-actions {
                button type="button" data-action="reset-confirm" .danger { "yes, wipe" }
                button type="button" data-action="reset-cancel" .ghost { "cancel" }
            }
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

/// Full pricing card — used inside the financial column. Injected
/// by `kick_verification` when the visitor is the owner; visitors
/// get a read-only price line via [`pricing_readonly_line`] instead.
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
    tba_hex: &str,
    owner_hex: &str,
    lh_balance_wei: u128,
    _price_wei: u128,
    _is_owner: bool,
) -> Markup {
    let tba_url = format!("https://moderato.tempo.xyz/address/{tba_hex}");
    let owner_url = format!("https://moderato.tempo.xyz/address/{owner_hex}");
    let balance_display = super::format_wei_as_test_eth(lh_balance_wei);
    html! {
        section #financial-slot .financial-card {
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
pub(crate) fn agents_list(agents: &[crate::app::registry::OwnedToken]) -> Markup {
    if agents.is_empty() {
        return html! {
            div #agents-list .agents-list .agents-empty {}
        };
    }
    html! {
        div #agents-list .agents-list {
            ul.agents-rows {
                @for agent in agents {
                    li.agent-row {
                        a.agent-name
                            href=(format!("https://{}.localharness.xyz/", agent.name)) {
                            (agent.name) ".localharness.xyz"
                        }
                        span.agent-id { "#" (agent.token_id) }
                        @if let Some(tba) = &agent.tba {
                            a.agent-tba
                                href=(format!("https://moderato.tempo.xyz/address/{tba}"))
                                target="_blank"
                                rel="noopener"
                                title=(format!("agent wallet (ERC-6551): {tba}")) {
                                "💰 " (short_addr(tba))
                            }
                        }
                    }
                }
            }
        }
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

/// (Retired in 0.10.10 — visitor context now lives in the terminal
/// status line via `dom::set_status`. Kept for now in case we want
/// a richer banner later.)
#[allow(dead_code)]
pub(crate) fn visitor_banner(owner_address: &str) -> Markup {
    html! {
        div #input-region .visitor-banner {
            h3 { "visitor mode · read-only" }
            p {
                "this subdomain is owned by "
                code { (owner_address) }
                "."
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
/// "unclaimed mode". M8 update: the recommended path is now a
/// cross-device-portable on-chain claim via apex; the original local
/// UUID flow stays as a "just save it on this device" fallback so
/// existing users aren't broken.
pub(crate) fn unclaimed(host: &Host, name: &str) -> Markup {
    let apex_claim_url = format!("https://localharness.xyz/?prefill={name}");
    html! {
        (site_header(host))
        main.apex-main {
            div.col-chat {
                section.step.step-unclaimed {
                    h2.unclaimed-name { (name) ".localharness.xyz" }
                    p.step-msg { "this name is open. claim it from the apex." }
                    a.button-link href=(apex_claim_url) { "claim on apex" }
                }
            }
        }
    }
}

// --- OPFS panel templates --------------------------------------------

pub(crate) fn opfs_breadcrumb(cwd: &[String]) -> Markup {
    html! {
        a data-action="opfs-nav" data-arg="" { "/" }
        @for i in 0..cwd.len() {
            @let arg = cwd[..=i].join("/");
            a data-action="opfs-nav" data-arg=(arg) { (cwd[i]) "/" }
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
                        li.dir data-action="opfs-nav" data-arg=(arg) {
                            span.name { (entry.name) }
                        }
                    }
                    _ => {
                        li.file data-action="opfs-open" data-arg=(entry.name) {
                            span.name { (entry.name) }
                            @if let Some(size) = entry.size {
                                span.size { (format_bytes(size)) }
                            }
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

/// Retired in 0.10.16 — every file open is now the editor directly.
#[allow(dead_code)]
pub(crate) fn opfs_viewer(display_path: &str, name: &str, text: &str) -> Markup {
    html! {
        div #fs-viewer-wrap {
            div.fs-viewer-header {
                span #fs-viewer-name { (display_path) }
                span.fs-viewer-actions {
                    button.close-viewer
                        type="button"
                        data-action="opfs-edit"
                        data-arg=(name) { "edit" }
                    " "
                    button.close-viewer
                        type="button"
                        data-action="opfs-close-viewer" { "close" }
                }
            }
            pre #fs-viewer .fs-viewer { (text) }
        }
    }
}

/// Editable variant. The textarea has id `fs-editor` so the save
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
            textarea #fs-editor .editor-textarea { (text) }
        }
    }
}

/// Retired in 0.10.13 — the view panel is now collapsed via a CSS
/// class flip on `#layout` rather than swapping a placeholder DOM
/// node back in. Kept allow-dead-code so older call sites compile.
#[allow(dead_code)]
pub(crate) fn opfs_viewer_placeholder() -> Markup {
    html! {
        div #fs-viewer-wrap hidden {
            div.fs-viewer-header {
                span #fs-viewer-name {}
                button.close-viewer
                    type="button"
                    data-action="opfs-close-viewer" { "close" }
            }
            pre #fs-viewer .fs-viewer {}
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
