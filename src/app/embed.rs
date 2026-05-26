//! Embed mode — `?embed=1` short-circuits the normal chrome and
//! paints just a single identity card sized for inclusion inside
//! another origin's `<iframe>`. The composable-subdomain primitive.
//!
//! ## Trust + isolation
//!
//! Each module remains in its own origin: an iframe pointing at
//! `name.localharness.xyz/?embed=1` still has its own OPFS, its own
//! cookies, its own wallet, its own signer. The host page only sees
//! the iframe's frame; it never gets to read the module's state.
//! That's the whole reason this design uses iframes instead of
//! ShadowDOM custom elements — losing per-origin isolation was
//! ruled out in the AI-OS planning thread.
//!
//! ## Recursion-depth concern
//!
//! Browsers cap iframe nesting at ~10. With one level of nesting
//! (host page → module iframe), we sit at depth 1 from the user's
//! tab. Modules embedding sub-modules would push depth further; the
//! current MVP doesn't do that — sub-composition is the host's job,
//! all at depth 1 as siblings.
//!
//! ## postMessage protocol (v1)
//!
//! ```text
//! module → parent: { type: "lh-embed-ready", name, height }
//! ```
//!
//! v1 is one-shot: the module paints itself, measures its rendered
//! height, posts the ready ping once. The host sizes its iframe to
//! that height. No host→module messages yet — interactive modules
//! (props, actions, re-renders) layer on top later.

use wasm_bindgen::prelude::*;

use crate::registry;

use super::dom;
use super::tenant;
use super::templates;

/// `true` iff `?embed=1` is in the URL.
pub(crate) fn has_embed_hint() -> bool {
    let Ok(window) = dom::window() else { return false };
    let Ok(search) = window.location().search() else { return false };
    search.contains("embed=1")
}

/// Paint the embed-mode chrome. Called from `mount()` when
/// [`has_embed_hint`] returns true. Loads the on-chain owner +
/// TBA + $LH balance in the background; the placeholder card paints
/// immediately so the host sees something fast.
pub(crate) async fn paint_embed(host: tenant::Host) {
    let Ok(doc) = dom::document() else { return };
    let Some(root) = doc.get_element_by_id("root") else { return };

    let name = match &host {
        tenant::Host::Tenant(n) => n.clone(),
        tenant::Host::Apex => "localharness".to_string(),
        tenant::Host::Other(h) => h.clone(),
    };

    // Initial paint — placeholder values. Real data swaps in below.
    let placeholder = templates::embed_card(&name, None, None, None, None).into_string();
    root.set_inner_html(&placeholder);
    notify_parent_ready(&name);

    // Resolve owner + TBA + balance + MAIN status in the background.
    // Each step that lands re-paints the card and re-notifies the parent
    // so the host can resize if our content height changed.
    let owner = registry::owner_of_name(&name).await.ok().flatten();
    let tba = match &owner {
        Some(_) => registry::tba_of_name(&name).await.ok().flatten(),
        None => None,
    };
    let lh_balance = match &tba {
        Some(addr) => registry::token_balance_of(addr).await.ok(),
        None => None,
    };
    let is_main = match &owner {
        Some(addr) => {
            let main_id = registry::main_of(addr).await.ok().unwrap_or(0);
            if main_id == 0 {
                false
            } else {
                registry::name_of_id(main_id)
                    .await
                    .ok()
                    .map(|n| n == name)
                    .unwrap_or(false)
            }
        }
        None => false,
    };

    let html = templates::embed_card(
        &name,
        owner.as_deref(),
        tba.as_deref(),
        lh_balance,
        Some(is_main),
    )
    .into_string();
    root.set_inner_html(&html);
    notify_parent_ready(&name);
}

/// Post `{type: 'lh-embed-ready', name, height}` to `window.parent`
/// so the host can size its iframe to match our content. Target
/// origin is `*` — the message carries no sensitive data, only
/// content metadata; the host enforces its own origin allowlist on
/// the receive side.
fn notify_parent_ready(name: &str) {
    let Ok(window) = dom::window() else { return };
    let height = window
        .document()
        .and_then(|d| d.document_element())
        .map(|el| el.scroll_height())
        .unwrap_or(0);

    let payload = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("type"),
        &JsValue::from_str("lh-embed-ready"),
    );
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("name"),
        &JsValue::from_str(name),
    );
    let _ = js_sys::Reflect::set(
        &payload,
        &JsValue::from_str("height"),
        &JsValue::from_f64(height as f64),
    );

    if let Ok(Some(parent)) = window.parent() {
        let _ = parent.post_message(&payload.into(), "*");
    }
}
