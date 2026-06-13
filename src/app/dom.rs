//! Thin web-sys helpers. Every function in this module is a one-liner
//! over web-sys; they exist so the rest of the app reads as HTMX-style
//! HTML swaps ("find this id, swap its inner") instead of web-sys
//! incantations. **Nothing here builds DOM nodes**; that's maud's job.

use std::cell::RefCell;
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

thread_local! {
    /// The element focused when a modal/overlay opened, so closing it returns
    /// focus there (a11y #58) instead of stranding the user on `<body>`. Only
    /// one overlay is open at a time, so a single slot suffices.
    static FOCUS_RETURN: RefCell<Option<Element>> = const { RefCell::new(None) };
}

/// Save the currently-focused element so a later [`restore_focus`] can return
/// to it. Call right before opening a modal/overlay. Focus is a BEHAVIOUR, not
/// DOM construction — the no-imperative-DOM rule is about building nodes.
pub(crate) fn remember_focus() {
    if let Ok(doc) = document() {
        FOCUS_RETURN.with(|c| *c.borrow_mut() = doc.active_element());
    }
}

/// Return focus to the element [`remember_focus`] saved (call when closing a
/// modal/overlay). No-op if nothing was saved or the element is gone.
pub(crate) fn restore_focus() {
    FOCUS_RETURN.with(|c| {
        if let Some(el) = c.borrow_mut().take() {
            if let Some(h) = el.dyn_ref::<web_sys::HtmlElement>() {
                let _ = h.focus();
            }
        }
    });
}

/// Move keyboard focus to the first focusable element inside `#container_id`
/// (a11y #58: an opened modal/overlay should take focus so keyboard + screen-
/// reader users land INSIDE it, not stranded on the trigger behind it). No-op
/// if the container or a focusable child is missing.
pub(crate) fn focus_first_in(container_id: &str) {
    let Some(c) = by_id(container_id) else { return };
    let sel = "button:not([disabled]), a[href], input:not([type=hidden]):not([disabled]), \
               textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex='-1'])";
    let Ok(list) = c.query_selector_all(sel) else { return };
    // Pick the first VISIBLE focusable. A modal often renders inactive tab
    // panels as `display:none` (e.g. the admin Account/Usage/Feedback tabs);
    // `.focus()` no-ops on a non-rendered element, which would silently strand
    // focus. `offset_parent() == None` flags a `display:none` subtree, so skip
    // those and focus the first one that's actually on screen.
    for i in 0..list.length() {
        if let Some(h) = list.get(i).and_then(|n| n.dyn_into::<web_sys::HtmlElement>().ok()) {
            if h.offset_parent().is_some() {
                let _ = h.focus();
                return;
            }
        }
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

/// Remove an element from the DOM by id (no-op if it's already gone).
/// Used to drop a pre-painted shell that ended up with nothing to show
/// (e.g. a pure-`finish` assistant turn — see `chat::stream_turn`).
pub(crate) fn remove(id: &str) {
    if let Some(el) = by_id(id) {
        el.remove();
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
    // Status lives IN THE STREAM (a single replaceable system line at the end
    // of the transcript), never in the input container — the user rejected
    // input-chrome status messages repeatedly (feedback #45/#64 + direct).
    // Empty message = clear the line. The node is recreated at the transcript
    // tail so it always reads as the latest event.
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if let Some(el) = doc.get_element_by_id("system-status") {
        el.remove();
    }
    if message.is_empty() {
        return;
    }
    let Some(transcript) = doc.get_element_by_id("transcript") else {
        return;
    };
    let cls = if is_error { "system-status err" } else { "system-status" };
    let _ = transcript.insert_adjacent_html(
        "beforeend",
        &format!(
            "<div id=\"system-status\" class=\"{cls}\">{}</div>",
            html_escape(message)
        ),
    );
    scroll_to_bottom("transcript");
}

/// Minimal text→HTML escaping for status text (it can carry raw error
/// strings — never inject them as markup).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
