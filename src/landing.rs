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
        p style="font-size:11px;letter-spacing:0.08em;text-transform:uppercase;color:var(--muted);margin:0 0 var(--space-1)" {
            "limited time"
        }
        p style="font-size:14px;margin:0 0 var(--chrome-pad)" {
            "1 agent + 200 $LH for $2"
        }
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

/// The name-claim form — the SAME control on the fresh front door and the
/// authed apex (one component, no per-page divergence). `#apex-input` (live
/// availability check, wired in the delegated input handler) and `#create-btn`
/// (the `claim` button-state machinery) are load-bearing ids; only one form is
/// ever in the DOM at a time, so the shared ids never collide. `action` is the
/// submit `data-action` (`onboard-create` on the fresh door → create+pay+claim
/// in one go; `apex-claim` on the authed apex → claim only).
pub(crate) fn claim_name_form(action: &str) -> Markup {
    html! {
        form.create-form data-action=(action) {
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
                        // is wasm-gated): brand + admin button, enough for a
                        // faithful screenshot. If the real header changes,
                        // refresh this replica.
                        header.site-header {
                            div.header-inner {
                                h1.header-brand { "localharness" }
                                div.header-admin {
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
}
