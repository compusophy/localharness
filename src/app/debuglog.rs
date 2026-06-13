//! On-screen diagnostics for devices with NO devtools (iOS Safari/PWA).
//!
//! Two surfaces, both monochrome and zero-cost when idle:
//!
//! * **Panic banner** — [`install_panic_banner`] chains AFTER
//!   `console_error_panic_hook` and paints the panic message (plus the last
//!   breadcrumbs) into a fixed banner at the top of the page. A wasm panic
//!   kills every spawned future, so pre-banner the ONLY mobile symptom was a
//!   silently frozen UI ("stuck on creating identity…" with the 15s timeout
//!   never firing — the timeout future was dead too).
//! * **Breadcrumb log** — [`log`] records the last [`CAP`] steps in a ring
//!   buffer. With `?debug=1` in the URL they also paint live into a fixed
//!   overlay (`#lh-debug`), so a hang (no panic, a promise that never
//!   settles) shows HOW FAR the flow got.
//!
//! The banner/overlay are built with maud (auto-escaped) and injected via
//! `insert_adjacent_html` at fixed ids — within the no-imperative-DOM rule.

use std::cell::RefCell;

use maud::html;

/// Max breadcrumbs retained (newest last).
const CAP: usize = 30;

thread_local! {
    static CRUMBS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// Record a breadcrumb (and mirror it to the console). Call this at every
/// stage of a flow that has EVER hung on mobile — the crumbs are what the
/// panic banner / `?debug=1` overlay show.
pub(crate) fn log(msg: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(&format!("[lh] {msg}")));
    CRUMBS.with(|c| {
        let mut v = c.borrow_mut();
        v.push(msg.to_string());
        if v.len() > CAP {
            let drop = v.len() - CAP;
            v.drain(..drop);
        }
    });
    if overlay_enabled() {
        paint_overlay();
    }
}

fn crumbs_snapshot() -> Vec<String> {
    CRUMBS.with(|c| c.borrow().clone())
}

/// `?debug=1` anywhere in the query turns the live overlay on.
fn overlay_enabled() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.contains("debug=1"))
        .unwrap_or(false)
}

/// (Re)paint the `#lh-debug` overlay with the current breadcrumbs.
fn paint_overlay() {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let crumbs = crumbs_snapshot();
    let markup = html! {
        div id="lh-debug" style="position:fixed;left:0;right:0;bottom:0;z-index:2147483646;\
            background:rgba(0,0,0,0.92);color:#9a9a9a;border-top:1px solid #444;\
            font:10px/1.5 monospace;padding:6px 10px;max-height:38vh;overflow-y:auto;\
            pointer-events:none;white-space:pre-wrap;" {
            @for line in &crumbs {
                div { (line) }
            }
        }
    };
    if let Some(el) = doc.get_element_by_id("lh-debug") {
        el.set_outer_html(&markup.into_string());
    } else if let Some(body) = doc.body() {
        let _ = body.insert_adjacent_html("beforeend", &markup.into_string());
    }
}

/// Install the visible panic banner. Chains the PREVIOUS hook first (so
/// `console_error_panic_hook` keeps logging a proper stack to the console),
/// then paints the message + breadcrumbs into `#lh-panic-banner`. Must be
/// called AFTER `console_error_panic_hook::set_once`.
pub(crate) fn install_panic_banner() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        prev(info);
        let msg = info.to_string();
        let crumbs = crumbs_snapshot();
        let tail_start = crumbs.len().saturating_sub(8);
        let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
        let markup = html! {
            div id="lh-panic-banner" style="position:fixed;top:0;left:0;right:0;\
                z-index:2147483647;background:#000;color:#fff;border-bottom:1px solid #fff;\
                font:11px/1.5 monospace;padding:12px 14px;max-height:50vh;overflow-y:auto;\
                white-space:pre-wrap;" {
                div { "⚠ the app crashed — screenshot this and send it:" }
                div style="margin-top:6px" { (msg) }
                @if !crumbs.is_empty() {
                    div style="margin-top:6px;color:#9a9a9a" { "last steps:" }
                    @for line in &crumbs[tail_start..] {
                        div style="color:#9a9a9a" { "· " (line) }
                    }
                }
            }
        };
        if let Some(el) = doc.get_element_by_id("lh-panic-banner") {
            el.set_outer_html(&markup.into_string());
        } else if let Some(body) = doc.body() {
            let _ = body.insert_adjacent_html("afterbegin", &markup.into_string());
        }
    }));
}
