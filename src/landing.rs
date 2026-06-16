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
//! Funnel: INVITE/REDEEM-FIRST. A fresh visitor's ONLY action is redeeming
//! a code — an invite (`inv-…`, `?invite=` links prefill the input) or a
//! redeem code — which mints AND funds the wallet in one tap. There is no
//! standalone free "create" path (an unfunded identity stranded people, and
//! let 0-$LH visitors squat names). Seed import is the quiet
//! returning-device door inside the card; the explore directory is the
//! escape hatch for the uninvited.

use maud::{Markup, html};

/// The fresh-visitor front door: a single white-bordered card whose ONLY
/// action is redeeming a code. An invite (`inv-…`) or a redeem code mints
/// AND funds the wallet in one tap, so there is no unfunded-wallet path. No
/// wordmark or tagline — the site header already carries the brand. Seed
/// import is the quiet returning-device door; the `#import-slot` /
/// `#seed-msg` slots MUST exist for the ShowImport/ImportSeed DOM swaps to
/// land. Element ids + `data-action` are load-bearing
/// (`events/credits.rs::RedeemInviteOnboard`, `Action::ShowImport`) — keep
/// them stable.
///
/// `prefill` = an invite code captured from an `?invite=CODE` link.
pub(crate) fn invite_onboarding(prefill: Option<&str>) -> Markup {
    html! {
        section.apex-onboard {
            form.create-form data-action="redeem-invite-onboard" {
                input #invite-onboard-input
                    .create-input
                    type="text"
                    aria-label="invite or redeem code"
                    placeholder="invite or redeem code"
                    value=[prefill]
                    autocomplete="off"
                    spellcheck="false"
                    required {}
                button type="submit" .create-button { "redeem" }
            }
            div #invite-onboard-msg .step-msg {}
            div.apex-onboard-secondary {
                button type="button" data-action="show-import" .apex-onboard-import {
                    "import an existing seed"
                }
            }
            div #import-slot {}
            div #seed-msg .step-msg {}
        }
    }
}

/// The muted footer links under the apex column. Explore is the visible
/// escape hatch for visitors WITHOUT an invite; skill.md is the agent
/// front door. Shared by the fresh-visitor and returning-owner states.
pub(crate) fn apex_links() -> Markup {
    html! {
        nav.apex-links {
            a href="?explore=1" { "explore all agents →" }
            a href="/skill.md" { "for agents: how to join →" }
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
                                    button type="button"
                                        .header-button.admin-button { "admin" }
                                }
                            }
                        }
                        // The REAL fresh-apex content path (`templates::apex`
                        // with no wallet) — not a copy.
                        main.apex-main {
                            div.col-chat {
                                div #status .terminal-status {}
                                (invite_onboarding(None))
                                (apex_links())
                            }
                        }
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
