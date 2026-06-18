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
    // `try_borrow`, not `borrow`: this is called from the PANIC hook, and a
    // panic raised while `log()` held `borrow_mut()` would otherwise double-
    // borrow and panic INSIDE the panic hook (aborting with no banner). A
    // contended borrow just yields no crumbs — the panic message still paints.
    CRUMBS.with(|c| c.try_borrow().map(|v| v.clone()).unwrap_or_default())
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

// --- Cross-reload crash telemetry --------------------------------------------
//
// A wasm panic paints the panic banner and FREEZES; the in-memory `CRUMBS` ring
// survives (the page is still up). But an iOS WebContent process kill (OOM /
// jetsam) is a RESET: Safari respawns the renderer and reloads the tab, so the
// wasm linear memory — and every breadcrumb in it — is gone, and no panic ever
// fired. The "gray screen then reload" mid-checkout left ZERO evidence.
//
// Fix: mirror the active checkout stage + a timestamp + a crumbs snapshot into
// sessionStorage (survives the reload on the same tab; auto-clears on tab
// close). A CLEAN exit sets `lh_crash_clean` — set on success/teardown AND on
// `pagehide` (a user-initiated reload/navigation fires it; an abrupt OOM kill
// does NOT). On the reloaded tab [`detect_previous_crash`] reports "died at
// stage X after Y ms" iff a stage was active and never cleanly exited. Keys are
// shared verbatim with `web/boot.js` (the `pagehide`/error traps).

const K_STAGE: &str = "lh_crash_stage";
const K_T0: &str = "lh_crash_t0";
const K_CRUMBS: &str = "lh_crash_crumbs";
const K_CLEAN: &str = "lh_crash_clean";

fn crash_store() -> Option<web_sys::Storage> {
    crate::app::dom::session_storage().ok().flatten()
}

/// Stamp the CURRENT checkout stage into sessionStorage so it survives an iOS
/// WebContent reset (OOM kill wipes the in-wasm ring + paints no panic banner).
/// Also records the in-memory breadcrumb. Pair with [`stage_clean`].
pub(crate) fn stage(name: &str) {
    log(name);
    let Some(s) = crash_store() else { return };
    let now = js_sys::Date::now() as u64;
    let _ = s.set_item(K_STAGE, name);
    let _ = s.set_item(K_T0, &now.to_string());
    if let Ok(json) = serde_json::to_string(&crumbs_snapshot()) {
        let _ = s.set_item(K_CRUMBS, &json);
    }
    // A freshly-entered stage is, by definition, not yet a clean exit.
    let _ = s.remove_item(K_CLEAN);
}

/// Mark the current checkout stage as cleanly exited (success / teardown) so a
/// later reload doesn't mistake a benign navigation for a crash.
pub(crate) fn stage_clean() {
    let Some(s) = crash_store() else { return };
    let _ = s.set_item(K_CLEAN, "1");
    let _ = s.remove_item(K_STAGE);
}

fn clear_crash_markers(s: &web_sys::Storage) {
    let _ = s.remove_item(K_STAGE);
    let _ = s.remove_item(K_T0);
    let _ = s.remove_item(K_CRUMBS);
    let _ = s.remove_item(K_CLEAN);
}

/// Boot-time: if the previous run was inside a checkout stage and never cleanly
/// exited, the iOS WebContent process was almost certainly killed (OOM/jetsam)
/// mid-checkout — a reset, not a panic, so neither the panic banner nor the
/// in-wasm crumbs survived. Records a breadcrumb (visible in the `?debug=1`
/// overlay) and, under `?debug=1`, paints an amber banner with the stage +
/// elapsed time + the last persisted crumbs. The visible banner is gated so a
/// real visitor who once crashed isn't greeted by an alarm line; the user
/// debugging iOS just opens the page with `?debug=1` (the query survives the
/// auto-reload). Markers are cleared either way so a clean reload shows nothing.
pub(crate) fn detect_previous_crash() {
    let Some(s) = crash_store() else { return };
    let Some(stage) = s.get_item(K_STAGE).ok().flatten() else { return };
    let clean = s.get_item(K_CLEAN).ok().flatten().as_deref() == Some("1");
    let t0 = s
        .get_item(K_T0)
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let crumbs: Vec<String> = s
        .get_item(K_CRUMBS)
        .ok()
        .flatten()
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();
    clear_crash_markers(&s);
    if clean {
        return; // clean exit recorded — nothing to report
    }
    let elapsed = (js_sys::Date::now() as u64).saturating_sub(t0);
    log(&format!(
        "⚠ previous session RESET at stage '{stage}' after {elapsed}ms (no clean exit — likely iOS OOM)"
    ));
    if !overlay_enabled() {
        return; // visible banner only under ?debug=1 (don't alarm real visitors)
    }
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let tail = crumbs.len().saturating_sub(6);
    let markup = html! {
        div id="lh-crash-banner" style="position:fixed;top:0;left:0;right:0;\
            z-index:2147483647;background:#1a1200;color:#ffae42;border-bottom:1px solid #ffae42;\
            font:11px/1.5 monospace;padding:12px 14px;max-height:50vh;overflow-y:auto;\
            white-space:pre-wrap;" {
            div { "⚠ previous session reset during checkout (no clean exit — likely iOS out-of-memory)" }
            div style="margin-top:6px" { "stage: " (stage) " · " (elapsed) "ms in" }
            @if !crumbs.is_empty() {
                div style="margin-top:6px;color:#bbb" { "last steps before reset:" }
                @for line in &crumbs[tail..] {
                    div style="color:#bbb" { "· " (line) }
                }
            }
        }
    };
    if let Some(el) = doc.get_element_by_id("lh-crash-banner") {
        el.set_outer_html(&markup.into_string());
    } else if let Some(body) = doc.body() {
        let _ = body.insert_adjacent_html("afterbegin", &markup.into_string());
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
