//! Thin web-sys helpers. Every function in this module is a one-liner
//! over web-sys; they exist so the rest of the app reads as HTMX-style
//! HTML swaps ("find this id, swap its inner") instead of web-sys
//! incantations. **Nothing here builds DOM nodes**; that's maud's job.

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{Document, Element, HtmlInputElement, HtmlTextAreaElement, Storage, Window};

pub(crate) fn window() -> Result<Window, JsValue> {
    web_sys::window().ok_or_else(|| JsValue::from_str("no window — wrong execution context"))
}

pub(crate) fn document() -> Result<Document, JsValue> {
    window()?
        .document()
        .ok_or_else(|| JsValue::from_str("no document — wrong execution context"))
}

pub(crate) fn session_storage() -> Result<Option<Storage>, JsValue> {
    window()?.session_storage()
}

pub(crate) fn by_id(id: &str) -> Option<Element> {
    document().ok()?.get_element_by_id(id)
}

pub(crate) fn input_by_id(id: &str) -> Option<HtmlInputElement> {
    by_id(id)?.dyn_into::<HtmlInputElement>().ok()
}

pub(crate) fn textarea_by_id(id: &str) -> Option<HtmlTextAreaElement> {
    by_id(id)?.dyn_into::<HtmlTextAreaElement>().ok()
}

/// Semantic colour for a status / result message span.
#[derive(Clone, Copy)]
pub(crate) enum Msg {
    Error,
    Muted,
    Accent,
}

impl Msg {
    fn css_var(self) -> &'static str {
        match self {
            Msg::Error => "--error",
            Msg::Muted => "--muted",
            Msg::Accent => "--accent",
        }
    }
}

/// Build a coloured status `<span>` whose body is HTML-escaped by maud.
/// Use this for ANY message that interpolates dynamic or externally-
/// sourced text — error strings, JSON-RPC node `err.message`, agent
/// summaries — instead of `format!("<span …>{err}</span>")`. Escaping
/// stops a hostile error message from injecting live markup into a
/// wallet-bearing origin (any localharness origin can iframe the apex
/// signer, so XSS there == full wallet compromise). Returns the span as
/// a string so it composes with `swap_inner` / `set_inner_html` / maud.
pub(crate) fn msg_span(kind: Msg, text: &str) -> String {
    let style = format!("color:var({})", kind.css_var());
    maud::html! { span style=(style) { (text) } }.into_string()
}

/// HTMX-style "swap inner". Replaces the entire inside of `#id` with
/// the supplied HTML string. No-op if the element doesn't exist.
pub(crate) fn swap_inner(id: &str, html: &str) {
    if let Some(el) = by_id(id) {
        el.set_inner_html(html);
    }
}

/// HTMX-style "swap outer". Replaces `#id` and all its children with
/// the supplied HTML. No-op if the element doesn't exist. Use this
/// instead of `swap_inner` when you want to change the element's own
/// tag, attributes, or classes.
pub(crate) fn swap_outer(id: &str, html: &str) {
    if let Some(el) = by_id(id) {
        el.set_outer_html(html);
    }
}

/// HTMX-style "append a fragment at the end of `#id`". Wraps
/// `Element::insert_adjacent_html("beforeend", ...)`. No-op on missing
/// id or on an HTML error.
pub(crate) fn append_html(id: &str, html: &str) {
    if let Some(el) = by_id(id) {
        let _ = el.insert_adjacent_html("beforeend", html);
    }
}

/// Scroll an element to the bottom. Used by the chat to keep the
/// latest content in view as the assistant streams.
pub(crate) fn scroll_to_bottom(id: &str) {
    if let Some(el) = by_id(id) {
        el.set_scroll_top(el.scroll_height());
    }
}

/// Scroll to the bottom now AND again shortly after, so content that
/// grows post-append still ends pinned to the latest entry. On first
/// load the transcript is restored before layout/font swap settles, so
/// a single synchronous scroll lands at the wrong offset; the delayed
/// passes (one quick, one after the web-font swaps in) correct it.
pub(crate) fn scroll_to_bottom_soon(id: &str) {
    scroll_to_bottom(id);
    let Ok(win) = window() else { return };
    for delay in [60, 350] {
        let id = id.to_string();
        let cb = Closure::once_into_js(move || scroll_to_bottom(&id));
        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
            cb.unchecked_ref(),
            delay,
        );
    }
}

/// Stamp `data-lh-ready` on `<html>` once an interactive surface is in
/// the DOM. Chrome's paint-holding keeps the PREVIOUS page's pixels on
/// screen across a reload, so the app can LOOK interactive seconds before
/// this bundle has mounted — clicks in that window land on a
/// not-yet-listening document and vanish. Automation (and the smoke drive,
/// `scripts/browser-smoke.md`) polls this attribute instead of guessing.
pub(crate) fn mark_ready() {
    if let Ok(doc) = document() {
        if let Some(el) = doc.document_element() {
            let _ = el.set_attribute("data-lh-ready", "1");
        }
    }
}

pub(crate) fn set_status(message: &str, is_error: bool) {
    if let Some(el) = by_id("status") {
        el.set_text_content(Some(message));
        let cls = el.class_name();
        let cleaned: Vec<&str> = cls.split_whitespace().filter(|c| *c != "err").collect();
        let mut new_cls = cleaned.join(" ");
        if is_error {
            if !new_cls.is_empty() {
                new_cls.push(' ');
            }
            new_cls.push_str("err");
        }
        el.set_class_name(&new_cls);
    }
}
