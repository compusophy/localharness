//! Devices — pairing, QR seed-adoption, P2P device sync, signer list,
//! consolidate, and unlink.

use wasm_bindgen::prelude::*;

use crate::encoding::{short_addr, tx_short_hash};

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

/// A device that announced with the matching code, awaiting the owner's
/// explicit approval before enrollment (see `pair_start_pressed`).
struct PendingPair {
    device: String,
    device_pubkey: Vec<u8>,
    token_id: u64,
    owner_hex: String,
}

thread_local! {
    static PENDING_PAIR: std::cell::RefCell<Option<PendingPair>> =
        const { std::cell::RefCell::new(None) };
}

/// Desktop side of device pairing. Generate a one-time code, show it +
/// the deep link to open on the other device, then poll the on-chain
/// pairing rendezvous. When the phone announces (its fresh device key as
/// `msg.sender` of a sponsored tx), we learn its address from the log
/// and — after the owner confirms it matches — enroll it via `addSigner`.
/// No 0x ever copied between machines.
pub(super) fn pair_start_pressed() {
    // The user's MAIN name is what the phone will open (its own
    // subdomain). Resolve it from the apex wallet's MAIN.
    wasm_bindgen_futures::spawn_local(async move {
        let owner_hex = crate::app::APP
            .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.address_hex()));
        let Some(owner_hex) = owner_hex else {
            dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, "no identity"));
            return;
        };
        let token_id = match crate::app::registry::main_of(&owner_hex).await {
            Ok(id) if id != 0 => id,
            _ => {
                dom::swap_inner(
                    "pair-msg",
                    &dom::msg_span(dom::Msg::Error, "claim a subdomain first"),
                );
                return;
            }
        };
        let name = match crate::app::registry::name_of_id(token_id).await {
            Ok(n) if !n.is_empty() => n,
            _ => {
                dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, "no MAIN name"));
                return;
            }
        };

        // One-time code: 6 uppercase base32-ish chars from CSPRNG.
        let code = generate_pair_code();
        let pair_url = format!("https://{name}.localharness.xyz/?pair={code}");
        dom::swap_outer(
            "pair-slot",
            &templates::pair_panel(&code, &pair_url).into_string(),
        );
        dom::swap_inner("pair-msg", "");

        // Poll the rendezvous: ~5 min at 3s intervals.
        let code_hash = crate::app::registry::pairing_code_hash(&code);
        let mut found: Option<(String, Vec<u8>)> = None;
        for _ in 0..100 {
            // Stop if the user cancelled (panel swapped away).
            if dom::by_id("pair-slot")
                .map(|el| !el.class_name().contains("pair-active"))
                .unwrap_or(true)
            {
                return;
            }
            match crate::app::registry::find_pairing_device(&code_hash).await {
                Ok(Some(pair)) => {
                    found = Some(pair);
                    break;
                }
                _ => {}
            }
            crate::runtime::sleep_ms(3000).await;
        }

        let Some((device, device_pubkey)) = found else {
            dom::swap_inner(
                "pair-msg",
                &dom::msg_span(dom::Msg::Error, "timed out — try again"),
            );
            dom::swap_outer(
                "pair-slot",
                &r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="add-device" class="ghost">add a device</button></div>"#,
            );
            return;
        };

        // SECURITY: do NOT auto-enroll. A device matching the code hash has
        // announced, but granting it signer control over the MAIN must be an
        // explicit owner decision — so the user can compare the address shown
        // here against the one on the device they're holding (out-of-band
        // verification on top of the code + the ~5-min window). Stash the
        // pending device and ask; enrollment happens in `pair_approve_pressed`.
        PENDING_PAIR.with(|p| {
            *p.borrow_mut() = Some(PendingPair {
                device: device.clone(),
                device_pubkey,
                token_id,
                owner_hex,
            });
        });
        dom::swap_outer(
            "pair-slot",
            &templates::pair_confirm_panel(&device).into_string(),
        );
        dom::swap_inner("pair-msg", "");
    });
}

/// Owner approved the announced device — enroll it as a signer on the
/// MAIN's TBA (+ share the ECIES-wrapped Gemini key). The deliberate
/// confirmation is the out-of-band check that the address matches the
/// device the user is actually holding.
pub(super) fn pair_approve_pressed() {
    let Some(pending) = PENDING_PAIR.with(|p| p.borrow_mut().take()) else {
        return;
    };
    dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Accent, "enrolling…"));
    wasm_bindgen_futures::spawn_local(async move {
        let PendingPair { device, device_pubkey, token_id, owner_hex } = pending;
        match run_add_device(device.clone()).await {
            Ok(tx_hash) => {
                let short = tx_short_hash(&tx_hash);
                dom::swap_inner(
                    "pair-msg",
                    &dom::msg_span(
                        dom::Msg::Accent,
                        &format!("✓ linked {} (tx {short})", short_addr(&device)),
                    ),
                );
                dom::swap_outer(
                    "pair-slot",
                    &r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="pair-start" class="ghost">link another device</button></div>"#,
                );
                // ECIES-wrap the MAIN Gemini key to this device's pubkey and
                // post it on-chain, so the phone gets the key WITHOUT ever
                // importing the seed. Best-effort.
                if !device_pubkey.is_empty() {
                    if wrap_and_post_key_to_device(token_id, &owner_hex, &device, &device_pubkey)
                        .await
                        .is_ok()
                    {
                        dom::swap_inner(
                            "pair-msg",
                            &dom::msg_span(
                                dom::Msg::Accent,
                                &format!(
                                    "✓ linked {} + shared your key — it's ready to use",
                                    short_addr(&device)
                                ),
                            ),
                        );
                    }
                }
                refresh_signer_list().await;
            }
            Err(err) => {
                dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, &format!("{err}")));
            }
        }
    });
}

/// Owner rejected the announced device — discard it and reset the panel.
pub(super) fn pair_reject_pressed() {
    PENDING_PAIR.with(|p| *p.borrow_mut() = None);
    dom::swap_outer(
        "pair-slot",
        &r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="add-device" class="ghost">add a device</button></div>"#,
    );
    dom::swap_inner("pair-msg", &dom::msg_span(dom::Msg::Error, "rejected that device"));
}

/// Cancel an in-progress pairing — swap the panel back to the button.
/// The poll loop notices the missing `.pair-active` class and exits.
pub(super) fn pair_cancel_pressed() {
    dom::swap_outer(
        "pair-slot",
        &r#"<div id="pair-slot" class="pair-slot"><button id="pair-btn" type="button" data-action="add-device" class="ghost">add a device</button></div>"#,
    );
    dom::swap_inner("pair-msg", "");
}

/// Derive a 32-byte transport key from a one-time pairing code. Keccak256
/// of the uppercased code — deterministic on both devices, so the desktop
/// can `seal_with_raw_key` and the phone can `open_with_raw_key`.
fn code_key(code: &str) -> [u8; 32] {
    use sha3::{Digest, Keccak256};
    let mut h = Keccak256::new();
    h.update(b"localharness/v0/adopt");
    h.update(code.trim().to_uppercase().as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// Thin wrapper over [`crate::encoding::hex_to_bytes`]: the adopt-link
/// ciphertext must be non-empty (an empty fragment means a mangled QR link).
fn hex_to_bytes(s: &str) -> Option<Vec<u8>> {
    crate::encoding::hex_to_bytes(s).ok().filter(|v| !v.is_empty())
}

/// Desktop side of Option A "add a device". Encrypt this device's seed
/// under a one-time code and render a QR of an apex URL whose FRAGMENT
/// carries the ciphertext (the fragment never leaves the browser / is
/// never sent to a server). The user reads the code off-screen and types
/// it on the other device to decrypt + import — no on-chain pairing, no
/// device keys, no redirect glue.
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

/// Phone side. The device opened `<name>.localharness.xyz/?pair=CODE`.
/// Generate a fresh device keypair, persist it as this origin's wallet,
/// and announce on-chain (sponsored) so the desktop can enroll it.
pub(crate) fn pair_join_pressed(code: String) {
    dom::swap_inner(
        "pair-join-msg",
        "<span style=\"color:var(--muted)\">generating device key + announcing…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        match run_pair_join(&code).await {
            Ok(addr) => {
                dom::swap_inner(
                    "pair-join-msg",
                    &dom::msg_span(
                        dom::Msg::Accent,
                        &format!(
                            "✓ this device is {addr} — approve it on your other \
                             device (check the address matches), and you'll be \
                             redirected automatically."
                        ),
                    ),
                );
            }
            Err(err) => {
                dom::swap_inner(
                    "pair-join-msg",
                    &dom::msg_span(dom::Msg::Error, &format!("{err}")),
                );
            }
        }
    });
}

/// Generate the device key, persist it to this origin's OPFS so the
/// device can keep acting as a signer, and announce `keccak(code)`
/// on-chain via a sponsored tx.
async fn run_pair_join(code: &str) -> Result<String, String> {
    // A fresh device identity for THIS subdomain origin. Persist it so
    // future visits reuse the same enrolled key (the apex flow stores a
    // mnemonic; here a per-device random key is enough — it's a signer,
    // not the master seed).
    let wallet = crate::wallet::generate();
    let device_hex = wallet.address_hex();
    crate::app::wallet_store::persist_device_key(&wallet.private_key_hex)
        .await
        .map_err(|e| format!("save device key: {e}"))?;

    let code_hash = crate::app::registry::pairing_code_hash(code);
    // Announce our compressed pubkey so the desktop can ECIES-wrap the
    // Gemini key directly to us — we never need the master seed.
    let pubkey = crate::wallet::pubkey_compressed(&wallet.signer);
    let fee_payer = crate::app::sponsor::signer()?;
    crate::app::registry::announce_pairing_sponsored(
        &wallet.signer,
        &fee_payer,
        &code_hash,
        &pubkey,
        crate::app::registry::ALPHA_USD_ADDRESS,
    )
    .await?;

    // Background: poll for the desktop's ECIES-wrapped Gemini key, decrypt
    // it with our device key, and save it locally — so this device never
    // prompts for an API key. Best-effort; the device still works as a
    // signer even if the desktop has no key to share.
    let device_signer = wallet.signer.clone();
    let device_addr = device_hex.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let name = match crate::app::tenant::current() {
            crate::app::tenant::Host::Tenant(n) => n,
            _ => return,
        };
        // The identity (MAIN) this subdomain belongs to — so we can detect
        // when the desktop enrolls us into its on-chain device index, and so
        // we can hand the apex a pointer to this owner on redirect.
        let (main_id, owner_hex) = match crate::app::registry::owner_of_name(&name).await {
            Ok(Some(owner)) => (crate::app::registry::main_of(&owner).await.unwrap_or(0), owner),
            _ => (0, String::new()),
        };
        let slot_id = super::key_sync::gemini_key_slot_id(&name).await.ok();
        let mut enrolled = false;
        let mut got_key = false;
        let mut post_enroll = 0u32;
        for _ in 0..60 {
            // 1) Enrollment confirmation: are we in the MAIN's device index
            //    yet? (The desktop writes it via linkDevice on enroll.) This
            //    is what tells the phone the link actually landed — instead
            //    of sitting forever on "finish on your other device".
            if !enrolled && main_id != 0 {
                if let Ok(devs) = crate::app::registry::devices_of(main_id).await {
                    if devs.iter().any(|d| d.eq_ignore_ascii_case(&device_addr)) {
                        enrolled = true;
                        dom::swap_inner(
                            "pair-join-msg",
                            &dom::msg_span(dom::Msg::Accent, "✓ this device is now linked"),
                        );
                    }
                }
            }
            // 2) ECIES-wrapped Gemini key (best-effort — never required).
            if !got_key {
                if let Some(slot_id) = slot_id {
                    if let Ok(Some(blob)) =
                        crate::app::registry::wrapped_device_key_of(slot_id, &device_addr).await
                    {
                        if let Some(pt) =
                            crate::app::encryption::ecies_open(&device_signer, &blob).await
                        {
                            if let Ok(key) = String::from_utf8(pt) {
                                crate::app::key_store::save(&key).await;
                                if let Ok(Some(storage)) = dom::session_storage() {
                                    let _ = storage.set_item("gemini_api_key", &key);
                                }
                                got_key = true;
                            }
                        }
                    }
                }
            }
            if enrolled {
                // Linked! The device is now an authorized signer, so opening
                // the subdomain lands it in the studio as an owner. Redirect
                // automatically rather than dead-ending on a message. Credits
                // are the default (no Gemini key needed to start); if the
                // desktop shared one via ECIES we grab it first, else give a
                // short grace window then go anyway.
                if got_key || post_enroll >= 2 {
                    dom::swap_inner(
                        "pair-join-msg",
                        &dom::msg_span(dom::Msg::Accent, "✓ linked — opening your subdomain…"),
                    );
                    if let Ok(window) = dom::window() {
                        // Route via the apex with a linked-owner pointer so the
                        // apex on THIS device learns which identity it belongs
                        // to (then it hops on to the subdomain). Falls back to a
                        // direct subdomain open if we never resolved the owner.
                        let url = if owner_hex.is_empty() {
                            format!("https://{name}.localharness.xyz/")
                        } else {
                            format!(
                                "https://localharness.xyz/?link_device={owner_hex}&then={name}"
                            )
                        };
                        let _ = window.location().set_href(&url);
                    }
                    return;
                }
                post_enroll += 1;
                dom::swap_inner(
                    "pair-join-msg",
                    &dom::msg_span(dom::Msg::Accent, "✓ linked — opening your subdomain…"),
                );
            }
            crate::runtime::sleep_ms(3000).await;
        }
    });

    Ok(device_hex)
}

/// Derive the seed-sync AES key from the LOCAL apex master wallet (no
/// iframe — this runs on the apex origin where the seed lives). Must
/// match `signer::seed_sync_key` byte-for-byte so a blob sealed by the
/// iframe path opens here. `None` if no wallet is loaded.
fn apex_seed_sync_key() -> Option<[u8; 32]> {
    use sha3::{Digest, Keccak256};
    crate::app::APP.with(|cell| {
        let app = cell.borrow();
        let wallet = app.wallet.as_ref()?;
        let entropy = wallet.mnemonic.to_entropy();
        let mut hasher = Keccak256::new();
        hasher.update(b"localharness/v0/keysync");
        hasher.update(&entropy);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hasher.finalize());
        Some(out)
    })
}

/// Desktop side. Read the MAIN's seed-sealed Gemini key from chain,
/// decrypt it with the local seed, ECIES-wrap it to the freshly-paired
/// device's pubkey, and post that blob under the device's per-device slot.
/// Errors (no key synced, no seed here) are returned so the caller can
/// silently skip — the device still enrolls as a signer regardless.
async fn wrap_and_post_key_to_device(
    main_id: u64,
    owner_hex: &str,
    device_hex: &str,
    device_pubkey: &[u8],
) -> Result<(), String> {
    let ct = crate::app::registry::gemini_key_of(main_id)
        .await
        .map_err(|e| format!("read key: {e}"))?
        .ok_or_else(|| "no synced key".to_string())?;
    let seed_key = apex_seed_sync_key().ok_or_else(|| "no seed on this device".to_string())?;
    let plaintext = crate::app::encryption::open_with_raw_key(&seed_key, &ct)
        .await
        .ok_or_else(|| "decrypt failed".to_string())?;
    let blob = crate::app::encryption::ecies_seal(device_pubkey, &plaintext)
        .await
        .ok_or_else(|| "wrap failed".to_string())?;
    let signer = crate::app::APP
        .with(|cell| cell.borrow().wallet.as_ref().map(|w| w.signer.clone()))
        .ok_or_else(|| "no wallet".to_string())?;
    let fee_payer = crate::app::sponsor::signer()?;
    crate::app::registry::set_device_wrapped_key_sponsored(
        &signer,
        &fee_payer,
        main_id,
        device_hex,
        &blob,
        crate::app::registry::ALPHA_USD_ADDRESS,
    )
    .await?;
    let _ = owner_hex; // owner is implied by the local signer
    Ok(())
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

/// Resolve the current user's MAIN's TBA address, then submit a
/// sponsored Tempo tx that creates the TBA (if needed) + adds the new
/// device as an authorized signer.
async fn run_add_device(new_signer_hex: String) -> Result<String, String> {
    // Apex wallet — must be present (the button is only rendered when
    // the apex has a wallet, so failing here is an unusual race).
    let (signer, owner_hex) = crate::app::APP
        .with(|cell| {
            cell.borrow()
                .wallet
                .as_ref()
                .map(|w| (w.signer.clone(), w.address_hex()))
        })
        .ok_or_else(|| "no apex identity".to_string())?;

    // Identify the user's MAIN. `mainOf` returns the tokenId or 0.
    let token_id = crate::app::registry::main_of(&owner_hex)
        .await
        .map_err(|e| format!("mainOf: {e}"))?;
    if token_id == 0 {
        return Err("claim a subdomain first — it becomes your MAIN".into());
    }

    // Derive the MAIN's TBA address (counterfactual or already-deployed).
    let tba_addr = crate::app::registry::tba_of_token_id(token_id)
        .await
        .map_err(|e| format!("tba lookup: {e}"))?
        .ok_or_else(|| "no TBA for MAIN".to_string())?;

    let fee_payer = crate::app::sponsor::signer()?;
    crate::app::registry::add_signer_sponsored(
        &signer,
        &fee_payer,
        token_id,
        &tba_addr,
        &new_signer_hex,
        crate::app::registry::ALPHA_USD_ADDRESS,
    )
    .await
}

/// Resolve (local owner signer, owner hex, MAIN id, MAIN TBA). The
/// shared preamble for consolidate/unlink — both act on the MAIN's TBA.
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

/// Consolidate this identity's OTHER subdomains into its MAIN's TBA — one
/// account owns them all, every linked device controls them. Moves NFTs,
/// so it's an explicit user action.
pub(super) fn consolidate_pressed() {
    dom::swap_inner(
        "consolidate-msg",
        "<span style=\"color:var(--muted)\">consolidating…</span>",
    );
    wasm_bindgen_futures::spawn_local(async move {
        let result = async {
            let (signer, owner_hex, main_id, main_tba) = owner_main_tba().await?;
            let tokens = crate::app::registry::list_owned_tokens(&owner_hex)
                .await
                .map_err(|e| format!("list: {e}"))?;
            let ids: Vec<u64> = tokens
                .iter()
                .map(|t| t.token_id)
                .filter(|id| *id != main_id)
                .collect();
            if ids.is_empty() {
                return Err("nothing to consolidate — only your MAIN".into());
            }
            let fee_payer = crate::app::sponsor::signer()?;
            crate::app::registry::consolidate_into_main_sponsored(
                &signer,
                &fee_payer,
                &main_tba,
                &ids,
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => dom::swap_inner(
                "consolidate-msg",
                "<span style=\"color:var(--muted)\">✓ subdomains consolidated under your MAIN</span>",
            ),
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!("consolidate: {e}")));
                dom::swap_inner(
                    "consolidate-msg",
                    "<span style=\"color:var(--error)\">consolidate failed</span>",
                );
            }
        }
    });
}

/// The X on a linked device. Removing a device's access is destructive
/// (revokes its signer authority + costs a sponsored tx + a re-pair to
/// undo), so a single accidental click must NOT do it — show a typed
/// confirmation in `#pair-msg` first. (Unlinking affects only THAT device;
/// the owner / other devices keep their access.)
pub(super) fn unlink_device_prompt(device_hex: String) {
    let short = short_addr(&device_hex);
    dom::swap_inner(
        "pair-msg",
        &format!(
            "<div class=\"unlink-confirm\">\
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
}

/// Abort an in-progress unlink — clear the confirmation prompt.
pub(super) fn unlink_cancel_pressed() {
    dom::swap_inner("pair-msg", "");
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
                crate::app::registry::ALPHA_USD_ADDRESS,
            )
            .await
        }
        .await;
        match result {
            Ok(_) => {
                dom::swap_inner("pair-msg", "");
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
