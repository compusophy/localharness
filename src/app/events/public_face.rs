//! Public face — publish the subdomain face choice (directory / app / html).

use crate::encoding::parse_address;

use crate::app::dom;

/// Set this subdomain's public face on-chain — `"directory"`, `"app"`, or
/// `"html"`. The choice lives under `keccak256("localharness.public_face")`
/// so every visitor honours it. For `"app"`/`"html"` we also publish the
/// device's local `app.rl`/`index.html` content in the SAME sponsored tx
/// (owner-signed through the apex iframe), so the chosen face is live
/// immediately. Owner-only.
pub(super) async fn run_set_public_face(choice: &str) {
    let msg = "publish-app-msg";
    let set_err = |m: &str| {
        dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, m));
    };

    let Some(name) = crate::app::tenant::current_name() else {
        set_err("only on a subdomain");
        return;
    };

    // App face: publish OFF-CHAIN to the app store (free, no gas) when this
    // device's EOA directly owns the name. Falls through to the on-chain path on
    // ANY inability (TBA owner, no local signer, or a store error), so a publish
    // never regresses from "works (on-chain gas)" to "broken".
    if choice == "app" && try_publish_app_offchain(&name, msg).await {
        return;
    }

    // The verified-EOA address IF this device verified as the on-chain
    // owner directly. May be None when the owner is a TBA we sign for
    // (consolidation) — that path is decided in the submit branch below.
    let verified_eoa = crate::app::APP.with(|cell| {
        use crate::app::VerifyState;
        match &cell.borrow().verify_state {
            VerifyState::Verified { address } => Some(address.clone()),
            _ => None,
        }
    });

    let id = match crate::app::registry::id_of_name(&name).await {
        Ok(id) if id != 0 => id,
        _ => {
            set_err("name isn't registered on-chain");
            return;
        }
    };

    let registry_addr = match parse_address(crate::app::registry::REGISTRY_ADDRESS()) {
        Ok(a) => a,
        Err(e) => {
            set_err(&e);
            return;
        }
    };
    let mk = |input: Vec<u8>| crate::tempo_tx::TempoCall {
        to: registry_addr,
        value_wei: 0,
        input,
    };

    // Build the call batch + gas estimate for the chosen face. Storing
    // `bytes` costs ~20k gas per 32-byte word (cold SSTORE) on top of the
    // ~275k Tempo sponsorship + base call.
    let (calls, gas): (Vec<crate::tempo_tx::TempoCall>, u128) = match choice {
        "directory" => (
            vec![mk(crate::app::registry::encode_set_public_face(id, "directory"))],
            500_000,
        ),
        "app" => {
            let fs = crate::app::shared_opfs();
            let src = match fs.read("app.rl").await {
                Ok(b) if !b.is_empty() => String::from_utf8_lossy(&b).into_owned(),
                _ => {
                    set_err("no app.rl on this device — build one first (run_cartridge)");
                    return;
                }
            };
            let wasm = match crate::rustlite::compile(&src) {
                Ok(w) => w,
                Err(e) => {
                    // The status line is single-line — append the line/col
                    // locator (the caret snippet wouldn't survive it).
                    let loc = e
                        .location(&src)
                        .map(|l| format!(" ({l})"))
                        .unwrap_or_default();
                    set_err(&format!("compile: {e}{loc}"));
                    return;
                }
            };
            if wasm.len() > 16_384 {
                set_err("app wasm too large to publish (max 16 KB)");
                return;
            }
            (
                vec![
                    mk(crate::app::registry::encode_set_app_wasm(id, &wasm)),
                    mk(crate::app::registry::encode_set_public_face(id, "app")),
                ],
                crate::app::gas::set_metadata_gas(wasm.len()),
            )
        }
        "html" => {
            let fs = crate::app::shared_opfs();
            let html = match fs.read("index.html").await {
                Ok(b) if !b.is_empty() => b,
                _ => {
                    set_err("no index.html on this device — create one first");
                    return;
                }
            };
            if html.len() > 24_576 {
                set_err("index.html too large to publish (max 24 KB)");
                return;
            }
            (
                vec![
                    mk(crate::app::registry::encode_set_public_html(id, &html)),
                    mk(crate::app::registry::encode_set_public_face(id, "html")),
                ],
                crate::app::gas::set_metadata_gas(html.len()),
            )
        }
        _ => {
            set_err("unknown public face");
            return;
        }
    };

    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">saving…</span>");

    // Decide the execution path from the on-chain owner:
    //  - owner is a TBA this device signs for (consolidation) → execute
    //    the setMetadata batch THROUGH the TBA, signed by our local key.
    //  - owner is our verified EOA → direct sponsored call (existing).
    let on_chain_owner = match crate::app::registry::owner_of_name(&name).await {
        Ok(Some(o)) => o,
        _ => {
            set_err("name isn't registered on-chain");
            return;
        }
    };
    let local = crate::app::chat::credit_signer().await;
    let is_signer = match &local {
        Some((_, addr)) => {
            let addr_hex = crate::encoding::bytes_to_hex_str(addr);
            crate::app::registry::is_authorized_signer(&on_chain_owner, &addr_hex)
                .await
                .unwrap_or(false)
        }
        None => false,
    };
    let result = if is_signer {
        let (signer, _) = local.unwrap();
        let fee_payer = match crate::app::sponsor::signer() {
            Ok(s) => s,
            Err(e) => {
                set_err(&e);
                return;
            }
        };
        let token_id = match crate::app::registry::tba_token_id_of(&on_chain_owner).await {
            Ok(t) => t,
            Err(e) => {
                set_err(&e);
                return;
            }
        };
        let targets: Vec<([u8; 20], Vec<u8>)> =
            calls.iter().map(|c| (registry_addr, c.input.clone())).collect();
        crate::app::registry::tba_execute_batch_sponsored(
            &signer,
            &fee_payer,
            token_id,
            &on_chain_owner,
            &targets,
            crate::app::registry::ALPHA_USD_ADDRESS(),
            gas + 800_000,
        )
        .await
    } else if let Some(owner_hex) =
        verified_eoa.filter(|a| a.eq_ignore_ascii_case(&on_chain_owner))
    {
        super::run_sponsored_tempo_call(&owner_hex, calls, gas, "public face").await
    } else {
        set_err("verify as owner first");
        return;
    };
    match result {
        Ok(_tx) => {
            if choice == "directory" {
                dom::swap_inner(
                    msg,
                    &maud::html! {
                        span style="color:var(--fg)" {
                            "public face → directory ✓ "
                            a href=(format!("https://{name}.localharness.xyz/"))
                              target="_blank" rel="noopener" style="color:var(--accent)" {
                                "open →"
                            }
                        }
                    }
                    .into_string(),
                );
            } else {
                // The share moment: published app/html is live for every
                // visitor — surface the URL + [copy] + QR right here.
                dom::swap_inner(
                    msg,
                    &crate::app::templates::publish_share_fragment(&name).into_string(),
                );
            }
            super::admin::refresh_public_face_status().await;
        }
        Err(e) => set_err(&format!("failed: {e}")),
    }
}

/// Try to publish this device's local `app.rl` to the OFF-CHAIN app store. `true`
/// = published (UI updated; the caller should return); `false` = not handled here
/// so the caller falls back to the on-chain path — no `app.rl`, a compile error,
/// too large, no local signer, the name is TBA-owned (not our EOA), or the store
/// POST failed. Reuses the SAME personal-sign token the model calls use; the
/// proxy authorizes via on-chain ownership (`ownerOf(name) == signer`).
async fn try_publish_app_offchain(name: &str, msg: &str) -> bool {
    let fs = crate::app::shared_opfs();
    let src = match fs.read("app.rl").await {
        Ok(b) if !b.is_empty() => String::from_utf8_lossy(&b).into_owned(),
        _ => return false, // no app.rl → let the on-chain path report it
    };
    let wasm = match crate::rustlite::compile(&src) {
        Ok(w) => w,
        Err(_) => return false, // compile error → on-chain path renders the caret
    };
    if wasm.len() > crate::app::registry::APP_STORE_MAX_WASM_BYTES {
        return false;
    }
    // The proxy gates the publish on ownerOf(name) == token signer, which holds
    // only when this device's MASTER wallet owns the name directly. Read
    // `APP.wallet` (the master) DIRECTLY — NOT credit_signer(), which can return
    // or MINT a per-origin device key (a linked device) that isn't the owner. A
    // TBA-owned name or a master-not-loaded device → on-chain path below.
    let owner = match crate::app::registry::owner_of_name(name).await {
        Ok(Some(o)) => o,
        _ => return false,
    };
    let Some((signer, addr)) = crate::app::APP
        .with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)))
    else {
        return false;
    };
    if !owner.eq_ignore_ascii_case(&crate::encoding::bytes_to_hex_str(&addr)) {
        return false; // TBA / different owner → on-chain path
    }
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now);
    dom::swap_inner(
        msg,
        "<span style=\"color:var(--muted)\">publishing (off-chain)…</span>",
    );
    match crate::app::registry::publish_app_to_store(name, &token, &wasm, &src).await {
        Ok(()) => {
            dom::swap_inner(
                msg,
                &crate::app::templates::publish_share_fragment(name).into_string(),
            );
            super::admin::refresh_public_face_status().await;
            true
        }
        Err(_) => false, // store hiccup → fall back to the on-chain publish
    }
}

/// Copy `text` to the clipboard (`navigator.clipboard.writeText`) and
/// flip the `flip_id` button's label to "copied ✓" as the only feedback.
/// Shared by the share-URL and seed-reveal [copy] buttons.
pub(super) async fn run_copy_to_clipboard(text: &str, flip_id: &str) {
    let Some(win) = web_sys::window() else { return };
    let promise = win.navigator().clipboard().write_text(text);
    if wasm_bindgen_futures::JsFuture::from(promise).await.is_ok() {
        dom::swap_inner(flip_id, "copied ✓");
    }
}
