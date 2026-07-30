//! Public face — publish the subdomain face choice (directory / app / html).

use crate::app::dom;

/// Set this subdomain's public face — `"directory"`, `"app"`, or `"html"`.
/// STORE-ONLY: content publishes carry bytes + the `<name>/face` choice record
/// in ONE authed POST (the proxy stamps it), and "directory" is a face-only
/// POST. Free, no gas, no tx — nothing here touches the chain (the sponsored
/// legacy path was purged with the pre-1.0.0 reset; a TBA-owned name or a
/// linked device without the seed gets an honest error until the store gains
/// TBA/authorized-signer auth). Owner-only.
pub(super) async fn run_set_public_face(choice: &str) {
    let msg = "publish-app-msg";
    let Some(name) = crate::app::tenant::current_name() else {
        dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, "only on a subdomain"));
        return;
    };
    match choice {
        "app" => publish_app_offchain(&name, msg).await,
        "html" => publish_html_offchain(&name, msg).await,
        "directory" => set_directory_offchain(&name, msg).await,
        _ => dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, "unknown public face")),
    }
}

/// The device MASTER wallet's signer, verified to be `name`'s on-chain owner —
/// the one identity the store's `ownerOf(name) == token signer` gate accepts.
/// Read `APP.wallet` DIRECTLY, never credit_signer(): that can return (or even
/// MINT) a per-origin DEVICE key that is not the owner. Errors are painted
/// into `msg`; `None` = already reported.
async fn store_publish_signer(name: &str, msg: &str) -> Option<k256::ecdsa::SigningKey> {
    let set_err = |m: &str| dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, m));
    let owner = match crate::app::registry::owner_of_name(name).await {
        Ok(Some(o)) => o,
        _ => {
            set_err("name isn't registered on-chain");
            return None;
        }
    };
    let Some((signer, addr)) = crate::app::APP
        .with(|c| c.borrow().wallet.as_ref().map(|w| (w.signer.clone(), w.address)))
    else {
        set_err("publishing needs this device to hold the owner wallet");
        return None;
    };
    if !owner.eq_ignore_ascii_case(&crate::encoding::bytes_to_hex_str(&addr)) {
        set_err(&format!(
            "this device's wallet doesn't own {name} (owner {owner}) — TBA-owned names / \
             linked devices can't publish yet"
        ));
        return None;
    }
    Some(signer)
}

/// Publish this device's local `app.rl` to the app store (bytes + the face
/// record in one POST). All errors paint into `msg` — no fallback path.
async fn publish_app_offchain(name: &str, msg: &str) {
    let set_err = |m: &str| dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, m));
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
            // The status line is single-line — append the line/col locator
            // (the caret snippet wouldn't survive it).
            let loc = e.location(&src).map(|l| format!(" ({l})")).unwrap_or_default();
            set_err(&format!("compile: {e}{loc}"));
            return;
        }
    };
    if wasm.len() > crate::app::registry::APP_STORE_MAX_WASM_BYTES {
        set_err("app wasm too large to publish (max 1 MB)");
        return;
    }
    let Some(signer) = store_publish_signer(name, msg).await else { return };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now, "publish");
    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">publishing…</span>");
    match crate::app::registry::publish_app_to_store(name, &token, &wasm, &src).await {
        Ok(()) => {
            dom::swap_inner(
                msg,
                &crate::app::templates::publish_share_fragment(name).into_string(),
            );
            super::admin::refresh_public_face_status().await;
        }
        Err(e) => set_err(&format!("publish failed: {e} — retry")),
    }
}

/// HTML-face sibling of [`publish_app_offchain`]: publish this device's local
/// `index.html` to the app store.
async fn publish_html_offchain(name: &str, msg: &str) {
    let set_err = |m: &str| dom::swap_inner(msg, &dom::msg_span(dom::Msg::Error, m));
    let fs = crate::app::shared_opfs();
    let html = match fs.read("index.html").await {
        Ok(b) if !b.is_empty() => b,
        _ => {
            set_err("no index.html on this device — create one first");
            return;
        }
    };
    if html.len() > crate::app::registry::APP_STORE_MAX_WASM_BYTES {
        set_err("index.html too large to publish (max 1 MB)");
        return;
    }
    let Some(signer) = store_publish_signer(name, msg).await else { return };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now, "publish");
    let html_str = String::from_utf8_lossy(&html).into_owned();
    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">publishing…</span>");
    match crate::app::registry::publish_html_to_store(name, &token, &html_str).await {
        Ok(()) => {
            dom::swap_inner(
                msg,
                &crate::app::templates::publish_share_fragment(name).into_string(),
            );
            super::admin::refresh_public_face_status().await;
        }
        Err(e) => set_err(&format!("publish failed: {e} — retry")),
    }
}

/// FACE-ONLY store write: pick "directory" by POSTing just the choice.
async fn set_directory_offchain(name: &str, msg: &str) {
    let Some(signer) = store_publish_signer(name, msg).await else { return };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now, "publish");
    dom::swap_inner(msg, "<span style=\"color:var(--muted)\">saving…</span>");
    if let Err(e) = crate::app::registry::publish_face_to_store(name, &token, "directory").await
    {
        dom::swap_inner(
            msg,
            &dom::msg_span(dom::Msg::Error, &format!("failed: {e} — retry")),
        );
        return;
    }
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
    super::admin::refresh_public_face_status().await;
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
