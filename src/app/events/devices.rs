//! Devices — QR seed-adoption (Option A), P2P device sync, signer list,
//! and unlink.

use crate::encoding::short_addr;

use crate::app::{dom, templates};

pub(super) async fn refresh_signer_list() {
    let addr = crate::app::APP.with(|cell| {
        cell.borrow().wallet.as_ref().map(|w| w.address_hex())
    });
    // On a linked device (no local master wallet) fall back to the
    // linked-owner pointer, so the on-chain device list shows the same
    // global state the seed-holding device sees.
    let addr = match addr {
        Some(a) => a,
        None => match crate::app::wallet_store::load_linked_owner().await {
            Some(o) => o,
            None => return,
        },
    };

    let main_id = match crate::app::registry::main_of(&addr).await {
        Ok(id) if id > 0 => id,
        _ => {
            dom::swap_inner("signer-list", "no MAIN set");
            return;
        }
    };

    // Read the on-chain enumerable index in ONE call — no log scraping.
    match crate::app::registry::devices_of(main_id).await {
        Ok(signers) if signers.is_empty() => {
            dom::swap_inner("signer-list", "owner only (no linked devices)");
        }
        Ok(signers) => {
            // Device addresses come back from an RPC node; maud escapes both
            // the displayed `short` and the `data-arg` so a hostile node can't
            // inject markup (the `data-arg` lands back in the click dispatcher).
            let html = maud::html! {
                @for s in &signers {
                    @let short = if s.len() > 10 {
                        format!("{}…{}", &s[..6], &s[s.len()-4..])
                    } else {
                        s.clone()
                    };
                    div style="display:flex;justify-content:center;align-items:center;gap:8px;color:var(--fg);font-size:11px;margin:2px 0" {
                        code { (short) }
                        button type="button" class="modal-close" data-action="unlink-device"
                            data-arg=(s) title="unlink" { "×" }
                    }
                }
            }
            .into_string();
            dom::swap_inner("signer-list", &html);
        }
        Err(_) => {
            dom::swap_inner("signer-list", "");
        }
    }
}

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

/// 6-char one-time pairing code (Crockford-ish base32, no ambiguous
/// chars) from the browser CSPRNG. Short enough to read aloud / type.
fn generate_pair_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut bytes = [0u8; 6];
    let _ = getrandom::getrandom(&mut bytes);
    bytes
        .iter()
        .map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char)
        .collect()
}

/// Resolve (local owner signer, owner hex, MAIN id, MAIN TBA). The
/// preamble for unlink, which acts on the MAIN's TBA.
async fn owner_main_tba() -> Result<(k256::ecdsa::SigningKey, String, u64, String), String> {
    let (signer, owner_hex) = crate::app::APP
        .with(|c| {
            c.borrow()
                .wallet
                .as_ref()
                .map(|w| (w.signer.clone(), w.address_hex()))
        })
        .ok_or_else(|| "no identity".to_string())?;
    let main_id = crate::app::registry::main_of(&owner_hex)
        .await
        .map_err(|e| format!("mainOf: {e}"))?;
    if main_id == 0 {
        return Err("set a MAIN first".into());
    }
    let main_name = crate::app::registry::name_of_id(main_id)
        .await
        .map_err(|e| format!("name: {e}"))?;
    let main_tba = crate::app::registry::tba_of_name(&main_name)
        .await
        .map_err(|e| format!("tba: {e}"))?
        .ok_or_else(|| "no MAIN TBA".to_string())?;
    Ok((signer, owner_hex, main_id, main_tba))
}

/// The X on a linked device. Removing a device's access is destructive
/// (revokes its signer authority + costs a sponsored tx + a re-pair to
/// undo), so a single accidental click must NOT do it — show a typed
/// confirmation in `#pair-msg` first. (Unlinking affects only THAT device;
/// the owner / other devices keep their access.)
pub(super) fn unlink_device_prompt(device_hex: String) {
    let short = short_addr(&device_hex);
    // Remember the trigger (the × on the device row) BEFORE swapping in the
    // panel, so closing returns focus there.
    dom::remember_focus();
    // `data-modal-trap`/`data-modal-cancel` make the delegated keydown listener
    // confine Tab to this panel and route Escape to cancel (a11y #75).
    dom::swap_inner(
        "pair-msg",
        &format!(
            "<div id=\"unlink-confirm-panel\" class=\"unlink-confirm\" role=\"dialog\" \
               aria-modal=\"true\" data-modal-trap data-modal-cancel=\"unlink-cancel\">\
               <div>remove <code>{short}</code>? type <b>yes</b> to confirm.</div>\
               <input id=\"unlink-confirm-input\" type=\"text\" autocomplete=\"off\" \
                 placeholder=\"yes\">\
               <div class=\"pair-confirm-actions\">\
                 <button type=\"button\" class=\"ghost\" data-action=\"unlink-cancel\">cancel</button>\
                 <button type=\"button\" class=\"button-link\" data-action=\"unlink-confirm\" \
                   data-arg=\"{device_hex}\">remove</button>\
               </div>\
             </div>"
        ),
    );
    // Pull focus INTO the armed panel (lands on the typed-confirm input).
    dom::focus_first_in("unlink-confirm-panel");
}

/// Abort an in-progress unlink — clear the confirmation prompt and return
/// focus to the trigger (a11y #75; also the Escape target).
pub(super) fn unlink_cancel_pressed() {
    dom::swap_inner("pair-msg", "");
    dom::restore_focus();
}

/// Only unlink when the user typed `yes` in the confirmation input.
pub(super) fn unlink_confirm_pressed(device_hex: String) {
    let typed = dom::input_by_id("unlink-confirm-input")
        .map(|i| i.value().trim().to_lowercase())
        .unwrap_or_default();
    if typed != "yes" {
        dom::swap_inner(
            "pair-msg",
            &dom::msg_span(dom::Msg::Error, "type yes to remove that device"),
        );
        return;
    }
    dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Accent, "removing…"));
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            let (signer, _owner, main_id, main_tba) = owner_main_tba().await?;
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::remove_signer_sponsored(
                &signer,
                &fee_payer,
                main_id,
                &main_tba,
                &device_hex,
                crate::app::registry::ALPHA_USD_ADDRESS(),
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                dom::swap_inner("pair-msg", "");
                // Panel gone — return focus to the trigger (a no-op if that
                // device row was just removed from the list).
                dom::restore_focus();
                refresh_signer_list().await
            }
            Err(e) => {
                dom::swap_inner(
                    "pair-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("unlink failed: {e}")),
                );
            }
        }
    });
}
