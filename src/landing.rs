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

/// The fresh-visitor front door: ONE white-bordered card with a single
/// `create wallet` CTA. It's the paid entry — wired to a checkout session
/// that creates AND funds the wallet — so a 0-$LH visitor never exists. No
/// code input or seed-import here (both live in admin; `?invite=` links
/// auto-redeem on mount). `data-action="create-wallet"` is load-bearing.
pub(crate) fn create_wallet_cta() -> Markup {
    html! {
        section.apex-onboard {
            button type="button" data-action="create-wallet" .apex-onboard-cta {
                "create wallet"
            }
            div #onboard-msg .step-msg {}
        }
    }
}

/// The muted footer link(s) under the apex column. A fresh visitor (no
/// account) sees only the agent-onboarding door; explore is added once an
/// identity exists — nothing to browse before you're in.
pub(crate) fn apex_links(fresh: bool) -> Markup {
    html! {
        nav.apex-links {
            @if !fresh {
                a href="?explore=1" { "explore all agents →" }
            }
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
                                (create_wallet_cta())
                                (apex_links(true))
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
