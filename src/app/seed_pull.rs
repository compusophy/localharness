//! Same-device cross-origin seed adoption — "local seed per origin".
//!
//! The master seed lives in the APEX origin's OPFS. Subdomains historically
//! reached it through a hidden cross-origin iframe (`?signer=1`) for every
//! seed-derived op (owner proof, key seal/open, tempo-tx signing). But
//! mobile browsers PARTITION cross-origin iframe storage: the apex iframe
//! embedded under `<name>.localharness.xyz` gets an empty, sandboxed OPFS,
//! so it never finds the seed and every op fails. The phone works on the
//! apex (first-party) but not on its own subdomains. That is the mobile
//! "subdomain is dead" bug.
//!
//! Fix: copy the seed into the subdomain origin's OWN OPFS so the iframe is
//! no longer needed there (see the local-first branches in `verify.rs`).
//! Transport is a **top-level apex round-trip** — each leg is a first-party
//! navigation, so it works on mobile where the iframe can't:
//!
//! 1. subdomain (no local seed) mints an ephemeral ECIES keypair, stashes
//!    the private half in sessionStorage, and navigates the top-level tab to
//!    `apex/?seed_export=1&to=<name>#epk=<ephemeral_pub>`.
//! 2. apex (first-party storage) reads its seed, confirms it actually owns
//!    `<name>` on-chain, ECIES-seals the mnemonic to `epk`, and navigates
//!    back to `<name>.localharness.xyz/?seed_import=1#s=<ct>`. With NO
//!    matching seed (every pure visitor) it goes `history.back()` instead:
//!    a bfcache restore resumes the subdomain's already-painted face with
//!    ZERO repaint, and a cache miss reloads the clean URL. Decision core:
//!    [`crate::seed_flow::none_bounce`]; the forward `?seed_import=none`
//!    nav survives only for a tab with nothing to go back to. `web/boot.js`
//!    short-circuits the definitive no-`.lh_wallet` case before the wasm
//!    even loads (parity guard: `tests/seed_pull_boot_parity.rs`).
//! 3. subdomain decrypts `s` with its stashed ephemeral key, imports the
//!    mnemonic into this origin's OPFS, and scrubs the URL. A stray
//!    `?seed_import=none` return (deploy skew / hand-typed) scrubs WITHOUT
//!    the import interstitial or an extra repaint
//!    ([`crate::seed_flow::should_repaint`], wired in `mount`).
//!
//! The sealed mnemonic rides a URL fragment (never sent to a server) and is
//! decryptable ONLY by the ephemeral key held in the subdomain's
//! sessionStorage — so the value in browser history is useless to anyone
//! else, and the apex hands nothing back unless it owns the name.

use crate::encoding::bytes_to_hex as hex;
use crate::wallet;

/// sessionStorage slot for the ephemeral ECIES private key (hex), held by
/// the subdomain across the round-trip.
const EPH_KEY: &str = "lh_seed_eph";
/// sessionStorage one-shot guard so a `seed_import=none` bounce (apex has
/// no matching seed) can't loop the tab back to apex forever.
const GUARD: &str = "lh_seed_pull_tried";

const APEX: &str = "https://localharness.xyz";

fn session() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.session_storage().ok().flatten())
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    crate::encoding::hex_to_bytes(s).ok()
}

/// Subdomain side. Kick the top-level round-trip to fetch the seed from
/// apex — UNCONDITIONAL (the caller decides when). Stashes an ephemeral
/// key, sets the one-shot guard, and navigates. Returns `true` if it
/// issued the navigation (the caller should stop painting).
pub(crate) async fn kick_export(name: &str) -> bool {
    let Some(storage) = session() else { return false };
    let (eph_pub, eph_signer) = wallet::ephemeral_keypair();
    let eph_priv_hex = hex(&eph_signer.to_bytes());
    if storage.set_item(EPH_KEY, &eph_priv_hex).is_err() {
        return false;
    }
    let _ = storage.set_item(GUARD, "1");
    let url = format!("{APEX}/?seed_export=1&to={name}#epk={}", hex(&eph_pub));
    if let Some(window) = web_sys::window() {
        return window.location().set_href(&url).is_ok();
    }
    false
}

/// Subdomain side. Kick the round-trip ONLY if it makes sense: this origin
/// has no local seed yet and we haven't already tried this tab session.
/// Returns `true` if it navigated.
pub(crate) async fn maybe_auto_kick(name: &str) -> bool {
    if super::wallet_store::load().await.is_some() {
        return false; // already have the seed locally
    }
    let Some(storage) = session() else { return false };
    if storage.get_item(GUARD).ok().flatten().is_some() {
        return false; // one attempt per tab — apex may legitimately have no seed
    }
    kick_export(name).await
}

/// Apex side (`?seed_export=1&to=<name>#epk=<hex>`). Seal the local seed to
/// the subdomain's ephemeral pubkey and navigate back — but ONLY if this
/// device's seed actually owns `<name>` on-chain. Otherwise (no seed, or a
/// visitor's unrelated identity) go BACK in history: the subdomain's
/// already-painted face restores from bfcache with zero repaint (or the
/// clean URL reloads on a cache miss), and its one-shot GUARD stops a
/// re-kick. The forward `?seed_import=none` nav is only the fallback for a
/// tab with no entry to go back to (`crate::seed_flow::none_bounce`).
pub(crate) async fn handle_apex_export() {
    let to = super::read_query_param("to")
        .map(|s| super::decode_uri_component(&s))
        .unwrap_or_default();
    let to_ok = !to.is_empty()
        && to.len() <= 63
        && to.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let epk_hex = super::read_fragment_param("epk").unwrap_or_default();
    if !to_ok || epk_hex.is_empty() {
        // Malformed — fall through to normal apex chrome rather than loop.
        super::paint_apex(super::tenant::Host::Apex).await;
        return;
    }

    let sealed_hex = seal_seed_for(&to, &epk_hex).await;
    let Some(window) = web_sys::window() else { return };
    match sealed_hex {
        Some(ct_hex) => {
            let url = format!("https://{to}.localharness.xyz/?seed_import=1#s={ct_hex}");
            let _ = window.location().set_href(&url);
        }
        None => {
            let history_len = window
                .history()
                .ok()
                .and_then(|h| h.length().ok())
                .unwrap_or(1);
            match crate::seed_flow::none_bounce(history_len) {
                crate::seed_flow::NoneBounce::Back => {
                    if let Ok(history) = window.history() {
                        if history.back().is_ok() {
                            return;
                        }
                    }
                    // back() itself failed — fall through to the forward nav.
                    let url = format!("https://{to}.localharness.xyz/?seed_import=none");
                    let _ = window.location().set_href(&url);
                }
                crate::seed_flow::NoneBounce::ForwardNone => {
                    let url = format!("https://{to}.localharness.xyz/?seed_import=none");
                    let _ = window.location().set_href(&url);
                }
            }
        }
    }
}

/// Apex helper: returns the ECIES-sealed mnemonic hex for `to` IF this
/// device holds the seed AND that seed owns `to` on-chain; else `None`.
async fn seal_seed_for(to: &str, epk_hex: &str) -> Option<String> {
    let epk = unhex(epk_hex)?;
    let wallet = super::wallet_store::load().await?;
    // Only ever hand over the seed for a name THIS seed owns. A visitor's
    // apex (different identity) returns None here → harmless `none` bounce.
    let owner = super::registry::owner_of_name(to).await.ok().flatten()?;
    if !owner.eq_ignore_ascii_case(&wallet.address_hex()) {
        return None;
    }
    let mnemonic = wallet.mnemonic.to_string();
    let ct = super::encryption::ecies_seal(&epk, mnemonic.as_bytes()).await?;
    Some(hex(&ct))
}

/// Subdomain side of a PAYLOAD-BEARING return (`?seed_import=1#s=<ct>` —
/// `mount` routes only `crate::seed_flow::should_repaint` legs here; an
/// empty `none` return takes [`finish_none_return`] instead). Imports the
/// seed into THIS origin's OPFS, scrubs the URL + clears the ephemeral key.
/// Returns `true` iff a seed was imported (caller repaints with a local
/// wallet). Tolerates a junk mode defensively (no import, same scrub).
pub(crate) async fn handle_tenant_import() -> bool {
    let mode = super::read_query_param("seed_import").unwrap_or_default();
    let imported = if mode == "1" { try_import().await } else { false };
    if let Some(storage) = session() {
        let _ = storage.remove_item(EPH_KEY);
    }
    scrub_url();
    imported
}

/// Subdomain side of an EMPTY return leg (`?seed_import=none`, or a
/// payload-less `1`): nothing to import, so do NOT touch the paint flow —
/// just drop the ephemeral key and scrub the URL (history.replaceState),
/// synchronously, and let the caller fall through to the ONE normal paint.
pub(crate) fn finish_none_return() {
    if let Some(storage) = session() {
        let _ = storage.remove_item(EPH_KEY);
    }
    scrub_url();
}

async fn try_import() -> bool {
    let Some(ct_hex) = super::read_fragment_param("s") else { return false };
    let Some(ct) = unhex(&ct_hex) else { return false };
    let Some(storage) = session() else { return false };
    let Some(eph_hex) = storage.get_item(EPH_KEY).ok().flatten() else { return false };
    let Ok(eph_signer) = wallet::from_private_key_hex(eph_hex.trim()) else { return false };
    let Some(pt) = super::encryption::ecies_open(&eph_signer, &ct).await else { return false };
    let Ok(phrase) = String::from_utf8(pt) else { return false };
    super::wallet_store::import(phrase.trim()).await.is_ok()
}

/// Drop the import params + fragment from the URL so a refresh can't
/// replay them (the fragment carried the sealed seed).
fn scrub_url() {
    let Some(window) = web_sys::window() else { return };
    let Ok(history) = window.history() else { return };
    let path = window.location().pathname().unwrap_or_else(|_| "/".to_string());
    let _ = history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&path));
}
