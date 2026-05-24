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

/// Header bar (h1 + version tag + tenant tag + verification pill).
/// Used by every chrome variant so the brand + home link + verify
/// status are consistent.
fn site_header(host: &Host) -> Markup {
    let (tenant_class, tenant_label) = match host {
        Host::Tenant(name) => ("tenant", format!("tenant · {name}")),
        Host::Apex => ("apex", "apex".to_string()),
        Host::Other(_) => ("other", host.label()),
    };
    html! {
        header {
            h1 {
                a href="https://localharness.xyz/" title="go home" { "localharness" }
            }
            span.tag { "web demo · 0.10.3" }
            span class={ "tag tenant-tag tenant-" (tenant_class) }
                title=(host.label()) { (tenant_label) }
            // Verify pill — present only on tenant subdomains.
            @if matches!(host, Host::Tenant(_)) {
                (verify_pill(&VerifyState::Pending))
                // TBA pill placeholder — filled in by kick_verification
                // once the address is fetched from the registry.
                span #tba-pill {}
            }
        }
    }
}

/// ERC-6551 token-bound account pill — the agent's wallet address.
/// Lives in the header next to verify-pill on tenant subdomains.
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
        main {
            div.col-chat {
                (site_header(host))
                p.sub {
                    "Streaming Gemini chat compiled to wasm32 — no backend, key stays in this tab. "
                    "The model can read/write files in your tab's private OPFS storage; "
                    "conversation history persists across reloads. "
                    "UI is rendered entirely by Rust → HTML; no JavaScript application code in the page."
                }

                div #input-region {
                    div.row {
                        label for="key" {
                            "Gemini API key "
                            span #keymeta {}
                        }
                        div.key-row {
                            input #key
                                type="password"
                                autocomplete="off"
                                placeholder="paste key (sessionStorage only)" {}
                            button.ghost
                                type="button"
                                data-action="clear-key"
                                title="Wipe the cached key from sessionStorage" {
                                "clear"
                            }
                        }
                    }

                    div.row {
                        label for="prompt" { "Prompt" }
                        textarea #prompt
                            placeholder="try: 'create notes.md with a haiku about Rust', then 'list my files', then 'show me notes.md' · ⌘/Ctrl+Enter to send" {}
                    }

                    div.actions {
                        button data-action="send" { "send" }
                        button.ghost data-action="reset" { "new conversation" }
                    }
                }

                div #status .status { "loading…" }
                div #transcript .transcript {}

                footer {
                    p {
                        "Compiled from "
                        a href="https://github.com/compusophy/localharness" { "localharness" }
                        " (Rust→wasm32). Driving the full "
                        code { "Agent" }
                        " loop in the browser — same code path the CLI host uses. "
                        strong { "10 of 11 builtins wired" }
                        " — including the 6 fs tools against per-origin OPFS storage; "
                        code { "run_command" }
                        " remains native-only. Tool calls render inline as they execute; "
                        "click a file in the panel to view or edit."
                    }
                    p {
                        "Conversation history and OPFS files both persist across "
                        "reloads (per-origin sandbox). Use "
                        strong { "new conversation" }
                        " to start fresh, or "
                        strong { "wipe" }
                        " in the OPFS panel to clear files."
                    }
                    (admin_corner())
                }
            }

            aside.col-fs {
                (pricing_card_placeholder())
                div.fs-panel {
                    div.fs-header {
                        div.fs-title { "OPFS · this tab" }
                        div.fs-actions {
                            button data-action="opfs-refresh"
                                title="Re-list the current directory" { "refresh" }
                            button data-action="opfs-wipe"
                                title="Delete all files in OPFS for this origin" { "wipe" }
                        }
                    }
                    div #fs-breadcrumb .fs-breadcrumb { "/" }
                    ul #fs-list .fs-list {}
                    div #fs-viewer-wrap hidden? {
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

/// Apex page — `localharness.xyz/`. Identity sidecar at the top
/// gates the claim form: without an on-device wallet the form is
/// disabled and the sidecar shows `[Create identity]` + `[Import
/// seed]`; with a wallet the form is live and the sidecar collapses
/// to address + agents + seed/import disclosures.
pub(crate) fn apex(host: &Host, wallet_address_hex: Option<&str>) -> Markup {
    let has_identity = wallet_address_hex.is_some();
    html! {
        main.apex-main {
            div.col-chat {
                (site_header(host))

                (identity_sidecar(wallet_address_hex))

                section.apex-hero {
                    h2.apex-headline { "your own browser-resident agent." }
                    p.apex-sub {
                        "pick a name. it becomes your subdomain. "
                        "the agent loop, your files, and your conversation history all live "
                        "in that subdomain's per-origin sandbox — no servers, no accounts to recover, "
                        "no one else can read your data."
                    }

                    form.apex-form data-action="apex-claim" {
                        div.apex-input-row {
                            @if has_identity {
                                input #apex-input
                                    type="text"
                                    placeholder="your-name"
                                    autocomplete="off"
                                    spellcheck="false"
                                    maxlength="32"
                                    required {}
                            } @else {
                                input #apex-input
                                    type="text"
                                    placeholder="your-name"
                                    autocomplete="off"
                                    spellcheck="false"
                                    maxlength="32"
                                    disabled
                                    required {}
                            }
                            span.apex-suffix { ".localharness.xyz" }
                        }
                        @if has_identity {
                            button type="submit" { "claim →" }
                        } @else {
                            button type="submit" disabled
                                title="create or import an identity above first" {
                                "claim →"
                            }
                        }
                        div #apex-msg .apex-msg {
                            @if !has_identity {
                                span style="color:var(--muted)" {
                                    "create or import an identity above to claim a name."
                                }
                            }
                        }
                    }

                    p.apex-fine {
                        "a–z, 0–9, dash. 3–32 chars. "
                        "names mint as NFTs on the Tempo Moderato registry; the wallet "
                        "that claims a name owns it across devices."
                    }
                }

                footer {
                    p {
                        "Open source · "
                        a href="https://github.com/compusophy/localharness" { "github.com/compusophy/localharness" }
                        " · Rust → wasm32 · no analytics, no telemetry, no backend."
                    }
                    (admin_corner())
                }
            }
        }
    }
}

/// Footer-corner admin affordance — a small muted link that toggles
/// an inline panel containing the "Reset local state" button.
/// Embedded in both apex and tenant chrome footers so a tester can
/// nuke this origin's OPFS without opening an incognito tab.
pub(crate) fn admin_corner() -> Markup {
    html! {
        div #admin-corner .admin-corner {
            a href="#" data-action="admin-toggle" .admin-link { "admin" }
            div #admin-panel hidden {}
        }
    }
}

/// Expanded admin panel — swapped into `#admin-panel` when the user
/// clicks the admin link. `body` is origin-specific warning text the
/// dispatcher composed from `tenant::current()`.
pub(crate) fn admin_panel_open(body: &str) -> Markup {
    html! {
        div #admin-panel .admin-panel {
            p.apex-fine { (body) }
            div.admin-actions {
                button type="button" data-action="admin-reset" .ghost { "reset local state" }
                button type="button" data-action="admin-close" .ghost { "cancel" }
            }
        }
    }
}

/// Pricing card placeholder. Painted into the right sidebar at
/// chrome-render time; `paint_tenant` swaps the inner once it knows
/// the verify state + current price.
pub(crate) fn pricing_card_placeholder() -> Markup {
    html! {
        section #pricing-card .pricing-card {
            div.pricing-header {
                div.pricing-title { "pricing" }
            }
            div #pricing-body .pricing-body {
                p.apex-fine { "(loading…)" }
            }
        }
    }
}

/// Pricing card body painted when verify is settled. Owner gets an
/// inline edit form; everyone else gets a read-only display of the
/// current per-turn cost.
pub(crate) fn pricing_card_body(price_wei: u128, is_owner: bool) -> Markup {
    let display = if price_wei == 0 {
        "free".to_string()
    } else {
        format!("{} test ETH/turn", super::format_wei_as_test_eth(price_wei))
    };
    html! {
        div #pricing-body .pricing-body {
            div.pricing-value { (display) }
            @if is_owner {
                div.pricing-edit {
                    input #pricing-input
                        type="text"
                        inputmode="decimal"
                        placeholder="0.001"
                        value=(if price_wei == 0 { String::new() } else { super::format_wei_as_test_eth(price_wei) }) {}
                    span.pricing-unit { "test ETH/turn" }
                    button.ghost
                        type="button"
                        data-action="pricing-save" { "save" }
                }
                div #pricing-msg .pricing-msg {}
            }
        }
    }
}

/// Identity sidecar that sits between the header and the claim form.
/// Two shapes — pre-identity (create / import buttons) and
/// post-identity (address + agents list + seed/import disclosures).
fn identity_sidecar(wallet_address_hex: Option<&str>) -> Markup {
    match wallet_address_hex {
        None => html! {
            section #identity-sidecar .apex-wallet .identity-empty {
                h3.apex-sub-headline { "sign in" }
                p.apex-sub {
                    "your identity is a secp256k1 keypair stored in this origin's "
                    "per-tab sandbox. create one to claim a fresh name, or import "
                    "your existing 12-word seed if you already own names on another device."
                }
                div.identity-actions {
                    button type="button" data-action="create-identity" { "create identity" }
                    button type="button" data-action="show-import" .ghost { "import existing seed" }
                }
                div #identity-msg .apex-msg {}
                div #import-slot {}
            }
        },
        Some(addr) => html! {
            section #identity-sidecar .apex-wallet {
                div.wallet-address-row {
                    span.wallet-label { "identity" }
                    code #wallet-address .wallet-address { (addr) }
                }

                // Async-populated by paint_apex once the registry
                // returns the user's owned tokens. Empty state shows
                // "(loading…)" while in flight.
                div #agents-list .agents-list {
                    p.apex-fine { "(loading your agents…)" }
                }

                details.apex-details {
                    summary { "show seed phrase (12 words)" }
                    div.apex-import {
                        p.apex-fine {
                            "write these 12 words down somewhere safe. "
                            "anyone with this phrase controls your identity and every subdomain you own."
                        }
                        div #seed-reveal .seed-reveal {
                            button type="button" data-action="reveal-seed" { "I have a pen and paper — reveal" }
                        }
                    }
                }

                details.apex-details {
                    summary { "import a different seed phrase" }
                    div.apex-import {
                        p.apex-fine {
                            "paste 12 words separated by spaces. "
                            strong { "this replaces your current wallet" }
                            " on this device — back up the existing seed phrase first if you want to keep it."
                        }
                        textarea #import-seed
                            placeholder="abandon ability able about above absent absorb abstract absurd abuse access accident"
                            rows="3" {}
                        button type="button" data-action="import-seed" { "import" }
                        div #seed-msg .apex-msg {}
                    }
                }
            }
        },
    }
}

/// Inline import-seed form swapped into `#import-slot` when a
/// pre-identity visitor clicks "import existing seed".
pub(crate) fn import_seed_inline() -> Markup {
    html! {
        div #import-slot .apex-import {
            p.apex-fine {
                "paste 12 words separated by spaces. they'll be used to derive "
                "your secp256k1 key — make sure no one's watching."
            }
            textarea #import-seed
                placeholder="abandon ability able about above absent absorb abstract absurd abuse access accident"
                rows="3" {}
            button type="button" data-action="import-seed" { "import" }
            div #seed-msg .apex-msg {}
        }
    }
}

/// Render the "your agents" table on apex. `agents` is what the
/// registry's `list_owned_tokens(wallet_address)` returned.
pub(crate) fn agents_list(agents: &[crate::app::registry::OwnedToken]) -> Markup {
    if agents.is_empty() {
        return html! {
            div #agents-list .agents-list {
                p.apex-fine {
                    "you don't own any agents yet — claim a name above to mint your first."
                }
            }
        };
    }
    html! {
        div #agents-list .agents-list {
            h4.agents-headline { "your agents (" (agents.len()) ")" }
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

/// Visitor-mode replacement for `#input-region` on a tenant subdomain
/// when the verifier confirms the visitor isn't the on-chain owner.
/// Hides every write affordance; the transcript + OPFS panel still
/// render because they live outside `#input-region`.
pub(crate) fn visitor_banner(owner_address: &str) -> Markup {
    html! {
        div #input-region .visitor-banner {
            h3 { "visitor mode · read-only" }
            p {
                "this subdomain is owned by "
                code { (owner_address) }
                " on the Tempo Moderato registry. you can read the public "
                "transcript and any OPFS files the owner has made visible, "
                "but you can't send messages or write state."
            }
            p.apex-fine {
                "want your own space? "
                a href="https://localharness.xyz/" { "go to apex" }
                " and claim a name."
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
        main.apex-main {
            div.col-chat {
                (site_header(host))

                section.apex-hero {
                    h2.apex-headline { "this name is open: " (name) }
                    p.apex-sub {
                        "no one on this device has claimed " strong { (name) ".localharness.xyz" }
                        " yet. you can claim it on-chain (cross-device, owned by your master "
                        "wallet) or just locally on this device."
                    }

                    div.claim-cards {
                        section.claim-card.claim-primary {
                            h3 { "claim on-chain" }
                            p.apex-fine {
                                "recommended. mints an agentId in the registry contract on Tempo "
                                "testnet, tied to your master wallet. any device with your seed "
                                "phrase can access this space. takes ~5 seconds."
                            }
                            a.button-link href=(apex_claim_url) { "go to apex →" }
                        }
                        section.claim-card.claim-secondary {
                            h3 { "save locally only" }
                            p.apex-fine {
                                "writes a random UUID to this device's OPFS. fast, no blockchain. "
                                "but: only this device can access it; lose this browser profile, "
                                "lose the name."
                            }
                            form.apex-form data-action="claim-here" {
                                button type="submit" { "claim " (name) " locally" }
                                div #claim-msg .apex-msg {}
                            }
                        }
                    }

                    details.apex-details {
                        summary { "already own this name on another device (UUID-style)?" }
                        div.apex-import {
                            p.apex-fine {
                                "if you claimed " (name) " on another device before on-chain "
                                "registration landed, paste the owner UUID from that device's "
                                "OPFS panel (file " code { ".lh_owner" } "). new claims should "
                                "go through the on-chain flow above instead."
                            }
                            div.apex-input-row {
                                input #import-uuid
                                    type="text"
                                    placeholder="00000000-0000-0000-0000-000000000000"
                                    autocomplete="off"
                                    spellcheck="false" {}
                            }
                            button type="button" data-action="import-owner" { "save UUID" }
                        }
                    }
                }

                footer {
                    p {
                        "want a different name? "
                        a href="https://localharness.xyz/" { "go home" }
                        " and pick a new one."
                    }
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

/// The viewer pane swapped into `#fs-viewer-wrap` when a file is open.
/// `name` is the leaf filename (used by the "edit" data-arg).
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
        div #fs-viewer-wrap {
            div.fs-viewer-header {
                span #fs-viewer-name { (display_path) " · editing" }
                span.fs-viewer-actions {
                    button.close-viewer
                        type="button"
                        data-action="opfs-save"
                        data-arg=(name) { "save" }
                    " "
                    button.close-viewer
                        type="button"
                        data-action="opfs-open"
                        data-arg=(name) { "cancel" }
                }
            }
            textarea #fs-editor .fs-viewer .fs-editor { (text) }
        }
    }
}

/// The hidden placeholder shape — restored when the viewer closes so a
/// later open can swap-outer it again.
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
