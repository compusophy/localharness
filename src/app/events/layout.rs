//! Reset + pricing + display-mode toggles — the typed-confirmation reset,
//! pricing save, and the live light-theme / mobile-preview switches.
//! (The old panel-collapse class toggles died with the tabbed layout —
//! the unified stream has no side panels to collapse.)

use crate::app::{dom, templates};

/// Flip the light theme live (`html.theme-light`) and persist the choice.
pub(super) fn toggle_theme() {
    set_render_mode("theme-light", "lh-theme", "light", "dark");
}

/// Toggle the desktop view on/off live. The app is mobile-first — framed as a
/// 9:16 phone column by default on desktop (`apply_render_modes`) — so this
/// REMOVES the `preview-mobile` frame (persisting `lh-preview=desktop`) and adds
/// it back (persisting `mobile`). Real phones are never framed regardless.
pub(super) fn toggle_preview() {
    set_render_mode("preview-mobile", "lh-preview", "mobile", "desktop");
}

/// Flip a render-mode class on `<html>`, persist the pref in `localStorage`,
/// then re-render `#display-toggles` so the toggles reflect the new state. No
/// reload — the token block (`style.rs`) + `styles.css` react to the class
/// instantly. Mirrored at mount by `mod::apply_render_modes`.
fn set_render_mode(class: &str, key: &str, on_val: &str, off_val: &str) {
    let Some(win) = web_sys::window() else { return };
    let Some(html) = win.document().and_then(|d| d.document_element()) else {
        return;
    };
    let list = html.class_list();
    let next_on = !list.contains(class);
    if next_on {
        let _ = list.add_1(class);
    } else {
        let _ = list.remove_1(class);
    }
    if let Ok(Some(storage)) = win.local_storage() {
        let _ = storage.set_item(key, if next_on { on_val } else { off_val });
    }
    dom::swap_outer(
        "display-toggles",
        &templates::display_toggles().into_string(),
    );
}

/// Inline-confirmed reset: FULL wipe of OPFS root (seed included), then reload
/// back to the fresh "create agent" stage. Destroys the identity — gated by the
/// typed "RESET" + the panel's back-up-your-seed warning.
/// Replaces the old `window.confirm()` flow per [[feedback-no-js-alerts]].
pub(super) fn reset_confirm_pressed() {
    // Typed confirmation — reset still clears app data/keys, so require the
    // literal word, not just a second click. (It no longer touches the seed.)
    let typed = dom::input_by_id("reset-confirm-text")
        .map(|i| i.value().trim().to_string())
        .unwrap_or_default();
    if !typed.eq_ignore_ascii_case("RESET") {
        dom::swap_inner(
            "reset-confirm-msg",
            "<span style=\"color:var(--error)\">type RESET to confirm</span>",
        );
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let fs = crate::app::shared_opfs();
        if let Ok(entries) = fs.read_dir("").await {
            for entry in entries {
                // FULL wipe — INCLUDING the seed (`.lh_wallet`) + owner hint
                // (`.lh_owner`), so reset returns to the fresh "create agent"
                // stage (the whole point of a reset on a test/second device).
                // The typed-"RESET" gate + the panel's identity-loss warning are
                // the deliberate-action safeguard against the old brick — reveal
                // and back up your seed first if you want to keep this identity.
                let _ = fs.delete(&entry.name).await;
            }
        }
        if let Ok(window) = dom::window() {
            let _ = window.location().reload();
        }
    });
}

/// Parse the pricing-input as a decimal test-ETH amount, convert to
/// wei, persist via `pricing::save`, and re-paint the card so the
/// new value shows. Owner-only — the input is only rendered when
/// the verifier confirmed this visitor is the owner — but we still
/// re-check `verify_state` here as belt-and-suspenders against a
/// stale DOM.
pub(super) fn pricing_save_pressed() {
    let Some(input) = dom::input_by_id("pricing-input") else {
        return;
    };
    let raw = input.value().trim().to_string();
    let wei = match parse_eth_to_wei(&raw) {
        Ok(w) => w,
        Err(err) => {
            dom::swap_inner(
                "pricing-msg",
                &dom::msg_span(dom::Msg::Error, &err.to_string()),
            );
            return;
        }
    };

    let is_owner = crate::app::APP.with(|cell| {
        matches!(cell.borrow().verify_state, crate::app::VerifyState::Verified { .. })
    });
    if !is_owner {
        dom::swap_inner(
            "pricing-msg",
            "<span style=\"color:var(--error)\">only the verified owner can change pricing</span>",
        );
        return;
    }

    dom::swap_inner(
        "pricing-msg",
        "<span style=\"color:var(--muted)\">saving…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match crate::app::pricing::save(wei).await {
            Ok(()) => {
                crate::app::APP
                    .with(|cell| cell.borrow_mut().pricing_wei = Some(wei));
                let html = templates::pricing_card_body(wei, true).into_string();
                dom::swap_outer("pricing-body", &html);
            }
            Err(err) => {
                dom::swap_inner(
                    "pricing-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("save failed: {err}")),
                );
            }
        }
    });
}

/// Parse a decimal test-ETH amount ("0", "0.001", "1.5") into a wei
/// `u128`. Rejects negatives, NaN-shaped input, and values with more
/// than 18 fractional digits (wei is the precision floor).
fn parse_eth_to_wei(s: &str) -> Result<u128, String> {
    if s.is_empty() {
        return Ok(0);
    }
    let (whole_str, frac_str) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if !whole_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err("price must be a positive decimal".into());
    }
    if !frac_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err("price must be a positive decimal".into());
    }
    if frac_str.len() > 18 {
        return Err("price has more precision than wei (18 decimals max)".into());
    }
    let whole: u128 = whole_str.parse().map_err(|e| format!("whole: {e}"))?;
    // Right-pad fraction to 18 digits then parse.
    let mut padded = String::with_capacity(18);
    padded.push_str(frac_str);
    while padded.len() < 18 {
        padded.push('0');
    }
    let frac: u128 = if padded.is_empty() {
        0
    } else {
        padded.parse().map_err(|e| format!("frac: {e}"))?
    };
    whole
        .checked_mul(1_000_000_000_000_000_000)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| "price too large".into())
}
