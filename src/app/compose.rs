//! Compose mode — `?compose=foo,bar,baz` renders a grid of module
//! iframes, each pointing at `<name>.localharness.xyz/?embed=1`.
//! The minimal host harness for the embed primitive.
//!
//! Activated on any host (apex / tenant / other). Each comma-
//! separated name becomes one iframe at depth 1 from the user's
//! tab. The iframe loads the embed-mode chrome (see `embed.rs`),
//! paints its identity card, and posts `lh-embed-ready` back to
//! the host. The host then sizes that iframe to the reported height.
//!
//! Trust model: this is host-controlled composition. The host page
//! decides which subdomains to load. Each module remains in its own
//! origin (its own OPFS, its own signer iframe to apex). The host
//! can't read module state across origin boundaries.
//!
//! v1 is read-only — module → host postMessage only. Host → module
//! (props, action dispatch) layers on later when interactive modules
//! exist.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::MessageEvent;

use super::dom;
use super::templates;

/// `Some(names)` iff `?compose=...` is in the URL with at least one
/// comma-separated entry. Names are sanitized — only lowercase
/// alphanumerics + hyphen are allowed, matching the registry's name
/// charset. Empty entries silently dropped.
pub(crate) fn compose_names() -> Option<Vec<String>> {
    let window = dom::window().ok()?;
    let search = window.location().search().ok()?;
    let stripped = search.trim_start_matches('?');
    for pair in stripped.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        if k != "compose" {
            continue;
        }
        let decoded = super::decode_uri_component(v);
        let names: Vec<String> = decoded
            .split(',')
            .map(sanitize_name)
            .filter(|s| !s.is_empty())
            .collect();
        if names.is_empty() {
            return None;
        }
        return Some(names);
    }
    None
}

fn sanitize_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Paint the compose chrome — a header + a grid of named iframes —
/// and install the postMessage listener that resizes each iframe
/// when its module posts `lh-embed-ready`.
pub(crate) fn paint_compose(names: Vec<String>) -> Result<(), JsValue> {
    let doc = dom::document()?;
    let root = doc
        .get_element_by_id("root")
        .ok_or_else(|| JsValue::from_str("missing #root"))?;
    root.set_inner_html(&templates::compose_chrome(&names).into_string());
    install_height_listener()?;
    Ok(())
}

/// Single delegated listener for `lh-embed-ready` messages. Walks the
/// modules grid, finds the iframe whose `data-embed-name` matches the
/// payload, and sets its `style.height` to the reported pixel value.
fn install_height_listener() -> Result<(), JsValue> {
    let window = dom::window()?;
    let installed: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let installed_for_handler = installed.clone();

    let handler = Closure::<dyn FnMut(_)>::new(move |event: MessageEvent| {
        if *installed_for_handler.borrow() {
            // Sticky flag for now — we never uninstall. The Rc just
            // keeps the borrow check happy.
        }
        let data = event.data();
        let msg_type = js_sys::Reflect::get(&data, &JsValue::from_str("type"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        if msg_type != "lh-embed-ready" {
            return;
        }
        let origin = event.origin();
        if !is_trusted_origin(&origin) {
            return;
        }
        let name = js_sys::Reflect::get(&data, &JsValue::from_str("name"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let height = js_sys::Reflect::get(&data, &JsValue::from_str("height"))
            .ok()
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        if name.is_empty() || height <= 0.0 {
            return;
        }
        resize_iframe(&name, height as i32);
    });

    *installed.borrow_mut() = true;
    window.add_event_listener_with_callback("message", handler.as_ref().unchecked_ref())?;
    handler.forget();
    Ok(())
}

fn resize_iframe(name: &str, height: i32) {
    let Ok(doc) = dom::document() else { return };
    let selector = format!("iframe[data-embed-name='{name}']");
    let Ok(Some(el)) = doc.query_selector(&selector) else { return };
    // Use the inline `style` attribute directly — we don't depend on
    // the `CssStyleDeclaration` web-sys feature this way. Add a small
    // buffer so the iframe's own scrollbar doesn't appear when the
    // embedded content is exactly its scroll-height (rounding).
    let _ = el.set_attribute("style", &format!("width:100%;height:{}px;border:0;display:block;background:transparent;", height + 4));
}

fn is_trusted_origin(origin: &str) -> bool {
    super::tenant::is_trusted_lh_origin(origin)
}
