//! Devices — QR seed-adoption (Option A) and P2P device sync.

use crate::app::{dom, templates};

/// Dismiss the seed-adoption QR panel — swap it back to the button.
pub(super) fn pair_cancel_pressed() {
    dom::swap_outer(
        "pair-slot",
        r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="add-device" class="ghost">add a device</button></div>"#,
    );
    dom::swap_inner("pair-msg", "");
}

/// Derive a 32-byte transport key from a one-time pairing code — the
/// canonical [`crate::wallet::adopt_code_key`] (tag `localharness/v0/adopt`),
/// shared with the `localharness link` CLI so a seed sealed here decrypts
/// there byte-for-byte. The desktop `seal_with_raw_key`s under it; the phone
/// (or the CLI) `open_with_raw_key`s with the same key from the typed code.
fn code_key(code: &str) -> [u8; 32] {
    crate::wallet::adopt_code_key(code)
}

/// Thin wrapper over [`crate::encoding::hex_to_bytes`]: the adopt-link
/// ciphertext must be non-empty (an empty fragment means a mangled QR link).
fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    crate::encoding::hex_to_bytes(s).ok().filter(|v| !v.is_empty())
}

/// "Sync my devices" — run one P2P collaboration pass: announce this device,
/// discover the owner's other online devices via the on-chain signaling roster,
/// connect over WebRTC, and union-sync the shared folder. Best-effort; status
/// lands in `#pair-msg`. (Needs the SignalingFacet cut + a second device online.)
pub(super) fn run_sync_devices() {
    dom::swap_inner(
        "pair-msg",
        "<span style=\"color:var(--muted)\">discovering devices…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let msg = match crate::app::teams_sync::sync_my_devices().await {
            Ok(0) => {
                "no other devices online — open this agent on another device and sync there too"
                    .to_string()
            }
            Ok(n) => format!("connected — syncing with {n} device(s)"),
            Err(e) => format!("sync failed: {e}"),
        };
        // `msg` can carry a sync/network error string (`sync failed: {e}`),
        // so escape it via maud rather than interpolating raw HTML.
        dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Muted, &msg));
    });
}

/// Desktop side of Option A "add a device". Encrypt this device's seed
/// under a one-time code and render a QR of an apex URL whose FRAGMENT
/// carries the ciphertext (the fragment never leaves the browser / is
/// never sent to a server). The user reads the code off-screen and types
/// it on the other device to decrypt + import — no on-chain pairing, no
/// device keys, no redirect glue.
pub(super) fn add_device_pressed() {
    let phrase = crate::app::APP
        .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.mnemonic.to_string()));
    let Some(phrase) = phrase else {
        dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, "no identity on this device"));
        return;
    };
    wasm_bindgen_futures::spawn_local(async move {
        let code = generate_pair_code();
        let Some(ct) = crate::app::encryption::seal_with_raw_key(&code_key(&code), phrase.as_bytes()).await
        else {
            dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, "encrypt failed"));
            return;
        };
        let hex = crate::encoding::bytes_to_hex(&ct);
        let url = format!("https://localharness.xyz/?adopt=1#s={hex}");
        dom::swap_outer("pair-slot", &templates::adopt_panel(&code, &url).into_string());
        dom::swap_inner("pair-msg", "");
    });
}

/// Phone side of Option A "add a device". Read the one-time code the user
/// typed + the ciphertext stashed in the hidden input (from the URL
/// fragment), decrypt, and import the seed — this device now IS the same
/// identity and owns every subdomain it holds. A full reload lands on the
/// clean apex with the wallet persisted.
pub(super) fn adopt_device_pressed() {
    let code = dom::input_by_id("adopt-code").map(|i| i.value()).unwrap_or_default();
    let ct_hex = dom::input_by_id("adopt-ct").map(|i| i.value()).unwrap_or_default();
    if code.trim().is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(async move {
        let Some(ct) = hex_to_bytes(&ct_hex) else {
            dom::swap_inner("adopt-msg", &dom::msg_span(dom::Msg::Error, "bad link — rescan the QR"));
            return;
        };
        match crate::app::encryption::open_with_raw_key(&code_key(&code), &ct).await {
            Some(bytes) => {
                let phrase = String::from_utf8_lossy(&bytes).into_owned();
                match crate::app::wallet_store::import(phrase.trim()).await {
                    Ok(_) => {
                        if let Ok(window) = dom::window() {
                            let _ = window.location().set_href("https://localharness.xyz/");
                        }
                    }
                    Err(err) => {
                        dom::swap_inner("adopt-msg", &dom::msg_span(dom::Msg::Error, &format!("import failed: {err}")));
                    }
                }
            }
            None => {
                dom::swap_inner("adopt-msg", &dom::msg_span(dom::Msg::Error, "wrong code"));
            }
        }
    });
}

/// 8-char one-time pairing code (Crockford-ish base32, no ambiguous chars) from
/// the browser CSPRNG — ~40 bits of entropy (32^8). The alphabet is 32 symbols and
/// 256 % 32 == 0, so the `% 32` map is unbiased (no modulo skew). Bumped from 6
/// (~30 bits) so the sealed-seed ciphertext can't be cheaply brute-forced offline
/// even if it leaks (audit H1); still short enough to read aloud / type.
fn generate_pair_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut bytes = [0u8; 8];
    let _ = getrandom::getrandom(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}
