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

/// Header bar (h1 + version tag + tenant tag). Used by every chrome
/// variant so the brand + home link are consistent.
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
            span.tag { "web demo · 0.7.2" }
            span class={ "tag tenant-tag tenant-" (tenant_class) }
                title=(host.label()) { (tenant_label) }
        }
    }
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
                }
            }

            aside.col-fs {
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

/// Apex page — `localharness.xyz/`. Single-CTA marketing surface,
/// mirrors `self.tools/` in spirit: input + go button → redirect to
/// the named subdomain. Also surfaces the master wallet — generated
/// on first visit, persisted in this origin's OPFS — with affordances
/// to back up or import a seed phrase.
pub(crate) fn apex(host: &Host, wallet_address_hex: &str) -> Markup {
    html! {
        main.apex-main {
            div.col-chat {
                (site_header(host))

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
                            input #apex-input
                                type="text"
                                placeholder="your-name"
                                autocomplete="off"
                                spellcheck="false"
                                maxlength="32"
                                required {}
                            span.apex-suffix { ".localharness.xyz" }
                        }
                        button type="submit" { "claim →" }
                        div #apex-msg .apex-msg {}
                    }

                    p.apex-fine {
                        "a–z, 0–9, dash. 3–32 chars. "
                        "first device to claim a name owns it on that device — "
                        "a central registry that prevents cross-device squatting lands in "
                        a href="https://github.com/compusophy/localharness/blob/main/DESIGN_M5_PLUS.md" { "M7" }
                        "."
                    }
                }

                section.apex-wallet {
                    h3.apex-sub-headline { "your master identity" }
                    p.apex-sub {
                        "a secp256k1 keypair generated on this device. "
                        "every subdomain you claim will be tied to this address "
                        "(once the on-chain registry lands in M7). "
                        "the only thing you need to back up to keep your account."
                    }
                    div.wallet-address-row {
                        span.wallet-label { "address" }
                        code #wallet-address .wallet-address { (wallet_address_hex) }
                    }

                    details.apex-details {
                        summary { "show seed phrase (12 words)" }
                        div.apex-import {
                            p.apex-fine {
                                "write these 12 words down somewhere safe. "
                                "anyone with this phrase controls your identity and every subdomain you own. "
                                "click the button below to reveal."
                            }
                            div #seed-reveal .seed-reveal {
                                button type="button" data-action="reveal-seed" { "I have a pen and paper — reveal" }
                            }
                        }
                    }

                    details.apex-details {
                        summary { "import a seed phrase from another device" }
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

                footer {
                    p {
                        "Open source · "
                        a href="https://github.com/compusophy/localharness" { "github.com/compusophy/localharness" }
                        " · Rust → wasm32 · no analytics, no telemetry, no backend."
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

/// Tenant subdomain that no one on this device has claimed yet —
/// "unclaimed mode". Show a "claim this name" CTA + an "I already
/// own it elsewhere" import affordance (paste your owner UUID).
pub(crate) fn unclaimed(host: &Host, name: &str) -> Markup {
    html! {
        main.apex-main {
            div.col-chat {
                (site_header(host))

                section.apex-hero {
                    h2.apex-headline { "this name is open: " (name) }
                    p.apex-sub {
                        "no one on this device has claimed " strong { (name) ".localharness.xyz" }
                        " yet. claim it to start using it as your space — your conversations and "
                        "files will live in this subdomain's private OPFS storage."
                    }

                    form.apex-form data-action="claim-here" {
                        button type="submit" { "claim " (name) " →" }
                        div #claim-msg .apex-msg {}
                    }

                    details.apex-details {
                        summary { "already own this name on another device?" }
                        div.apex-import {
                            p.apex-fine {
                                "if you claimed " (name) " elsewhere, paste the owner UUID from "
                                "that device's OPFS panel (file " code { ".lh_owner" } "). "
                                "this puts a copy of the marker on this device too. "
                                "(cross-device claim sync lands in M7.)"
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
