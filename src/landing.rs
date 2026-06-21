//! Apex fresh-visitor landing — the product front door at
//! `localharness.xyz` for a visitor with NO identity.
//!
//! Hoisted out of the wasm-gated `app/` tree (same pattern as `raster.rs`
//! / `compose.rs`) so the exact shipping markup renders NATIVELY: the
//! `landing_preview` test below writes `target/landing-preview.html`
//! (stylesheet linked relatively into `web/`) so the page can be
//! screenshot-reviewed without an identity-free browser profile.
//! Regenerate with:
//!
//! ```sh
//! cargo test --features browser-app landing_preview
//! # then open target/landing-preview.html
//! ```
//!
//! Funnel: ONE decision — create a wallet. The fresh-visitor front door is a
//! single `create wallet` CTA (the paid entry: it creates AND funds the
//! wallet, so there's no unfunded-wallet path and no 0-$LH name-squatting).
//! Invited users skip this entirely — an `?invite=CODE` link/QR auto-redeems
//! on mount (`app/mod.rs` → `try_redeem_pending_invite`). Redeeming a code and
//! importing a seed are recovery/edge paths and live in the admin panel, NOT
//! here. Explore is post-auth only (no account yet → nothing to browse).

use maud::{Markup, html};

/// The two-line offer pitch: a "limited time" label + the offer. Shown ABOVE the
/// create form AND kept at the top of the inline checkout card, so the offer
/// (and "limited time") does NOT vanish the moment the user starts checkout.
pub(crate) fn onboard_pitch() -> Markup {
    html! {
        // The label width is the NARROW measure of the chiasma — the CREATE
        // button matches it (`.create-button` in `.apex-onboard`); the offer line
        // is the WIDE measure the input field matches. Styling in `.onboard-pitch-*`.
        p.onboard-pitch-label { "limited time" }
        p.onboard-pitch-offer { "1 agent + 200 $LH for $2" }
    }
}

/// The header feedback (bug) glyph — a monochrome stroke insect icon in the
/// same hand-drawn style as the settings gear / notification bell (`fill=none`,
/// `currentColor`, stroke). Backs the dedicated header feedback button (sits
/// between the bell and the cog) that opens the on-chain feedback widget. ONE
/// source so the native landing replica and the wasm `site_header` stay identical.
pub(crate) fn bug_glyph() -> Markup {
    html! {
        (maud::PreEscaped(
            "<svg viewBox=\"0 0 24 24\" width=\"15\" height=\"15\" fill=\"none\" \
             stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" \
             stroke-linejoin=\"round\" aria-hidden=\"true\">\
             <path d=\"m8 2 1.88 1.88\"/><path d=\"M14.12 3.88 16 2\"/>\
             <path d=\"M9 7.13v-1a3.003 3.003 0 1 1 6 0v1\"/>\
             <path d=\"M12 20c-3.3 0-6-2.7-6-6v-3a4 4 0 0 1 4-4h4a4 4 0 0 1 4 4v3c0 3.3-2.7 6-6 6\"/>\
             <path d=\"M12 20v-9\"/><path d=\"M6.53 9C4.6 8.8 3 7.1 3 5\"/>\
             <path d=\"M6 13H2\"/><path d=\"M3 21c0-2.1 1.7-3.9 3.8-4\"/>\
             <path d=\"M20.97 5c0 2.1-1.6 3.8-3.5 4\"/><path d=\"M22 13h-4\"/>\
             <path d=\"M17.2 17c2.1.1 3.8 1.9 3.8 4\"/></svg>",
        ))
    }
}

/// The header settings (gear) glyph — a monochrome stroke icon in the same
/// hand-drawn style as the notification bell (`fill=none`, `currentColor`,
/// 1.3 stroke). Replaces the old "admin" text button. ONE source so the
/// native landing replica and the wasm `site_header` stay identical.
pub(crate) fn settings_glyph() -> Markup {
    html! {
        (maud::PreEscaped(
            "<svg viewBox=\"0 0 24 24\" width=\"15\" height=\"15\" fill=\"none\" \
             stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" \
             stroke-linejoin=\"round\" aria-hidden=\"true\">\
             <path d=\"M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z\"/>\
             <circle cx=\"12\" cy=\"12\" r=\"3\"/></svg>",
        ))
    }
}

/// Turn-status glyph — THINKING. A lucide "brain" in the same monochrome
/// stroke style as the bell/bug/gear. Painted into the header `#turn-status`
/// slot by `chat::stage` while the model is reasoning. ONE source so any
/// native replica and the wasm header stay identical.
pub(crate) fn brain_glyph() -> Markup {
    html! {
        (maud::PreEscaped(
            "<svg viewBox=\"0 0 24 24\" width=\"15\" height=\"15\" fill=\"none\" \
             stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" \
             stroke-linejoin=\"round\" aria-hidden=\"true\">\
             <path d=\"M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 1 0 12 18Z\"/>\
             <path d=\"M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 1 1 12 18Z\"/>\
             <path d=\"M15 13a4.5 4.5 0 0 1-3-4 4.5 4.5 0 0 1-3 4\"/>\
             <path d=\"M17.599 6.5a3 3 0 0 0 .399-1.375\"/>\
             <path d=\"M6.003 5.125A3 3 0 0 0 6.401 6.5\"/>\
             <path d=\"M3.477 10.896a4 4 0 0 1 .585-.396\"/>\
             <path d=\"M19.938 10.5a4 4 0 0 1 .585.396\"/>\
             <path d=\"M6 18a4 4 0 0 1-1.967-.516\"/>\
             <path d=\"M19.967 17.484A4 4 0 0 1 18 18\"/></svg>",
        ))
    }
}

/// Turn-status glyph — STREAMING. A lucide "waves" (flowing water) — final
/// answer text is flowing in. Same stroke envelope as [`brain_glyph`].
pub(crate) fn wave_glyph() -> Markup {
    html! {
        (maud::PreEscaped(
            "<svg viewBox=\"0 0 24 24\" width=\"15\" height=\"15\" fill=\"none\" \
             stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" \
             stroke-linejoin=\"round\" aria-hidden=\"true\">\
             <path d=\"M2 6c.6.5 1.2 1 2.5 1C7 7 7 5 9.5 5c2.5 0 2.5 2 5 2 2.6 0 2.4-2 5-2 1.3 0 1.9.5 2.5 1\"/>\
             <path d=\"M2 12c.6.5 1.2 1 2.5 1 2.5 0 2.5-2 5-2 2.6 0 2.4 2 5 2 2.5 0 2.5-2 5-2 1.3 0 1.9.5 2.5 1\"/>\
             <path d=\"M2 18c.6.5 1.2 1 2.5 1 2.5 0 2.5-2 5-2 2.6 0 2.4 2 5 2 2.5 0 2.5-2 5-2 1.3 0 1.9.5 2.5 1\"/></svg>",
        ))
    }
}

/// Turn-status glyph — TOOLS. A lucide "wrench" — a tool call is executing.
/// Same stroke envelope as [`brain_glyph`].
pub(crate) fn wrench_glyph() -> Markup {
    html! {
        (maud::PreEscaped(
            "<svg viewBox=\"0 0 24 24\" width=\"15\" height=\"15\" fill=\"none\" \
             stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" \
             stroke-linejoin=\"round\" aria-hidden=\"true\">\
             <path d=\"M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z\"/></svg>",
        ))
    }
}

/// The name-claim form — the SAME control on the fresh front door and the
/// authed apex (one component, no per-page divergence). `#apex-input` (live
/// availability check, wired in the delegated input handler) and `#create-btn`
/// (the `claim` button-state machinery) are load-bearing ids; only one form is
/// ever in the DOM at a time, so the shared ids never collide. `action` is the
/// submit `data-action` (`onboard-create` on the fresh door → create+pay+claim
/// in one go; `apex-claim` on the authed apex → claim only).
pub(crate) fn claim_name_form(action: &str) -> Markup {
    html! {
        form.create-form.claim-form data-action=(action) {
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
    }
}

/// The fresh-visitor front door: pick a name + CREATE in ONE step. Submitting
/// (`Action::OnboardCreate`) creates the identity, runs the $2 checkout, then —
/// once paid — claims the chosen name and drops the user straight into their
/// agent's chat. No separate post-payment name step, no second CREATE.
pub(crate) fn create_wallet_cta() -> Markup {
    html! {
        section #apex-onboard .apex-onboard {
            (onboard_pitch())
            (claim_name_form("onboard-create"))
            div #onboard-msg .step-msg {}
        }
    }
}

/// Shown on the apex front door INSTEAD of the create CTA when the visitor is on
/// iOS / iPadOS (detected in `templates::is_ios`). iOS Safari's OPFS writes stall
/// the single-threaded wasm app, so onboarding can't reliably complete there —
/// gate it off with an honest message rather than ship a broken flow. Same
/// `#apex-onboard` shell so the page layout is unchanged.
pub(crate) fn ios_unavailable() -> Markup {
    html! {
        section #apex-onboard .apex-onboard {
            p style="font-size:14px;margin:0" {
                "not available on iOS"
            }
        }
    }
}

/// The muted footer link(s) under the apex column. The home screen stays a
/// single front door — the public agent directory (`?explore=1`) is reachable
/// from the admin panel / direct link, not surfaced here (per request). Only
/// the agent-onboarding pointer remains.
pub(crate) fn apex_links(_fresh: bool) -> Markup {
    html! {
        nav.apex-links {
            a href="/skill.md" { "for agents →" }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use maud::DOCTYPE;

    /// Writes the apex fresh-visitor page to `target/landing-preview.html`
    /// for screenshot review (no browser profile / wasm build needed).
    ///
    /// Run: `cargo test --features browser-app landing_preview`
    /// then open `target/landing-preview.html` (file:// works — the
    /// stylesheet is linked relatively as `../web/styles.css`; the IBM
    /// Plex Mono link needs network, fallback is ui-monospace).
    #[test]
    fn landing_preview() {
        let page = html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport"
                        content="width=device-width,initial-scale=1";
                    link rel="preconnect" href="https://fonts.googleapis.com";
                    link rel="preconnect" href="https://fonts.gstatic.com"
                        crossorigin;
                    link rel="stylesheet"
                        href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&display=swap";
                    link rel="stylesheet" href="../web/styles.css";
                    title { "localharness — landing preview" }
                }
                body {
                    div #root {
                        // STATIC replica of `templates::site_header` (which
                        // is wasm-gated): brand + feedback (bug) + admin button,
                        // enough for a faithful screenshot. If the real header
                        // changes, refresh this replica.
                        header.site-header {
                            div.header-inner {
                                h1.header-brand { "localharness" }
                                div.header-admin {
                                    button type="button" aria-label="feedback"
                                        title="feedback"
                                        .header-button.feedback-bug-btn { (bug_glyph()) }
                                    button type="button" aria-label="settings"
                                        title="settings"
                                        .header-button.admin-button { (settings_glyph()) }
                                }
                            }
                        }
                        // The REAL fresh-apex content path (`templates::apex`
                        // with no wallet) — not a copy.
                        main.apex-main.apex-front {
                            div.col-chat {
                                div #status .terminal-status {}
                                (create_wallet_cta())
                            }
                        }
                        footer.apex-footer { (apex_links(true)) }
                    }
                }
            }
        };
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::create_dir_all(&dir).expect("create target/");
        let path = dir.join("landing-preview.html");
        std::fs::write(&path, page.into_string())
            .expect("write landing-preview.html");
        println!("wrote {}", path.display());
    }

    /// Writes the AUTHED apex (existing user: agents list + the shared claim
    /// form) to `target/authed-preview.html` to verify it shares the fresh
    /// door's centered layout + footer. The agents-list markup mirrors the
    /// wasm-gated `templates::agents_list` (which can't be called natively).
    ///
    /// Run: `cargo test --features browser-app authed_preview`
    #[test]
    fn authed_preview() {
        let agent_row = |name: &str, main: bool| {
            html! {
                li.agent-row {
                    a.agent-row-line href=(format!("https://{name}.localharness.xyz/")) {
                        span.agent-name { (name) }
                        span.agent-row-spacer {}
                        @if main { span.main-badge { "main" } }
                        @else { span.alt-badge { "alt" } }
                    }
                }
            }
        };
        let page = html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="utf-8";
                    meta name="viewport" content="width=device-width,initial-scale=1";
                    link rel="stylesheet"
                        href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&display=swap";
                    link rel="stylesheet" href="../web/styles.css";
                    title { "localharness — authed preview" }
                }
                body {
                    div #root {
                        header.site-header {
                            div.header-inner {
                                h1.header-brand { "localharness" }
                                div.header-admin {
                                    button type="button" aria-label="feedback"
                                        title="feedback"
                                        .header-button.feedback-bug-btn { (bug_glyph()) }
                                    button type="button" aria-label="settings"
                                        title="settings"
                                        .header-button.admin-button { (settings_glyph()) }
                                }
                            }
                        }
                        // Mirrors `templates::apex` authed branch: centered
                        // step-agents (list + the SAME claim form) + footer.
                        main.apex-main.apex-front {
                            div.col-chat {
                                div #status .terminal-status {}
                                section.step.step-agents {
                                    div #agents-list .agents-list {
                                        ul.agents-rows {
                                            (agent_row("krafto", true))
                                            (agent_row("console", false))
                                        }
                                    }
                                    (claim_name_form("apex-claim"))
                                    div #claim-fund-slot {}
                                }
                            }
                        }
                        footer.apex-footer { (apex_links(true)) }
                    }
                }
            }
        };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::create_dir_all(&dir).expect("create target/");
        let path = dir.join("authed-preview.html");
        std::fs::write(&path, page.into_string()).expect("write authed-preview.html");
        println!("wrote {}", path.display());
    }
}
