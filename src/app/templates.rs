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
                div.api-key-title { "gemini api key" }
                form onsubmit="return false" {
                    div.api-key-row {
                        input #api-key-input
                            type="password"
                            autocomplete="off"
                            placeholder="paste key" {}
                        button type="button"
                            data-action="save-api-key" { "save" }
                    }
                }
                div.api-key-hint {
                    a href="https://aistudio.google.com/apikey"
                        target="_blank" rel="noopener" { "get a free key →" }
                }
                div #api-key-msg .feedback-msg {}
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

/// Sticky header — brand left, two utility buttons right (feedback,
/// admin). Footer is gone; the feedback button moved into the header
/// so the bottom of the viewport can be claimed by the terminal /
/// active panel. Both header buttons share a fixed min-width via
/// `.header-button` so they read as a uniform pair regardless of
/// label length.
pub(crate) fn site_header(_host: &Host) -> Markup {
    html! {
        header.site-header {
            div.header-inner {
                h1.header-brand {
                    a href="https://localharness.xyz/" title="go home" { "localharness" }
                }
                button type="button"
                    data-action="feedback-open"
                    .header-button.feedback-button { "feedback" }
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

/// Version string, used in the admin dropdown bottom. Bumped in
/// lockstep with Cargo.toml.
pub(crate) const APP_VERSION: &str = "0.11.0";

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
                (send_button())
            }
            div.terminal-actions {
                button type="button" data-action="compact" .terminal-action title="compact conversation context" { "compact" }
                button type="button" data-action="reset" .terminal-action title="clear conversation" { "clear" }
            }
        }
    }
}

/// The terminal send button (`→`). Swapped out for [`stop_button`]
/// while a turn is streaming so the same slot becomes the kill switch.
pub(crate) fn send_button() -> Markup {
    html! {
        button #terminal-send .terminal-send data-action="send" title="send" { "→" }
    }
}

/// The stop button (`■`) shown in place of the send button while a turn
/// is in flight. Clicking it requests cooperative cancellation of the
/// running turn.
pub(crate) fn stop_button() -> Markup {
    html! {
        button #terminal-stop .terminal-send.terminal-stop data-action="stop-turn" title="stop" { "■" }
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

/// Compose-mode chrome — the host shell that includes one iframe per
/// named module. Each iframe carries `data-embed-name=<name>` so the
/// resize listener can target it. Iframe `src` is the embed-mode URL;
/// initial height defaults to a small placeholder until the module
/// posts `lh-embed-ready` and we resize.
pub(crate) fn compose_chrome(names: &[String]) -> Markup {
    html! {
        main.compose-shell {
            header.compose-header {
                h1.compose-title { "compose" }
                p.compose-sub { (names.len()) " module" @if names.len() != 1 { "s" } }
            }
            div.compose-grid {
                @for name in names {
                    div.compose-cell {
                        iframe.compose-iframe
                            src=(format!("https://{name}.localharness.xyz/?embed=1"))
                            data-embed-name=(name)
                            loading="lazy"
                            referrerpolicy="no-referrer" {}
                    }
                }
            }
        }
    }
}

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
/// subdomain. Newest first.
pub(crate) fn explore_grid(agents: &[(u64, String)]) -> Markup {
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
            @for (_, name) in agents {
                a.explore-card
                    href=(format!("https://{name}.localharness.xyz/"))
                    rel="noopener" {
                    span.explore-card-name { (name) }
                    span.explore-card-host { (name) ".localharness.xyz" }
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

            // Agent (right) — same archetype, no inner header.
            // Body is the financial-slot injected by kick_verification.
            (col_side(
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

/// Mobile-only tab bar shown above main on narrow viewports.
/// Switches the `tab-<name>` class on `#layout` so CSS shows
/// exactly one panel at a time. Hidden on desktop.
pub(crate) fn mobile_tabs() -> Markup {
    html! {
        nav.mobile-tabs {
            button #tab-btn-files type="button" data-action="show-tab" data-arg="files" .tab-button { "files" }
            button #tab-btn-chat type="button" data-action="show-tab" data-arg="chat" .tab-button.active { "chat" }
            button #tab-btn-agent type="button" data-action="show-tab" data-arg="agent" .tab-button { "agent" }
        }
    }
}

// site_footer() retired — the feedback button moved into site_header,
// the footer node is gone from the DOM, and the matching CSS is a
// `display: none` shim. If a footer ever comes back, reintroduce
// here with a meaningful purpose.

/// Feedback modal — opened from the footer button. Inline confirm
/// pattern (no JS dialog). Submit appends to `.lh_feedback.txt`
/// in OPFS for now; an on-chain FeedbackFacet submission lands
/// next (requires a contract + bundle wiring).
pub(crate) fn feedback_modal() -> Markup {
    html! {
        div #feedback-modal .feedback-modal {
            div.feedback-card {
                div.feedback-title { "feedback" }
                p.feedback-blurb {
                    "what's broken, missing, or wrong. submitted on-chain "
                    "and saved locally."
                }
                textarea #feedback-text
                    .feedback-textarea
                    rows="6"
                    placeholder="type here…" {}
                div.feedback-actions {
                    button type="button" data-action="feedback-submit" { "submit" }
                    button type="button" data-action="feedback-close" .ghost { "cancel" }
                }
                div #feedback-msg .feedback-msg {}
                div.feedback-recent-title { "recent" }
                div #feedback-list .feedback-list { "loading…" }
            }
        }
    }
}

/// Render the harvested on-chain feedback as a scrollable list, newest
/// first. Each row: relative time + short submitter + the text.
pub(crate) fn feedback_list(entries: &[crate::registry::FeedbackEntry]) -> Markup {
    if entries.is_empty() {
        return html! { div #feedback-list .feedback-list .feedback-empty { "no feedback yet" } };
    }
    html! {
        div #feedback-list .feedback-list {
            @for e in entries {
                div.feedback-item {
                    div.feedback-item-meta {
                        span.feedback-item-when { (fmt_unix_ago(e.timestamp)) }
                        span.feedback-item-who title=(e.sender) { (short_addr(&e.sender)) }
                    }
                    div.feedback-item-text { (e.text) }
                }
            }
        }
    }
}

/// Format a unix timestamp (seconds) as a short relative age like
/// "3h ago" / "5d ago", falling back to the day count for older items.
fn fmt_unix_ago(unix_secs: u64) -> String {
    let now_secs = (js_sys::Date::now() / 1000.0) as u64;
    let delta = now_secs.saturating_sub(unix_secs);
    if delta < 60 {
        "just now".to_string()
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

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

/// Apex page — `localharness.xyz/`. The subdomain IS the identity:
/// a visitor without a wallet still sees the claim form, and submit
/// auto-creates the wallet inside the same flow. No more "create
/// identity first, then claim a name" two-step. Seed import lives in
/// the admin dropdown for the recovery / cross-device case.
pub(crate) fn apex(host: &Host, _wallet_address_hex: Option<&str>) -> Markup {
    html! {
        (site_header(host))
        main.apex-main {
            div.col-chat {
                (apex_claim())
                div.apex-explore-link {
                    a href="?explore=1" { "explore all agents →" }
                }
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
            div.admin-dialog {
                (admin_identity_section(None, owner_hex.as_deref(), None))
                @if has_wallet {
                    (admin_credits_section())
                    (admin_devices_section())
                }
                (admin_security_collapsed())
                div.admin-footer {
                    button type="button" data-action="header-admin-close" .ghost { "close" }
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
    let name = match super::tenant::current() {
        super::tenant::Host::Tenant(n) => Some(n),
        _ => None,
    };
    let (owner_hex, tba_hex) = super::APP.with(|cell| {
        use super::VerifyState;
        let app = cell.borrow();
        let owner = match &app.verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            VerifyState::Visitor { visitor_address, .. } => Some(visitor_address.clone()),
            _ => None,
        };
        (owner, app.tba_address.clone())
    });
    html! {
        div #header-admin-panel .header-admin-panel {
            div.admin-dialog {
                (admin_identity_section(name.as_deref(), owner_hex.as_deref(), tba_hex.as_deref()))
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
                (admin_prompt_section())
                (admin_app_section())
                (admin_tool_allowlist_section())
                (admin_security_collapsed())
                div.admin-footer {
                    button type="button" data-action="header-admin-close" .ghost { "close" }
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
                    placeholder="optional — empty uses the default" {}
                div.prompt-actions {
                    button type="submit" .ghost { "save" }
                }
            }
            div #prompt-msg .admin-msg-slot {}
        }
    }
}

/// Publish-app section — pushes the device's local `app.rl` on-chain so
/// every visitor (not just this device) boots into the subdomain's app.
/// Owner-only; the button no-ops to an error if not verified as owner.
pub(crate) fn admin_app_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "app" }
            button type="button" data-action="publish-app" .ghost { "publish app on-chain" }
            div #publish-app-msg .admin-msg-slot {}
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
            } @else {
                p.admin-blurb { "verifying…" }
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

/// Credit balance + daily claim. Balance pill on the left is filled
/// async by `refresh_credits_pill`; the claim button on the right
/// fires `Action::ClaimCredits` and is a no-op if the user already
/// claimed today (the chain reverts; the bundle surfaces the revert
/// inline). Both `#credits-balance` and `#claim-credits-btn` are
/// addressable so events.rs can swap them independently.
pub(crate) fn admin_credits_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "credits" }
            div.admin-credits-row {
                code #credits-balance .admin-identity-value { "…" }
                button #claim-credits-btn
                    type="button"
                    data-action="claim-credits"
                    .ghost { "claim daily" }
            }
            div #claim-status .admin-msg-slot {}
            div #claim-credits-msg .admin-msg-slot {}
        }
    }
}

pub(crate) fn admin_devices_section() -> Markup {
    html! {
        div.admin-section {
            div.admin-section-title { "linked devices" }
            div #signer-list .admin-msg-slot { "loading…" }
            form #add-device-form .add-device-form
                data-action="add-device" {
                input #add-device-input
                    type="text"
                    placeholder="another device's 0x…"
                    autocomplete="off"
                    spellcheck="false"
                    maxlength="42" {}
                button #add-device-btn type="submit" .ghost { "add" }
            }
            div #add-device-msg .admin-msg-slot {}
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
            (lh_transfer_form(tba_hex))
        }
    }
}

/// $localharness transfer form, embedded in the financial card. Sends
/// from the visitor's apex wallet (signed via the iframe signer) to
/// whatever recipient the user types. Default recipient is the agent's
/// TBA so "support this agent" is the one-click path; the user can
/// overwrite to send anywhere.
pub(crate) fn lh_transfer_form(default_recipient: &str) -> Markup {
    html! {
        form #lh-transfer-form .lh-transfer data-action="lh-transfer" {
            div.lh-transfer-title { "send $localharness" }
            div.lh-transfer-row {
                input #lh-transfer-to
                    type="text"
                    autocomplete="off"
                    spellcheck="false"
                    placeholder="0x… recipient"
                    value=(default_recipient) {}
            }
            div.lh-transfer-row {
                input #lh-transfer-amount
                    type="text"
                    inputmode="decimal"
                    autocomplete="off"
                    spellcheck="false"
                    placeholder="amount" {}
                button type="submit" .lh-transfer-send { "send" }
            }
            div #lh-transfer-msg .lh-transfer-msg {}
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
                        div.agent-row-line {
                            a.agent-name
                                href=(format!("https://{}.localharness.xyz/", agent.name)) {
                                (agent.name)
                            }
                            @if main_token_id != 0 && agent.token_id == main_token_id {
                                span.main-badge title="primary identity" { "main" }
                            }
                            span.agent-row-spacer {}
                            button type="button"
                                data-action="agent-act-toggle"
                                data-arg=(agent.token_id)
                                .ghost.agent-act-btn { "act" }
                        }
                        div #(format!("agent-act-{}", agent.token_id))
                            .agent-act-panel hidden {}
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
                placeholder="recipient 0x…"
                autocomplete="off"
                spellcheck="false"
                maxlength="42" {}
            input
                id=(format!("agent-send-amt-{token_id}"))
                type="text"
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
                        li.file {
                            span.name data-action="opfs-open" data-arg=(entry.name) {
                                (entry.name)
                            }
                            @if let Some(size) = entry.size {
                                span.size { (format_bytes(size)) }
                            }
                            button.file-delete
                                type="button"
                                data-action="opfs-delete"
                                data-arg=(entry.name)
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
            textarea #fs-editor .editor-textarea { (text) }
        }
    }
}

/// DISPLAY surface — the framebuffer the cartridge loader blits into.
/// A small toolbar with a STOP button (kills the running cartridge's
/// frame loop and closes the surface) sits above a single `<canvas>`;
/// the canvas backing store is sized in `display::mount_canvas` and CSS
/// letterboxes it 16:9. This is the "screen" half of the Orbital-style
/// compositor.
pub(crate) fn display_surface() -> Markup {
    html! {
        div.display-wrap {
            div.display-toolbar {
                button type="button" data-action="display-stop" .display-stop
                    title="stop the running cartridge" { "stop" }
            }
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
pub(crate) fn app_fullscreen() -> Markup {
    html! {
        div.app-fullscreen {
            div.app-stage {
                canvas #display-canvas .display-canvas {}
            }
            a.app-edit href="?edit=1" title="edit this app" { "edit" }
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
