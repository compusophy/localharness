//! SURFACE — the main-thread DOM half of the display: canvas mount + overlay
//! chrome, pointer/touch state, embed-card plumbing, and the broadcast-composer
//! UI. The cartridge itself runs off-thread (see [`super::worker`]); this
//! module owns everything it draws INTO and the input it polls FROM.

use std::cell::{Cell, RefCell};

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

use crate::app::{dom, templates};

use super::{bridge, worker, FB_H, FB_W};

thread_local! {
    /// Latest cursor position in framebuffer coordinates. Updated by the
    /// delegated `mousemove` listener (see `events.rs`), forwarded to the
    /// worker (which owns the cartridge's `pointer_*` host imports). Poll model.
    static POINTER: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    /// 1 while the primary mouse button is down over the canvas. Updated
    /// by the delegated mousedown/mouseup listeners, forwarded to the worker.
    static POINTER_DOWN: Cell<i32> = const { Cell::new(0) };
    /// The DOM id of the canvas the CURRENTLY-RUNNING cartridge draws into.
    /// v1 is single-worker (one cartridge active at a time, OVERLAY or an
    /// inline embed), so pointer input maps relative to THIS canvas's rect.
    /// `run_with_ctx` sets it on each launch; defaults to the overlay canvas.
    /// (Concurrent embeds — a per-canvas worker registry — would replace this
    /// single id with per-worker input routing; see `run_in_canvas`.)
    static ACTIVE_CANVAS_ID: RefCell<String> = RefCell::new(String::from("display-canvas"));
}

/// Record which canvas the current cartridge launch owns (pointer routing).
/// Called by `run_with_ctx` / `mount_composition` on each launch.
pub(super) fn set_active_canvas(canvas_id: &str) {
    ACTIVE_CANVAS_ID.with(|c| *c.borrow_mut() = canvas_id.to_string());
}

/// Clear the primary-button state (a fresh cartridge starts with no input).
pub(super) fn reset_pointer_down() {
    POINTER_DOWN.with(|d| d.set(0));
}

/// Update the primary-button state from mousedown/mouseup over the
/// canvas. Called from the delegated listeners in `events.rs`.
pub(crate) fn set_pointer_down(down: bool) {
    POINTER_DOWN.with(|d| d.set(if down { 1 } else { 0 }));
    forward_pointer_to_worker();
}

/// Forward the latest pointer state (position + button) to the cartridge
/// worker if one is live. Every cartridge path — single, embed, and `?compose=`
/// — runs off-thread, so the `pointer_*` host imports read cells INSIDE the
/// worker; we keep them fresh by posting on every pointer event. The worker's
/// `?compose=` loop hit-tests this pointer to the child under it (focus-gated).
/// No-op when no worker is active.
fn forward_pointer_to_worker() {
    if worker::is_active() {
        let (x, y) = POINTER.with(|p| p.get());
        let down = POINTER_DOWN.with(|d| d.get());
        worker::post_input(x, y, down);
    }
}

/// Update the cursor position from a `mousemove` over the canvas. Maps
/// client (CSS-pixel) coordinates to framebuffer coordinates using the
/// canvas's displayed rect, so cartridges see logical pixels regardless
/// of how the canvas is scaled. Called from the delegated listener in
/// `events.rs`.
pub(crate) fn set_pointer(client_x: f64, client_y: f64) {
    // Map relative to the ACTIVE cartridge's canvas — the overlay
    // `#display-canvas` for a fullscreen run, or an inline `#embed-canvas`
    // for an `embed_app` card. v1 is single-worker, so exactly one canvas is
    // the live cartridge's at a time (`run_with_ctx` records its id).
    let active_id = ACTIVE_CANVAS_ID.with(|c| c.borrow().clone());
    let Some(el) = dom::by_id(&active_id) else { return };
    let Ok(canvas) = el.dyn_into::<HtmlCanvasElement>() else { return };
    // The LIVE rect carries both the canvas's current page OFFSET (left/top —
    // robust to the container sitting anywhere in the layout) and its RENDERED
    // size (width/height — what CSS letterboxing scaled it to). Reading it every
    // event means a moved/resized/letterboxed canvas never desyncs the map.
    let rect = canvas.get_bounding_client_rect();
    let (rect_w, rect_h) = (rect.width(), rect.height());
    if rect_w <= 0.0 || rect_h <= 0.0 {
        return;
    }
    // Map into the canvas's ACTUAL backing-store resolution — the cartridge's
    // declared framebuffer dims (which `blit_frame` set on the first frame), NOT
    // the fixed default. A 512×512 cartridge sees pointer coords in 512×512
    // space. Fall back to the default before the first frame sizes the canvas.
    let fb_w = if canvas.width() > 0 { canvas.width() } else { FB_W };
    let fb_h = if canvas.height() > 0 { canvas.height() } else { FB_H };
    // framebuffer_x = (clientX - rect.left) * (fb_width / rect.width); same for y.
    // (clientX - rect.left) is the cursor's offset INTO the rendered canvas and
    // (fb / rect) is the rendered→framebuffer scale, so together they undo any
    // page offset AND any letterbox scaling.
    let fx = ((client_x - rect.left()) * (fb_w as f64 / rect_w)).clamp(0.0, (fb_w - 1) as f64) as i32;
    let fy = ((client_y - rect.top()) * (fb_h as f64 / rect_h)).clamp(0.0, (fb_h - 1) as f64) as i32;
    POINTER.with(|p| p.set((fx, fy)));
    forward_pointer_to_worker();
}

thread_local! {
    /// The bytes of the most-recently-launched inline cartridge, kept so the
    /// inline card's [fullscreen] button (`Action::RunInDisplay`) can relaunch
    /// the SAME cartridge into the fullscreen overlay. Session-lived; cleared
    /// on nothing (a new run overwrites it).
    static LAST_CARTRIDGE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Stash the most-recently-run cartridge so the inline card's [fullscreen]
/// button can relaunch the SAME bytes into the overlay (issue #52a).
pub(super) fn remember_last_cartridge(wasm: &[u8]) {
    LAST_CARTRIDGE.with(|c| *c.borrow_mut() = Some(wasm.to_vec()));
}

/// The inline card's [fullscreen] button: mount the display overlay and
/// relaunch the most-recently-run inline cartridge into it. No-op (just opens
/// an idle overlay) when nothing has run yet. Wired from `Action::RunInDisplay`.
pub(crate) async fn relaunch_last_in_fullscreen() {
    let Some(wasm) = LAST_CARTRIDGE.with(|c| c.borrow().clone()) else {
        // Nothing to relaunch — open an idle framebuffer surface instead.
        crate::app::opfs::toggle_display();
        return;
    };
    if let Err(e) = super::run_wasm(&wasm).await {
        embed_trace(&format!("fullscreen relaunch failed: {e:?}"));
    }
}

thread_local! {
    /// Cartridge bytes the `embed_app` tool fetched, waiting for the chat
    /// transcript to paint the `#embed-canvas` card so they can be launched
    /// into it. The tool can't draw the card itself (the `#tool-{id}-card`
    /// slot is filled by `chat::stream_turn` AFTER the tool returns), so it
    /// stashes the wasm here and the ToolResult handler drains it via
    /// [`launch_pending_embed`] once the canvas exists. NOT serialized into
    /// history — replay paints a marker card only (no bytes, like the display
    /// snapshot thumb).
    static PENDING_EMBED: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
}

/// Stash cartridge `wasm` for the next embed card to pick up. The
/// `embed_app` tool calls this just before returning its `{embedded:true}`
/// result; `launch_pending_embed` (run from the ToolResult handler) drains it.
pub(crate) fn stash_pending_embed(wasm: Vec<u8>) {
    PENDING_EMBED.with(|c| *c.borrow_mut() = Some(wasm));
}

thread_local! {
    /// Monotonic suffix for embed-canvas DOM ids. Every embed card —
    /// live OR history-replayed — must carry a UNIQUE canvas id: when all
    /// cards shared `#embed-canvas`, `by_id` resolved to the OLDEST card
    /// (often a dead replayed one at the top of the transcript), the
    /// cartridge launched into THAT, and the new card stayed a blank
    /// default-size canvas — the embed_app blank-render bug.
    static EMBED_CANVAS_SEQ: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// A fresh unique DOM id for an embed card's canvas (`embed-canvas-<n>`).
pub(crate) fn next_embed_canvas_id() -> String {
    EMBED_CANVAS_SEQ.with(|c| {
        let n = c.get().wrapping_add(1);
        c.set(n);
        format!("embed-canvas-{n}")
    })
}

/// If an `embed_app` tool stashed cartridge bytes AND the transcript has
/// painted its embed card, launch the cartridge into THAT CARD's canvas (the
/// inline interactive card). `card_id` is the `#tool-{id}-card` slot the card
/// just swapped into — scoping the lookup there (instead of a global id)
/// guarantees the cartridge lands in the card the user is looking at, not an
/// older embed's canvas. No-op when nothing is pending. Drains the stash
/// either way so a missing canvas can't leak bytes into a later embed.
pub(crate) async fn launch_pending_embed(card_id: &str) {
    let Some(wasm) = PENDING_EMBED.with(|c| c.borrow_mut().take()) else {
        embed_trace(&format!("no-stash for #{card_id}"));
        return;
    };
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    let Ok(Some(el)) = doc.query_selector(&format!("#{card_id} canvas.embed-app-canvas")) else {
        embed_trace(&format!("no-canvas inside #{card_id}"));
        return;
    };
    let Ok(canvas) = el.dyn_into::<HtmlCanvasElement>() else { return };
    match super::run_in_canvas(canvas, &wasm).await {
        Ok(()) => embed_trace(&format!("launched into #{card_id}")),
        Err(e) => embed_trace(&format!("run failed in #{card_id}: {e:?}")),
    }
}

/// Last embed-launch outcome, exposed at `window.__lhEmbedTrace` (and the
/// console) — the launch path's failure branches are otherwise silent in the
/// UI, and console capture is flaky under automation. One line, overwritten
/// per launch; costs nothing and makes "the embed is blank" diagnosable live.
fn embed_trace(msg: &str) {
    web_sys::console::warn_1(&JsValue::from_str(&format!("[embed] {msg}")));
    let _ = js_sys::Reflect::set(
        &js_sys::global(),
        &JsValue::from_str("__lhEmbedTrace"),
        &JsValue::from_str(msg),
    );
}

/// `true` if `id` is the DOM id of a cartridge canvas the delegated pointer
/// listeners should route input from — the fullscreen overlay `display-canvas`
/// or an inline `embed-canvas-<n>` (an `embed_app` card). Used by `events::mod`
/// to gate `set_pointer`/`set_pointer_down` on a pointer event's target.
pub(crate) fn is_cartridge_canvas_id(id: &str) -> bool {
    id == "display-canvas" || id.starts_with("embed-canvas")
}

/// `true` when a cartridge canvas is currently mounted (overlay OR an embed
/// card), so `mousemove`/`touchmove` know whether to bother updating the
/// poll-model pointer. Cheap DOM presence check (no worker query).
pub(crate) fn cartridge_canvas_present() -> bool {
    if dom::by_id("display-canvas").is_some() {
        return true;
    }
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.query_selector("canvas.embed-app-canvas").ok().flatten())
        .is_some()
}

/// Mount the display overlay (fullscreen, dismissable — the unified
/// stream's display surface) with a fresh canvas, then size + grab its
/// 2D context. Re-mounting over an already-open overlay just swaps in a
/// fresh surface, mirroring the old re-swap-the-panel behavior.
pub(super) fn mount_canvas() -> Result<CanvasRenderingContext2d, JsValue> {
    dom::swap_outer("display-overlay", &templates::display_overlay().into_string());
    size_and_get_ctx()
}

/// Snapshot the live `#display-canvas` as a PNG data URL — used by the
/// inline display card in the transcript. `None` when no canvas is mounted
/// or the encode fails. Cheap: the backing store is the cartridge's logical
/// framebuffer (512x512 default, up to 1024² for a `dims()`-declared
/// cartridge), so the PNG is at most a few hundred KB.
pub(crate) fn snapshot_data_url() -> Option<String> {
    let canvas = dom::by_id("display-canvas")?
        .dyn_into::<HtmlCanvasElement>()
        .ok()?;
    canvas.to_data_url().ok()
}

/// Size the existing `#display-canvas` backing store to the logical
/// framebuffer and return its 2D context. Assumes the canvas is already
/// in the DOM.
pub(super) fn size_and_get_ctx() -> Result<CanvasRenderingContext2d, JsValue> {
    let canvas = dom::by_id("display-canvas")
        .ok_or_else(|| JsValue::from_str("display-canvas missing"))?
        .dyn_into::<HtmlCanvasElement>()?;
    canvas.set_width(FB_W);
    canvas.set_height(FB_H);

    let ctx = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into::<CanvasRenderingContext2d>()?;
    Ok(ctx)
}

/// host_agent::broadcast_compose — swap the composer panel in over the
/// canvas so the presser can type a custom message before it goes out (a
/// cartridge is pixels-only; only a real `<input>` summons a mobile
/// keyboard). Focuses + selects the prefilled default so typing replaces it.
pub(super) fn open_broadcast_composer(title: &str, default_body: &str) {
    dom::swap_outer(
        "broadcast-composer",
        &templates::broadcast_composer(title, default_body).into_string(),
    );
    if let Some(input) = dom::by_id("broadcast-input")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        let _ = input.focus();
        input.select();
    }
}

/// True while the composer panel is swapped in (events uses this to route
/// Escape to the composer before the display overlay).
pub(crate) fn broadcast_composer_open() -> bool {
    dom::by_id("broadcast-composer")
        .map(|el| !el.has_attribute("hidden"))
        .unwrap_or(false)
}

/// The composer's [cancel] (and Escape): dismiss without sending.
pub(crate) fn close_broadcast_composer() {
    dom::swap_outer(
        "broadcast-composer",
        &templates::broadcast_composer_closed().into_string(),
    );
}

/// The composer's [send]: broadcast the typed body under `title` (the
/// cartridge-supplied title rides the button's `data-arg`), then close.
/// An emptied input still sends — the title alone is a valid ding.
pub(crate) fn broadcast_send(title: String) {
    let body: String = dom::by_id("broadcast-input")
        .and_then(|el| el.dyn_into::<web_sys::HtmlInputElement>().ok())
        .map(|i| i.value())
        .unwrap_or_default()
        .trim()
        .chars()
        .take(200)
        .collect();
    close_broadcast_composer();
    if title.is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(bridge::feed::do_feed_broadcast(title, body));
}
