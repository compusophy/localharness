//! DISPLAY — a pixel framebuffer surface that runs wasm cartridges.
//!
//! North star: Redox OS's **Orbital** display server. The canvas is the
//! screen (the scanout framebuffer); this module is the compositor /
//! display server; a wasm cartridge is an Orbital-style client app. The
//! `host_display` import module is the **Orbclient** analog — the draw
//! API a cartridge calls.
//!
//! ## Model: host-owned framebuffer + draw commands
//! A wasm module can't touch the canvas, GPU, or DOM — it only has linear
//! memory and the imports we grant it. So the host owns the framebuffer
//! and the cartridge issues draw commands into it, then flips it to the
//! screen. This fits rustlite (no arrays / raw memory needed — just
//! integer host calls), so an *agent-written* cartridge can draw.
//!
//! ## Cartridge ABI (`host_display`)
//! - `clear(rgb)` — fill the whole framebuffer (`0xRRGGBB`, opaque)
//! - `set_pixel(x, y, rgb)`
//! - `fill_rect(x, y, w, h, rgb)`
//! - `draw_char(x, y, codepoint, rgb, scale)` — one 5x7 glyph, scaled
//! - `draw_number(x, y, value, rgb, scale)` — a decimal integer
//! - `present()` — flush the framebuffer to the canvas
//! - `width() -> i32`, `height() -> i32`
//! - `pointer_x() -> i32`, `pointer_y() -> i32` — cursor position in
//!   framebuffer coordinates (poll model, like Orbclient's event queue)
//! - `pointer_down() -> i32` — 1 while the primary button is pressed
//! - `state_get(slot) -> i32`, `state_set(slot, value)` — a 64-slot
//!   integer register file that persists across `frame` calls (rustlite
//!   has no globals, so this is how a cartridge keeps state)
//!
//! ## Cartridge ABI (`host_audio`) — Web Audio playback (see `mod audio`)
//! Module-qualified spelling: call `audio::tone(...)`, NOT a bare `tone`.
//! Integer ABI, fire-and-forget like `host_net`; silent until the first
//! user gesture (browser AudioContext rule).
//! - `tone(freq_hz, dur_ms, wave) -> handle` — `wave`: 0 sine, 1 square,
//!   2 sawtooth, 3 triangle; returns a voice handle >= 0, or -1
//! - `tone_at(freq_hz, dur_ms, wave, delay_ms) -> handle` — schedule a
//!   tone `delay_ms` ahead (sequence a bar of notes from one `frame`)
//! - `noise(dur_ms) -> handle` — white-noise burst (hats / explosions)
//! - `stop(handle)` — stop one voice; `stop(-1)` stops every voice
//! - `set_volume(pct)` — master gain, `pct` clamped 0..=100
//!
//! A cartridge exports `memory` and either an animated `frame(t: i32)`
//! (driven by `requestAnimationFrame`, `t` = elapsed ms) or a one-shot
//! `render()`.
//!
//! The Closures here are the wasm↔host runtime bridge, not UI/DOM event
//! handling — a wasm import *must* be a JS function. They live only in
//! this module and never build DOM, so the app's "no imperative DOM"
//! rule is untouched.

use std::cell::{Cell, RefCell};

use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

use super::dom;
use super::templates;

/// DEFAULT logical framebuffer resolution. 16:9. A cartridge that does NOT
/// export `dims()` renders at this size (backward compatible). The canvas
/// backing store is sized to this initially; CSS scales it up with
/// `image-rendering: pixelated` so individual pixels stay crisp.
///
/// ## Variable resolution
/// A cartridge MAY export `dims() -> i32` returning a packed `(w << 16) | h`
/// (width high 16 bits, height low 16). The WORKER reads it after instantiate,
/// allocates a framebuffer of that size, and STAMPS every `frame` message with
/// the actual `w`/`h`. The main thread ([`worker::blit_frame`]) sizes the
/// canvas backing store from those per-frame dims and builds the `ImageData`
/// at `w`×`h` — so the host adapts to whatever the cartridge declared without
/// the main thread ever calling `dims()` itself. Dimensions are validated +
/// clamped to `[16, 1024]` in the worker; out of range falls back to the
/// default. These consts remain the default + the composition/HTML-render path
/// (single fixed surface).
const FB_W: u32 = 256;
const FB_H: u32 = 144;
const FB_BYTES: usize = (FB_W * FB_H * 4) as usize;

thread_local! {
    /// Generation counter for cartridge launches. Each launch bumps it so a
    /// stale launch's deferred work self-cancels when the global generation
    /// moves past the one it started with.
    static FRAME_GEN: Cell<u32> = const { Cell::new(0) };
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

/// Instantiate `wasm_bytes` as a display cartridge in the display
/// overlay (swaps in the overlay + surface). Used by the `run_cartridge`
/// tool and opening a `.wasm`/`.rl` from the files modal.
pub(crate) async fn run_wasm(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    let ctx = mount_canvas()?;
    run_with_ctx(wasm_bytes, ctx, "display-canvas").await
}

/// A cartridge run that never went live: the stable `LH1xxx` runtime code
/// (when the worker/watchdog produced one) + the human detail. What the
/// `run_cartridge` tool folds into its structured error result.
pub(crate) struct RunFailure {
    /// `LH1xxx` registry code (`error_codes::FRAME_TIMEOUT` etc); `None` for
    /// a spawn-level failure the worker never got to classify.
    pub code: Option<u16>,
    /// The detail string (worker error message / watchdog reason).
    pub detail: String,
}

/// How long [`run_wasm_reporting`] waits for the cartridge's FIRST lifecycle
/// signal before giving up. Must exceed the worker watchdog's window
/// (`WATCHDOG_MS` + one `WATCHDOG_TICK_MS` poll = 2000ms) so a first-frame
/// hang is reported as the watchdog's coded LH1001 kill, not as this
/// wrapper's vaguer timeout.
const FIRST_SIGNAL_MS: u32 = 2600;

/// Run `wasm_bytes` as a cartridge INLINE in the chat transcript (issue #52a)
/// rather than auto-opening the fullscreen overlay: the user strongly prefers
/// inline-by-default, fullscreen opt-in. This stashes the bytes for the
/// `run_cartridge` inline card (`launch_pending_embed`, the SAME path
/// `embed_app` uses) AND remembers them so the card's [fullscreen] button can
/// relaunch the SAME cartridge into the overlay. It does NOT mount the overlay.
///
/// `run_cartridge`'s "report the first frame" contract (issue #7) can't be
/// honoured before the card paints (the canvas doesn't exist yet), so this
/// returns `Ok(())` once the bytes are stashed; the inline launch surfaces a
/// dead/blank canvas if the cartridge fails, the same way an `embed_app` card
/// does. Fire-and-forget overlay callers (public-face boot, opening a file)
/// keep using `run_wasm` / `run_wasm_reporting`.
pub(crate) fn run_wasm_inline(wasm_bytes: &[u8]) {
    remember_last_cartridge(wasm_bytes);
    stash_pending_embed(wasm_bytes.to_vec());
}

/// Stash the most-recently-run cartridge so the inline card's [fullscreen]
/// button can relaunch the SAME bytes into the overlay (issue #52a).
fn remember_last_cartridge(wasm: &[u8]) {
    LAST_CARTRIDGE.with(|c| *c.borrow_mut() = Some(wasm.to_vec()));
}

thread_local! {
    /// The bytes of the most-recently-launched inline cartridge, kept so the
    /// inline card's [fullscreen] button (`Action::RunInDisplay`) can relaunch
    /// the SAME cartridge into the fullscreen overlay. Session-lived; cleared
    /// on nothing (a new run overwrites it).
    static LAST_CARTRIDGE: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
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
    if let Err(e) = run_wasm(&wasm).await {
        embed_trace(&format!("fullscreen relaunch failed: {e:?}"));
    }
}

/// [`run_wasm`], but AWAIT the cartridge's first lifecycle signal and report
/// it (issue #7): `Ok(())` once the first frame (or a one-shot `done`)
/// lands, `Err(RunFailure)` when the worker posts a coded fatal error
/// (instantiate failure / trap / missing entry) or the watchdog kills a hung
/// first frame. The old fire-and-forget `run_wasm` told the agent "running
/// on display" even when the canvas was painting "CARTRIDGE STOPPED".
///
/// A healthy cartridge posts its first frame within a few ms, so the await
/// costs success paths almost nothing; only failures wait (bounded by
/// [`FIRST_SIGNAL_MS`]). Fire-and-forget callers (public-face boot, opening
/// a file) keep using `run_wasm` — the overlay is their reporting surface.
pub(crate) async fn run_wasm_reporting(wasm_bytes: &[u8]) -> Result<(), RunFailure> {
    // issue #52a: `run_cartridge` now renders INLINE in the chat transcript by
    // default (the user strongly prefers inline-by-default), with fullscreen as
    // an opt-in [fullscreen] button on the inline card. So instead of mounting
    // the fullscreen overlay + awaiting the first frame here, stash the bytes
    // for the inline card to launch (the SAME `launch_pending_embed` path
    // `embed_app` uses) and remember them for the [fullscreen] relaunch. The
    // inline canvas surfaces a blank/dead frame on failure exactly like an
    // `embed_app` card, so the first-frame report (issue #7) is no longer the
    // success signal — the card IS the surface.
    run_wasm_inline(wasm_bytes);
    Ok(())
}

/// Mount the overlay + await the cartridge's first lifecycle signal — the
/// OVERLAY reporting path (issue #7), retained for callers that still want a
/// fullscreen run with a hard pass/fail (none ship today; kept so the
/// first-frame watchdog plumbing has a home and isn't dead code).
#[allow(dead_code)]
pub(crate) async fn run_wasm_reporting_fullscreen(wasm_bytes: &[u8]) -> Result<(), RunFailure> {
    run_wasm(wasm_bytes).await.map_err(|e| RunFailure {
        code: None,
        detail: format!("worker spawn failed: {e:?}"),
    })?;
    let mut waited = 0u32;
    loop {
        match worker::current_outcome() {
            worker::RunOutcome::Live => return Ok(()),
            worker::RunOutcome::Failed { code, detail } => {
                return Err(RunFailure { code, detail })
            }
            worker::RunOutcome::Pending => {}
        }
        if waited >= FIRST_SIGNAL_MS {
            // Shouldn't happen (the watchdog classifies a silent worker
            // first), but never hang the tool turn on a missing signal.
            return Err(RunFailure {
                code: None,
                detail: format!(
                    "no frame and no error within {FIRST_SIGNAL_MS}ms of spawning \
                     the cartridge worker"
                ),
            });
        }
        crate::runtime::sleep_ms(50).await;
        waited += 50;
    }
}

/// Instantiate `wasm_bytes` against an existing `#display-canvas`
/// already in the DOM (app mode — the subdomain booted straight into a
/// fullscreen cartridge, no overlay swap).
pub(crate) async fn run_in_root_canvas(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    let ctx = size_and_get_ctx()?;
    run_with_ctx(wasm_bytes, ctx, "display-canvas").await
}

/// THE EMBED SEAM: run `wasm_bytes` as a cartridge targeting ANY canvas in
/// the DOM (not just the fullscreen `#display-canvas` overlay) — what the
/// `embed_app` agent tool uses to render another subdomain's published
/// cartridge as a live, interactive card INLINE in the chat transcript.
/// `run_in_root_canvas` is the thin specialization (it just resolves
/// `#display-canvas` first); both funnel into the SAME `run_with_ctx` →
/// `mod worker` path, so an embed and the overlay share the single `WORKER`
/// slot.
///
/// ## v1 constraint: ONE cartridge at a time (single worker)
/// There is exactly one [`worker::WORKER`] slot, so starting a cartridge here
/// REPLACES any cartridge already running — the overlay's, or a prior embed's.
/// A second `embed_app` in the same transcript supersedes the first (the first
/// card goes inert: its canvas keeps its last painted frame but stops
/// updating). That's acceptable for v1 (one live interactive embed); true
/// concurrent embeds need a per-canvas worker registry, tracked as follow-up.
///
/// ## Variable framebuffer resolution
/// The canvas BACKING STORE is sized to the DEFAULT [`FB_W`]×[`FB_H`] (256×144)
/// here as an initial size, but it is RESIZED to the cartridge's actual dims
/// the moment its first `frame` message arrives ([`worker::blit_frame`] reads
/// the `w`/`h` the worker stamped on each frame and resizes the canvas + builds
/// the `ImageData` at `w`×`h`). A cartridge declares its size by exporting
/// `dims() -> i32` (packed `(w<<16)|h`); with no such export it stays 256×144
/// (backward compatible). CSS scales the canvas ELEMENT to the card box with
/// `image-rendering: pixelated`; aspect-preserving letterboxing comes from the
/// stylesheet's `max-width/height:100%` + `object-fit` on the canvas element.
pub(crate) async fn run_in_canvas(
    canvas: HtmlCanvasElement,
    wasm_bytes: &[u8],
) -> Result<(), JsValue> {
    // Initial backing store = the default; the worker's first frame resizes it
    // to the cartridge's declared dims (see `worker::blit_frame`). Sizing it
    // here avoids a 0×0 canvas flashing before the first frame lands.
    canvas.set_width(FB_W);
    canvas.set_height(FB_H);
    let id = canvas.id();
    let ctx = canvas
        .get_context("2d")?
        .ok_or_else(|| JsValue::from_str("no 2d context"))?
        .dyn_into::<CanvasRenderingContext2d>()?;
    run_with_ctx(wasm_bytes, ctx, &id).await
}

/// Render an HTML document into the framebuffer as pixels (no DOM, no
/// iframe) — the loader's "universal" path alongside `.wasm` cartridges.
/// A block-level subset (headings, paragraphs, lists, breaks) is laid out
/// with word-wrap and blitted via the bitmap font, monochrome. This is a
/// *snapshot* render: no CSS box model, images, colors, or scripts. Stops
/// any running cartridge first so its frame loop can't blit over the page.
pub(crate) fn render_html(source: &str) -> Result<(), JsValue> {
    stop();
    let ctx = mount_canvas()?;
    let blocks = html_to_blocks(source);
    let buf = paint_html_fb(&blocks);
    let img = ImageData::new_with_u8_clamped_array_and_sh(Clamped(&buf[..]), FB_W, FB_H)?;
    ctx.put_image_data(&img, 0.0, 0.0)?;
    Ok(())
}

/// Render an HTML document into the **root** `#display-canvas` (app mode
/// — the subdomain booted straight into a fullscreen public face), the
/// HTML counterpart of [`run_in_root_canvas`]. Same block-level snapshot
/// render as [`render_html`], just targeting the already-mounted canvas
/// instead of swapping in the workshop view panel.
pub(crate) fn render_html_in_root_canvas(source: &str) -> Result<(), JsValue> {
    stop();
    let ctx = size_and_get_ctx()?;
    let blocks = html_to_blocks(source);
    let buf = paint_html_fb(&blocks);
    let img = ImageData::new_with_u8_clamped_array_and_sh(Clamped(&buf[..]), FB_W, FB_H)?;
    ctx.put_image_data(&img, 0.0, 0.0)?;
    Ok(())
}

/// Shared core: run the cartridge OFF the main thread in a Web Worker so a
/// hung/unbounded `frame()` can't freeze the app, and start the watchdog that
/// terminates a worker which stops posting frames. The worker re-implements the
/// `host_display` ABI faithfully (`web/cartridge-worker.js`); the main thread
/// only blits the framebuffer it posts, feeds input, and plays forwarded audio.
///
/// This is the BRICK FIX: a cartridge persisted as the subdomain's public face
/// can no longer wedge the tab (chat included) — synchronous wasm is
/// un-preemptable from JS, so the only containment is "run it elsewhere + be
/// able to kill it", which the worker + watchdog provide. The `?compose=`
/// composition path (`mount_composition`) runs in the SAME worker (issue #77) —
/// a composed child is untrusted wasm too, so it must be contained off-thread.
async fn run_with_ctx(
    wasm_bytes: &[u8],
    ctx: CanvasRenderingContext2d,
    canvas_id: &str,
) -> Result<(), JsValue> {
    // Record which canvas this cartridge owns so the delegated pointer
    // listeners map client coords relative to ITS rect (overlay or an inline
    // embed). v1 single-worker: the most-recent launch wins both the worker
    // slot AND the pointer routing.
    ACTIVE_CANVAS_ID.with(|c| *c.borrow_mut() = canvas_id.to_string());
    // Bump the generation first so any previous launch's deferred work stops,
    // and tear down the previous worker (terminate + drop its closures).
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    worker::stop_worker();
    // Silence the prior cartridge's scheduled voices so a long note can't
    // drone into the new one.
    audio::stop_all();

    // Fresh cartridge starts with cleared input (the worker owns the 64-slot
    // state register file and zeroes it on load).
    POINTER_DOWN.with(|d| d.set(0));

    worker::spawn_cartridge(wasm_bytes, ctx)
}

/// Composite several published cartridges into ONE framebuffer, iframe-free —
/// the `?compose=name1,name2,…` path (roadmap Track A). Runs in the SAME isolated
/// Web Worker + main-thread watchdog as the single-cartridge path (issue #77): a
/// composed child is UNTRUSTED wasm too, so a hung `frame()` must only stall the
/// worker, never the main thread. This previously ran each child's `frame()`
/// DIRECTLY on the main thread (an in-thread `start_compose_loop`), which had no
/// isolation and re-bricked the tab.
///
/// `names` are the requested subdomains in order. The main thread lays them out
/// in a near-square grid via the native-tested [`crate::compose::grid_viewports`]
/// and hands the tiles to the worker, which mounts each as a compose-tree child
/// and resolves its published on-chain `app.wasm` through the EXISTING
/// `compose_spawn` / `compose_bytes` round-trip (a child that hasn't published an
/// app just stays a black cell). Admission stays capped by the worker's mirror of
/// [`crate::compose::ComposeBudget`] so an attacker-chosen graph can't exhaust it.
pub(crate) async fn mount_composition(names: Vec<String>) -> Result<(), JsValue> {
    let ctx = size_and_get_ctx()?;
    // Bump the generation so any previous launch's deferred work stops, and
    // record the overlay canvas so pointer events route to it. (The worker
    // teardown happens inside `spawn_composition` → `spawn_worker`.)
    ACTIVE_CANVAS_ID.with(|c| *c.borrow_mut() = "display-canvas".to_string());
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    // Silence any prior cartridge's scheduled voices on the shared engine.
    audio::stop_all();
    POINTER_DOWN.with(|d| d.set(0));

    if names.is_empty() {
        return Err(JsValue::from_str("compose: no module to composite"));
    }
    // Lay out a grid cell for EVERY requested name (native-tested layout), so the
    // worker mounts them in fixed positions; an unpublished name leaves its cell
    // black instead of shifting its siblings.
    let viewports = crate::compose::grid_viewports(names.len(), FB_W as i32, FB_H as i32);
    let slots: Vec<(String, crate::raster::Viewport)> =
        names.into_iter().zip(viewports).collect();
    worker::spawn_composition(slots, ctx)
}

/// Draw one 5x7 glyph into `buf` at `(x, y)`, each source pixel expanded
/// to a `scale`x`scale` block. Out-of-bounds pixels are clipped.
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
    // the fixed default. A 320×240 cartridge sees pointer coords in 320×240
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
    match run_in_canvas(canvas, &wasm).await {
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

// NOTE: the old single-cartridge in-thread `start_frame_loop` AND the in-thread
// `start_compose_loop` (the `?compose=` compositor) were removed — BOTH the
// single-cartridge path and the `?compose=` composition now run in a Web Worker
// (see `mod worker` + `web/cartridge-worker.js`) so a hung `frame()` can't freeze
// the main thread. The compose tree (recursion, budget caps, focus) lives in the
// worker; the main thread only blits frames, forwards input, and runs the
// watchdog. This closed issue #77 (untrusted compose wasm on the main thread).

/// Stop any running cartridge (e.g. when the surface is closed).
pub(crate) fn stop() {
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    // Terminate + drop the cartridge worker (every cartridge path — single,
    // embed, and `?compose=` — runs off-thread). Idempotent — a no-op when no
    // worker is live.
    worker::stop_worker();
    // Halt any voices already scheduled on the shared thread_local engine so a
    // swap never leaves a drone playing.
    audio::stop_all();
}

/// Mount the display overlay (fullscreen, dismissable — the unified
/// stream's display surface) with a fresh canvas, then size + grab its
/// 2D context. Re-mounting over an already-open overlay just swaps in a
/// fresh surface, mirroring the old re-swap-the-panel behavior.
fn mount_canvas() -> Result<CanvasRenderingContext2d, JsValue> {
    dom::swap_outer("display-overlay", &templates::display_overlay().into_string());
    size_and_get_ctx()
}

/// Snapshot the live `#display-canvas` as a PNG data URL — used by the
/// inline display card in the transcript. `None` when no canvas is mounted
/// or the encode fails. Cheap: the backing store is the cartridge's logical
/// framebuffer (256x144 default, up to 1024² for a `dims()`-declared
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
fn size_and_get_ctx() -> Result<CanvasRenderingContext2d, JsValue> {
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

// --- cartridge worker: off-main-thread containment ----------------------
//
// The single-cartridge path runs the UNTRUSTED cartridge wasm in a Web Worker
// (`web/cartridge-worker.js`) so a hung/unbounded `frame()` can only block the
// worker, never the main thread (chat / studio stay live). The worker posts a
// transferable framebuffer each frame; this module blits it to the canvas,
// forwards pointer input + plays forwarded audio, and runs the WATCHDOG that
// terminates a worker which stops posting frames — the actual hang defense
// (synchronous wasm is un-preemptable from JS, so "kill it" is the only cure).
//
// This is what un-bricks a subdomain whose persisted public-face cartridge
// loops forever: a previous build froze the whole tab on every reload; now the
// reload spawns a worker, the watchdog fires after WATCHDOG_MS, the worker is
// terminated, and an overlay invites a retry while the rest of the app works.
// =============================================================================
// host_agent feed bridge (the "Ready Up" loop, feedback #103)
//
// Cartridge feed calls cross the worker→main boundary as messages. WRITES
// (subscribe/unsubscribe/broadcast) are async on-chain / proxy ops the worker
// can't do; the main thread runs them and posts the refreshed context back so
// the cartridge's sync getters (`is_subscribed`, `subscriber_count`,
// `viewer_has_identity`) catch up next frame. The feed is THIS cartridge's own
// subdomain — there are no cross-subdomain feeds in v1.
// =============================================================================

/// The current cartridge's feed tokenId = the tenant subdomain's NFT id.
/// `None` off a tenant (apex / localhost) — a feed needs a subdomain.
async fn feed_token_id() -> Option<u64> {
    let name = crate::app::tenant::current_name()?;
    match crate::app::registry::id_of_name(&name).await {
        Ok(id) if id != 0 => Some(id),
        _ => None,
    }
}

/// Post a partial `agent_context` update to the worker (only the provided
/// fields). The worker's `applyAgentContext` merges them into its cached state.
fn post_agent_context(
    worker: &web_sys::Worker,
    has_identity: Option<bool>,
    is_subscribed: Option<bool>,
    subscriber_count: Option<u32>,
) {
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("agent_context"));
    if let Some(h) = has_identity {
        let _ = Reflect::set(&msg, &JsValue::from_str("viewerHasIdentity"), &JsValue::from_f64(if h { 1.0 } else { 0.0 }));
    }
    if let Some(sub) = is_subscribed {
        let _ = Reflect::set(&msg, &JsValue::from_str("feedIsSubscribed"), &JsValue::from_f64(if sub { 1.0 } else { 0.0 }));
    }
    if let Some(c) = subscriber_count {
        let _ = Reflect::set(&msg, &JsValue::from_str("feedSubscriberCount"), &JsValue::from_f64(c as f64));
    }
    let _ = worker.post_message(&msg);
}

/// Read the live feed context (subscribed? count? identity?) and post it to
/// the worker. Best-effort: any read failure leaves that field unsent.
async fn refresh_feed_context(worker: web_sys::Worker) {
    // Skip for an anonymous visitor (no wallet): they can't subscribe, and this
    // avoids 3 RPC reads on EVERY cartridge launch (the public RPC is
    // rate-limited — a likely source of launch slowness).
    let addr = crate::app::chat::credit_address_existing().await;
    if addr.is_none() {
        return;
    }
    let Some(feed_id) = feed_token_id().await else { return };
    let has_identity = addr.is_some();
    let is_sub = match &addr {
        Some(a) => crate::app::registry::is_subscribed(feed_id, a).await.unwrap_or(false),
        None => false,
    };
    let count = crate::app::registry::subscriber_count(feed_id).await.unwrap_or(0) as u32;
    post_agent_context(&worker, Some(has_identity), Some(is_sub), Some(count));
}

/// subscribe / unsubscribe the viewer to this feed (sponsored), then refresh.
async fn do_feed_subscribe(worker: web_sys::Worker, subscribe: bool) {
    let Some(feed_id) = feed_token_id().await else { return };
    let Some((signer, _)) = crate::app::chat::credit_signer().await else { return };
    let Ok(sponsor) = crate::app::sponsor::signer() else { return };
    let token = crate::app::registry::ALPHA_USD_ADDRESS();
    let res = if subscribe {
        crate::app::registry::subscribe_sponsored(&signer, &sponsor, feed_id, token).await
    } else {
        crate::app::registry::unsubscribe_sponsored(&signer, &sponsor, feed_id, token).await
    };
    if let Err(e) = res {
        web_sys::console::warn_1(&JsValue::from_str(&format!("feed subscribe: {e}")));
    } else if subscribe {
        // The SUBSCRIBE gesture is the right (and only) place to ask for
        // notification permission and register THIS device for Web Push — a
        // subscriber only ever RECEIVES a broadcast if their push subscription
        // is published under their OWN identity's MAIN (the proxy resolves
        // mainOf(subscriber) → push_sub). Removing this earlier left every
        // broadcast unreachable AND never prompted for permission, so nothing
        // ever buzzed. Best-effort: no-ops (silently) for a bare device key
        // with no MAIN tokenId to hang the subscription on.
        if crate::app::notifications::ensure_permission().await.unwrap_or(false) {
            publish_viewer_push_sub().await;
        }
    }
    refresh_feed_context(worker).await;
}

/// Register the VIEWER's Web Push subscription on-chain keyed by THEIR OWN
/// ADDRESS (`PushFacet.setPushSub`), signed by the viewer's credit key
/// (sponsored) — the slot `/api/broadcast` reads to reach this exact device.
/// Address-keyed so it works for ANY device, INCLUDING a bare device key with
/// no registered MAIN identity (the old MAIN-tokenId slot left such devices
/// unreachable — the cross-device-push bug). Permission already ensured by the
/// caller.
async fn publish_viewer_push_sub() {
    let Ok(sub_json) = crate::app::notifications::subscribe_push().await else { return };
    let Some((signer, _)) = crate::app::chat::credit_signer().await else { return };
    let Ok(sponsor) = crate::app::sponsor::signer() else { return };
    let token = crate::app::registry::ALPHA_USD_ADDRESS();
    if let Err(e) = crate::registry::set_push_sub_sponsored(
        &signer,
        &sponsor,
        sub_json.as_bytes(),
        token,
    )
    .await
    {
        web_sys::console::warn_1(&JsValue::from_str(&format!("publish push_sub: {e}")));
    }
}

/// THE READY UP: POST /api/broadcast so the proxy pushes to every subscriber.
async fn do_feed_broadcast(title: String, body: String) {
    let Some(feed_id) = feed_token_id().await else { return };
    let Some((signer, _)) = crate::app::chat::credit_signer().await else { return };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now);
    let url = format!(
        "{}api/broadcast",
        crate::registry::CREDIT_PROXY_URL
    );
    let payload = serde_json::json!({ "targetId": feed_id, "title": title, "body": body });
    let send = async {
        reqwest::Client::new()
            .post(&url)
            .header("content-type", "application/json")
            .header("x-goog-api-key", token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("broadcast request: {e}"))
    };
    match crate::app::net::with_timeout(20_000, send).await {
        Ok(Ok(_resp)) => {}
        Ok(Err(e)) => web_sys::console::warn_1(&JsValue::from_str(&format!("broadcast: {e}"))),
        Err(e) => web_sys::console::warn_1(&JsValue::from_str(&format!("broadcast timeout: {e}"))),
    }
    // Immediate LOCAL feedback for the presser, on BOTH surfaces the user asked
    // for: (1) the in-app header bell (always — no permission needed), and (2) an
    // OS notification (permission-gated). The proxy fan-out reaches OTHER
    // subscribers' phones via Web Push; this is the presser's own ding.
    crate::app::notifications::push_to_bell(&title, &body);
    if crate::app::notifications::ensure_permission().await.unwrap_or(false) {
        let _ = crate::app::notifications::show(&title, &body).await;
    }
    crate::app::notifications::vibrate(120);
}

/// host_agent::broadcast_compose — swap the composer panel in over the
/// canvas so the presser can type a custom message before it goes out (a
/// cartridge is pixels-only; only a real `<input>` summons a mobile
/// keyboard). Focuses + selects the prefilled default so typing replaces it.
fn open_broadcast_composer(title: &str, default_body: &str) {
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
    wasm_bindgen_futures::spawn_local(do_feed_broadcast(title, body));
}

/// Ensure the viewer has a wallet (credit_signer generates + persists a device
/// key if none), then refresh context so `viewer_has_identity` flips to 1.
async fn do_feed_request_identity(worker: web_sys::Worker) {
    let _ = crate::app::chat::credit_signer().await;
    refresh_feed_context(worker).await;
}

thread_local! {
    /// True while a cartridge that imports the host::agent FEED is running (set
    /// by the worker's `cartridge_uses_feed` message). Gates permission-priming
    /// so ONLY feed cartridges prompt — a plain game never asks for notifications.
    static FEED_CARTRIDGE_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// One-shot guard so priming fires at most once per cartridge load (each tap
    /// would otherwise re-attempt). Reset when a new feed cartridge loads.
    static FEED_PRIMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(crate) fn set_feed_cartridge_active(on: bool) {
    FEED_CARTRIDGE_ACTIVE.with(|c| c.set(on));
    if on {
        FEED_PRIMED.with(|c| c.set(false));
    }
}

/// Called from the main-thread CANVAS TAP (mousedown / touchstart on a cartridge
/// canvas) — the ONE real user gesture in the cartridge flow. The cartridge's
/// own `subscribe()` tap can't request notification permission (it arrives via a
/// worker postMessage with no user activation); THIS does, on the tap that
/// produced it. Once permission is granted it registers this device for Web Push
/// (address-keyed) so a READY-UP broadcast can reach it. Fires at most once per
/// cartridge load; no-ops for non-feed cartridges and after a hard deny.
pub(crate) fn prime_feed_permission_on_gesture() {
    if !FEED_CARTRIDGE_ACTIVE.with(|c| c.get()) || FEED_PRIMED.with(|c| c.get()) {
        return;
    }
    if matches!(
        web_sys::Notification::permission(),
        web_sys::NotificationPermission::Denied
    ) {
        return; // a denied site can't be re-prompted — needs a manual settings reset
    }
    FEED_PRIMED.with(|c| c.set(true));
    wasm_bindgen_futures::spawn_local(async {
        if crate::app::notifications::ensure_permission().await.unwrap_or(false) {
            publish_viewer_push_sub().await;
        } else {
            // permission not (yet) granted — allow a later tap to try again
            FEED_PRIMED.with(|c| c.set(false));
        }
    });
}

thread_local! {
    /// Session-lived cache of published child `app.wasm` bytes, keyed by name
    /// (host::compose). The worker can't do the on-chain registry read, so the
    /// main thread resolves the bytes for a `compose_spawn` and posts them back;
    /// repeat spawns of the same name reuse the cache instead of re-hitting the
    /// chain. Cache lifetime = the page session (a parent reload re-spawns from
    /// scratch); staleness only matters across an on-chain republish, which is a
    /// reload-scale event. (The compose-core `WasmCache` is content-addressed;
    /// this main-thread cache is name-keyed — the cheaper of the two for the
    /// "same name, same session" hit path.)
    static COMPOSE_WASM_CACHE: RefCell<std::collections::HashMap<String, Vec<u8>>> =
        RefCell::new(std::collections::HashMap::new());
}

/// host::compose main-thread half: the worker asked to mount `name` as a child
/// (handle already allocated worker-side in the LOADING state). Resolve that
/// subdomain's PUBLISHED on-chain `app.wasm` (cached per session) and post it
/// back as `compose_bytes`; the worker instantiates it into its slot. A
/// `wasm: null` reply marks the child FAILED (unregistered / no published app).
async fn do_compose_spawn(worker: web_sys::Worker, uid: i32, name: String) {
    // Cache hit → reuse; else fetch the published bytes and remember them.
    let cached = COMPOSE_WASM_CACHE.with(|c| c.borrow().get(&name).cloned());
    let bytes = match cached {
        Some(b) => Some(b),
        None => {
            let fetched = super::compose_module_wasm(&name).await;
            if let Some(ref b) = fetched {
                COMPOSE_WASM_CACHE.with(|c| {
                    c.borrow_mut().insert(name.clone(), b.clone());
                });
            }
            fetched
        }
    };
    post_compose_bytes(&worker, uid, bytes.as_deref());
}

/// Post a `compose_bytes` reply to the worker: the resolved child `app.wasm`
/// (transferred zero-copy) or `wasm: null` to mark the slot FAILED. Keyed by the
/// child's global `uid` (compose is a tree now — a flat handle can't address a
/// node nested under another node's table).
fn post_compose_bytes(worker: &web_sys::Worker, uid: i32, bytes: Option<&[u8]>) {
    use js_sys::{Object, Reflect, Uint8Array};
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("compose_bytes"));
    let _ = Reflect::set(&msg, &JsValue::from_str("uid"), &JsValue::from_f64(uid as f64));
    match bytes {
        Some(b) => {
            let arr = Uint8Array::from(b);
            let buf = arr.buffer();
            let _ = Reflect::set(&msg, &JsValue::from_str("wasm"), &buf);
            // Transfer the ArrayBuffer (zero-copy); instantiate copies it worker-side.
            let transfer = js_sys::Array::new();
            transfer.push(&buf);
            let _ = worker.post_message_with_transfer(&msg, &transfer);
        }
        None => {
            let _ = Reflect::set(&msg, &JsValue::from_str("wasm"), &JsValue::NULL);
            let _ = worker.post_message(&msg);
        }
    }
}

/// host_http main-thread half (issue #19): the worker asked to GET `url` (the
/// cartridge has no auth signer + can't fetch cross-origin). Run the SAME authed
/// `/api/fetch` proxy POST the `web_fetch` tool uses (signed token, https-only,
/// private hosts denied, 200KB cap, textual content only), then post an
/// `http_result { id, status, body }` (or `{ id, error:true }`) back so the
/// worker's poll-model handle flips READY/ERROR. Mirrors `do_compose_spawn`.
async fn do_http_fetch(worker: web_sys::Worker, id: i32, url: String) {
    // A FRESH per-request proxy token (same scheme as web_fetch). No identity =>
    // the cartridge can't fetch; mark the handle errored.
    let Some((signer, _)) = crate::app::chat::credit_signer().await else {
        post_http_result(&worker, id, None);
        return;
    };
    let now = (js_sys::Date::now() / 1000.0) as u64;
    let token = crate::registry::proxy_auth_token(&signer, now);
    let endpoint = format!(
        "{}api/fetch",
        crate::registry::CREDIT_PROXY_URL
    );
    let send = async {
        let resp = reqwest::Client::new()
            .post(&endpoint)
            .header("content-type", "application/json")
            .header("x-goog-api-key", token)
            .json(&serde_json::json!({ "url": url }))
            .send()
            .await
            .map_err(|e| format!("proxy request: {e}"))?;
        let status = resp.status();
        let body = resp
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("proxy response decode: {e}"))?;
        Ok::<_, String>((status, body))
    };
    match crate::app::net::with_timeout(20_000, send).await {
        Ok(Ok((status, body))) if status.is_success() => {
            // `/api/fetch` returns { status, contentType, truncated, body } for a
            // textual hit, or { status, contentType, note } for binary (no body).
            let upstream = body.get("status").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let text = body.get("body").and_then(|v| v.as_str()).unwrap_or("");
            post_http_result_ok(&worker, id, upstream, text);
        }
        _ => post_http_result(&worker, id, None),
    }
}

/// Post a successful `http_result` to the worker (upstream status + body text).
fn post_http_result_ok(worker: &web_sys::Worker, id: i32, status: i32, body: &str) {
    let msg = Object::new();
    let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("http_result"));
    let _ = Reflect::set(&msg, &JsValue::from_str("id"), &JsValue::from_f64(id as f64));
    let _ = Reflect::set(&msg, &JsValue::from_str("status"), &JsValue::from_f64(status as f64));
    let _ = Reflect::set(&msg, &JsValue::from_str("body"), &JsValue::from_str(body));
    let _ = worker.post_message(&msg);
}

/// Post an `http_result` to the worker. `None` => mark the request ERRORED
/// (`error: true`); the worker's `ready(handle)` then returns -2.
fn post_http_result(worker: &web_sys::Worker, id: i32, ok: Option<(i32, &str)>) {
    match ok {
        Some((status, body)) => post_http_result_ok(worker, id, status, body),
        None => {
            let msg = Object::new();
            let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("http_result"));
            let _ = Reflect::set(&msg, &JsValue::from_str("id"), &JsValue::from_f64(id as f64));
            let _ = Reflect::set(&msg, &JsValue::from_str("error"), &JsValue::TRUE);
            let _ = worker.post_message(&msg);
        }
    }
}

mod worker {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use js_sys::{ArrayBuffer, Object, Reflect, Uint8Array, Uint8ClampedArray};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::{Clamped, JsCast};
    use web_sys::{CanvasRenderingContext2d, ImageData, MessageEvent, Worker};

    use super::dom;
    use super::{audio, FB_H, FB_W};

    /// No-frame timeout: if the worker doesn't post a frame within this many
    /// ms, the watchdog treats the cartridge as hung and terminates it. ~1.5s
    /// is well past a normal frame (16ms) yet short enough that a hang is
    /// obvious. Re-spawned workers each get the full window.
    const WATCHDOG_MS: f64 = 1500.0;
    /// How often the watchdog checks the last-frame timestamp.
    const WATCHDOG_TICK_MS: i32 = 500;

    thread_local! {
        /// The live cartridge worker + its kept-alive closures + watchdog
        /// handle. Replaced on the next cartridge load (terminating the prior
        /// one) and cleared on `stop`/hang.
        static WORKER: RefCell<Option<WorkerHandle>> = const { RefCell::new(None) };
        /// Spawn generation — bumped by every `spawn_cartridge` so a LATE
        /// message from a torn-down worker (its onmessage may already be
        /// queued when we terminate it) can't write an outcome that belongs
        /// to the previous run.
        static RUN_GEN: Cell<u32> = const { Cell::new(0) };
        /// The FIRST lifecycle outcome of the current spawn (issue #7: the
        /// run_cartridge tool used to return "running on display" no matter
        /// what — instantiate failures / traps / watchdog kills only reached
        /// the console + overlay, so the agent saw success on a dead run).
        /// `await_first_outcome` polls this to report the truth instead.
        static RUN_OUTCOME: RefCell<RunOutcome> = const { RefCell::new(RunOutcome::Pending) };
    }

    /// The first lifecycle signal a spawned cartridge produced.
    #[derive(Clone)]
    pub(super) enum RunOutcome {
        /// Nothing posted yet (still instantiating / first frame in flight).
        Pending,
        /// A frame (or a one-shot `done`) arrived — the cartridge is live.
        Live,
        /// A fatal error: the worker's coded `{type:'error'}` message, or the
        /// watchdog kill (`LH1001`). `code` is an `LH1xxx` registry value.
        Failed { code: Option<u16>, detail: String },
    }

    /// Record the FIRST outcome for generation `gen` (later signals and
    /// stale-generation writes are ignored — the first frame/error is the
    /// truth the tool result wants).
    fn record_outcome(generation: u32, outcome: RunOutcome) {
        if RUN_GEN.with(|g| g.get()) != generation {
            return;
        }
        RUN_OUTCOME.with(|o| {
            let mut o = o.borrow_mut();
            if matches!(*o, RunOutcome::Pending) {
                // Auto-report a cartridge FAILURE off-chain (the LH1xxx code +
                // detail + the usual rich context), once per run, gated by the
                // telemetry toggle. Worker traps (LH1002–1004) and the watchdog
                // kill (LH1001) both funnel through here, so this is the one hook
                // — and the Pending guard makes it fire at most once per run.
                if let RunOutcome::Failed { code, detail } = &outcome {
                    if crate::app::telemetry::enabled() {
                        let code = *code;
                        let detail = detail.clone();
                        let fp: String = detail
                            .chars()
                            .filter(|c| c.is_ascii_alphanumeric())
                            .take(40)
                            .collect();
                        let signature = format!("cartridge-{fp}");
                        let title = format!(
                            "cartridge failed: {}",
                            detail.chars().take(100).collect::<String>()
                        );
                        wasm_bindgen_futures::spawn_local(crate::app::telemetry::report_event(
                            "cartridge".to_string(),
                            code,
                            title,
                            signature,
                            detail,
                            String::new(),
                        ));
                    }
                }
                *o = outcome;
            }
        });
    }

    /// The current spawn's first outcome so far (clone — cheap).
    pub(super) fn current_outcome() -> RunOutcome {
        RUN_OUTCOME.with(|o| o.borrow().clone())
    }

    /// Everything that must outlive a running worker: the `Worker` itself, the
    /// `onmessage` closure (JS holds a reference into it), the watchdog interval
    /// id + its callback closure, and the `terminated` flag the watchdog/stop
    /// path flip so an in-flight tick is a no-op. (The last-frame timestamp is
    /// owned by the onmessage + watchdog closures via their own `Rc` clones —
    /// it doesn't need a struct slot.)
    ///
    /// The watchdog interval id lives in a SHARED [`Rc<Cell<Option<i32>>>`] —
    /// the watchdog callback `take()`s it when it self-clears (on a hang), the
    /// `done` handler `take()`s it when a one-shot render disarms it, and `Drop`
    /// clears whatever id is still present. Exactly one of those clears the
    /// interval; the others see `None`. (A bare `Option<i32>` here was the leak:
    /// `stop_worker` set `terminated` BEFORE the drop, so `Drop` wrongly assumed
    /// the watchdog had self-cleared and skipped the clear — leaving a live
    /// interval whose `Closure` was just dropped, an invoke-after-drop hazard.)
    struct WorkerHandle {
        worker: Worker,
        _onmessage: Closure<dyn FnMut(MessageEvent)>,
        watchdog: Rc<Cell<Option<i32>>>,
        _watchdog_cb: Option<Closure<dyn FnMut()>>,
        terminated: Rc<Cell<bool>>,
    }

    impl Drop for WorkerHandle {
        fn drop(&mut self) {
            // Clear the watchdog interval if it's still armed. Whoever cleared
            // it first (the watchdog's hang self-clear, or the one-shot `done`
            // disarm) `take()`s the id to `None`, so this is a no-op in those
            // cases and never double-clears a recycled id. terminate() is
            // idempotent.
            if let Some(id) = self.watchdog.take() {
                if let Ok(win) = dom::window() {
                    win.clear_interval_with_handle(id);
                }
            }
            self.worker.terminate();
        }
    }

    /// Spawn a worker for a SINGLE `wasm_bytes` cartridge, wire its message
    /// handler to the canvas `ctx`, post the cartridge, and arm the watchdog.
    /// Replaces any previous worker (its `Drop` terminates it + clears its
    /// interval).
    pub(super) fn spawn_cartridge(
        wasm_bytes: &[u8],
        ctx: CanvasRenderingContext2d,
    ) -> Result<(), JsValue> {
        let bytes = wasm_bytes.to_vec();
        spawn_worker(ctx, move |worker| {
            // Post the wasm. `instantiate` copies the bytes, so we don't need to
            // transfer ownership of this buffer.
            let arr = Uint8Array::from(&bytes[..]);
            let msg = Object::new();
            Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("load"))?;
            Reflect::set(&msg, &JsValue::from_str("wasm"), &arr.buffer())?;
            attach_viewer_context(&msg)?;
            worker
                .post_message(&msg)
                .map_err(|e| JsValue::from_str(&format!("worker post failed: {e:?}")))
        })
    }

    /// Spawn a worker for a ROOTLESS `?compose=` composition (issue #77): the
    /// grid-laid-out named modules run in the SAME isolated worker + watchdog as
    /// a single cartridge, so a hung child can't freeze the tab. `slots` carries
    /// `(name, x, y, w, h)` viewport tiles the main thread computed via
    /// [`crate::compose::grid_viewports`]; the worker mounts each as a compose
    /// child and resolves its on-chain `app.wasm` through the SAME
    /// `compose_spawn` / `compose_bytes` round-trip a recursive spawn uses.
    pub(super) fn spawn_composition(
        slots: Vec<(String, crate::raster::Viewport)>,
        ctx: CanvasRenderingContext2d,
    ) -> Result<(), JsValue> {
        spawn_worker(ctx, move |worker| {
            let arr = js_sys::Array::new();
            for (name, vp) in &slots {
                let s = Object::new();
                Reflect::set(&s, &JsValue::from_str("name"), &JsValue::from_str(name))?;
                Reflect::set(&s, &JsValue::from_str("x"), &JsValue::from_f64(vp.ox as f64))?;
                Reflect::set(&s, &JsValue::from_str("y"), &JsValue::from_f64(vp.oy as f64))?;
                Reflect::set(&s, &JsValue::from_str("w"), &JsValue::from_f64(vp.w as f64))?;
                Reflect::set(&s, &JsValue::from_str("h"), &JsValue::from_f64(vp.h as f64))?;
                arr.push(&s);
            }
            let msg = Object::new();
            Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("compose_load"))?;
            Reflect::set(&msg, &JsValue::from_str("slots"), &arr)?;
            attach_viewer_context(&msg)?;
            worker
                .post_message(&msg)
                .map_err(|e| JsValue::from_str(&format!("worker post failed: {e:?}")))
        })
    }

    /// Stamp the load message with this device's viewer context (verified owner?
    /// has a wallet?) for `host_agent`. Both read sync from APP state — cheap, no
    /// RPC. A cartridge gates host-only controls on `viewer_is_owner`.
    fn attach_viewer_context(msg: &Object) -> Result<(), JsValue> {
        let (is_owner, has_identity) = crate::app::APP.with(|c| {
            let app = c.borrow();
            (
                matches!(app.verify_state, crate::app::VerifyState::Verified { .. }),
                app.wallet.is_some(),
            )
        });
        Reflect::set(
            msg,
            &JsValue::from_str("viewerIsOwner"),
            &JsValue::from_f64(if is_owner { 1.0 } else { 0.0 }),
        )?;
        Reflect::set(
            msg,
            &JsValue::from_str("viewerHasIdentity"),
            &JsValue::from_f64(if has_identity { 1.0 } else { 0.0 }),
        )?;
        Ok(())
    }

    /// Spawn + wire a cartridge worker against canvas `ctx`, run `post_load` to
    /// send the initial work (a single `load` or a `compose_load`), and arm the
    /// watchdog. The message handler, watchdog, and teardown are IDENTICAL for
    /// both the single-cartridge and `?compose=` paths — only the load message
    /// differs — so both share this core. Replaces any previous worker (its
    /// `Drop` terminates it + clears its interval).
    fn spawn_worker(
        ctx: CanvasRenderingContext2d,
        post_load: impl FnOnce(&Worker) -> Result<(), JsValue>,
    ) -> Result<(), JsValue> {
        // Tear down the previous worker first (idempotent).
        stop_worker();

        // New spawn generation: reset the first-outcome slot so this run's
        // signals (and only this run's) populate it.
        let run_gen = RUN_GEN.with(|g| {
            let n = g.get().wrapping_add(1);
            g.set(n);
            n
        });
        RUN_OUTCOME.with(|o| *o.borrow_mut() = RunOutcome::Pending);

        let worker = Worker::new(&worker_url())
            .map_err(|e| JsValue::from_str(&format!("worker spawn failed: {e:?}")))?;

        let last_frame = Rc::new(Cell::new(js_sys::Date::now()));
        let terminated = Rc::new(Cell::new(false));
        // Shared watchdog interval id. `arm_watchdog` fills it; the watchdog
        // self-clears it on a hang, the `done` handler disarms it after a
        // one-shot render completes, and `WorkerHandle::Drop` clears whatever
        // remains. `take()` makes exactly one of those the real clear.
        let watchdog_id: Rc<Cell<Option<i32>>> = Rc::new(Cell::new(None));

        // Message handler: blit frames, play forwarded audio, surface errors.
        let onmessage = {
            let ctx = ctx.clone();
            let last_frame = last_frame.clone();
            let watchdog_id = watchdog_id.clone();
            let worker_for_msg = worker.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let data = e.data();
                let ty = Reflect::get(&data, &JsValue::from_str("type"))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                match ty.as_str() {
                    "frame" => {
                        last_frame.set(js_sys::Date::now());
                        record_outcome(run_gen, RunOutcome::Live);
                        blit_frame(&data, &ctx);
                    }
                    "audio" => handle_audio(&data),
                    "error" => {
                        let detail = Reflect::get(&data, &JsValue::from_str("detail"))
                            .ok()
                            .and_then(|v| v.as_string())
                            .unwrap_or_default();
                        // The worker tags each fatal error with a stable LH1xxx
                        // runtime code (trap / instantiate / no-entry); paint it
                        // into the overlay so the canvas shows the coded reason
                        // instead of just going dark. The watchdog handles the
                        // hang code (LH1001) on its own path — DISARM it here
                        // (same one-shot `take()` as the `done` arm), or it
                        // fires ~1.5s later (no frames after a fatal error) and
                        // repaints the coded overlay as a false LH1001.
                        if let Some(id) = watchdog_id.take() {
                            if let Ok(win) = dom::window() {
                                win.clear_interval_with_handle(id);
                            }
                        }
                        let code = Reflect::get(&data, &JsValue::from_str("code"))
                            .ok()
                            .and_then(|v| v.as_f64())
                            .map(|n| n as u16);
                        // Surface the failure to the agent too: the
                        // run_cartridge tool awaits this outcome, so the
                        // coded reason lands in the TOOL RESULT instead of
                        // only the canvas overlay + console (issue #7).
                        record_outcome(
                            run_gen,
                            RunOutcome::Failed { code, detail: detail.clone() },
                        );
                        if let Some(code) = code {
                            paint_stopped_overlay_coded(&ctx, code);
                        }
                        web_sys::console::warn_1(&JsValue::from_str(&format!(
                            "cartridge error{}: {detail}",
                            code.map(|c| format!(" {}", crate::error_codes::fmt_label(c)))
                                .unwrap_or_default()
                        )));
                    }
                    "log" => {
                        let msg = Reflect::get(&data, &JsValue::from_str("msg"))
                            .ok()
                            .and_then(|v| v.as_string())
                            .unwrap_or_default();
                        web_sys::console::log_1(&JsValue::from_str(&msg));
                    }
                    // host_agent::notify — a cartridge asked to buzz the viewer.
                    // Permission-GATED (never prompts: `show` only renders if
                    // already granted, and we skip entirely otherwise) and the
                    // worker already rate-limited. Local to THIS viewer only.
                    "agent_notify" => {
                        if matches!(
                            web_sys::Notification::permission(),
                            web_sys::NotificationPermission::Granted
                        ) {
                            let title = Reflect::get(&data, &JsValue::from_str("title"))
                                .ok()
                                .and_then(|v| v.as_string())
                                .unwrap_or_default();
                            let body = Reflect::get(&data, &JsValue::from_str("body"))
                                .ok()
                                .and_then(|v| v.as_string())
                                .unwrap_or_default();
                            if !title.is_empty() {
                                wasm_bindgen_futures::spawn_local(async move {
                                    let _ = crate::app::notifications::show(&title, &body).await;
                                });
                            }
                        }
                    }
                    // The running cartridge imports the host::agent feed surface
                    // → arm permission-priming so the NEXT canvas tap (a real
                    // main-thread gesture) can request notification permission,
                    // which the cartridge's own subscribe() tap cannot.
                    "cartridge_uses_feed" => super::set_feed_cartridge_active(true),
                    "agent_subscribe" => {
                        let w = worker_for_msg.clone();
                        wasm_bindgen_futures::spawn_local(super::do_feed_subscribe(w, true));
                    }
                    "agent_unsubscribe" => {
                        let w = worker_for_msg.clone();
                        wasm_bindgen_futures::spawn_local(super::do_feed_subscribe(w, false));
                    }
                    "agent_broadcast" => {
                        let title = Reflect::get(&data, &JsValue::from_str("title"))
                            .ok().and_then(|v| v.as_string()).unwrap_or_default();
                        let body = Reflect::get(&data, &JsValue::from_str("body"))
                            .ok().and_then(|v| v.as_string()).unwrap_or_default();
                        if !title.is_empty() {
                            wasm_bindgen_futures::spawn_local(super::do_feed_broadcast(title, body));
                        }
                    }
                    // The cartridge wants a CUSTOM broadcast message — open the
                    // host-side text input over the canvas (the cartridge can't
                    // summon a keyboard from pixels). [send] runs the same
                    // do_feed_broadcast as agent_broadcast, with the typed body.
                    "agent_broadcast_compose" => {
                        let title = Reflect::get(&data, &JsValue::from_str("title"))
                            .ok().and_then(|v| v.as_string()).unwrap_or_default();
                        let body = Reflect::get(&data, &JsValue::from_str("body"))
                            .ok().and_then(|v| v.as_string()).unwrap_or_default();
                        if !title.is_empty() {
                            super::open_broadcast_composer(&title, &body);
                        }
                    }
                    "agent_request_identity" => {
                        let w = worker_for_msg.clone();
                        wasm_bindgen_futures::spawn_local(super::do_feed_request_identity(w));
                    }
                    // host::compose — a cartridge ANYWHERE in the compose tree
                    // spawned a child. The worker can't read the on-chain
                    // registry, so it posted the child's name + its global uid
                    // here; resolve the published app.wasm on the MAIN thread and
                    // post the bytes back (or a FAILED signal). The worker
                    // instantiates it into the matching node.
                    "compose_spawn" => {
                        let uid = Reflect::get(&data, &JsValue::from_str("uid"))
                            .ok().and_then(|v| v.as_f64()).map(|n| n as i32).unwrap_or(-1);
                        let name = Reflect::get(&data, &JsValue::from_str("name"))
                            .ok().and_then(|v| v.as_string()).unwrap_or_default();
                        if uid >= 0 && !name.is_empty() {
                            let w = worker_for_msg.clone();
                            wasm_bindgen_futures::spawn_local(super::do_compose_spawn(w, uid, name));
                        }
                    }
                    // host::http — a cartridge called http::get. The worker can't
                    // sign the proxy token or fetch cross-origin, so it posted the
                    // url + a global id here; run the authed /api/fetch proxy POST
                    // on the MAIN thread and post the body back as `http_result`.
                    "http_fetch" => {
                        let id = Reflect::get(&data, &JsValue::from_str("id"))
                            .ok().and_then(|v| v.as_f64()).map(|n| n as i32).unwrap_or(-1);
                        let url = Reflect::get(&data, &JsValue::from_str("url"))
                            .ok().and_then(|v| v.as_string()).unwrap_or_default();
                        if id >= 0 && !url.is_empty() {
                            let w = worker_for_msg.clone();
                            wasm_bindgen_futures::spawn_local(super::do_http_fetch(w, id, url));
                        }
                    }
                    "done" => {
                        record_outcome(run_gen, RunOutcome::Live);
                        // A one-shot `render()` finished and posted its single
                        // frame — it can't hang now (it already returned), so
                        // DISARM the watchdog. Otherwise it would fire ~1.5s
                        // later and falsely paint "CARTRIDGE STOPPED LH1001"
                        // over a perfectly-good static render. The worker stays
                        // alive (a re-load reuses it). Animated cartridges never
                        // send `done`, so they keep the watchdog.
                        if let Some(id) = watchdog_id.take() {
                            if let Ok(win) = dom::window() {
                                win.clear_interval_with_handle(id);
                            }
                        }
                    }
                    _ => {}
                }
            })
        };
        worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

        // Post the initial work — a single `load` or a `compose_load` — both
        // already stamped with the viewer context via `attach_viewer_context`.
        post_load(&worker)?;

        // Best-effort: resolve the live feed context (subscribed? count?) and
        // post it so host_agent::is_subscribed / subscriber_count reflect
        // reality a beat after launch. Off a tenant this is a no-op.
        {
            let w = worker.clone();
            wasm_bindgen_futures::spawn_local(super::refresh_feed_context(w));
        }

        // Arm the watchdog: terminate the worker if no frame lands in time.
        let watchdog_cb = arm_watchdog(
            worker.clone(),
            ctx,
            last_frame.clone(),
            terminated.clone(),
            watchdog_id.clone(),
            run_gen,
        );

        WORKER.with(|cell| {
            *cell.borrow_mut() = Some(WorkerHandle {
                worker,
                _onmessage: onmessage,
                watchdog: watchdog_id,
                _watchdog_cb: watchdog_cb,
                terminated,
            });
        });
        Ok(())
    }

    /// Terminate + drop the current worker (clears its watchdog). Idempotent.
    pub(super) fn stop_worker() {
        WORKER.with(|cell| {
            // Mark terminated so an in-flight watchdog tick is a no-op, then
            // drop the handle (its `Drop` terminates the worker + clears the
            // interval).
            if let Some(h) = cell.borrow().as_ref() {
                h.terminated.set(true);
            }
            *cell.borrow_mut() = None;
        });
    }

    /// Forward the latest pointer to the worker (poll model). No-op if no
    /// worker is live.
    pub(super) fn post_input(x: i32, y: i32, down: i32) {
        WORKER.with(|cell| {
            if let Some(h) = cell.borrow().as_ref() {
                let msg = Object::new();
                let _ = Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("input"));
                let _ = Reflect::set(&msg, &JsValue::from_str("x"), &JsValue::from_f64(x as f64));
                let _ = Reflect::set(&msg, &JsValue::from_str("y"), &JsValue::from_f64(y as f64));
                let _ = Reflect::set(&msg, &JsValue::from_str("down"), &JsValue::from_f64(down as f64));
                let _ = h.worker.post_message(&msg);
            }
        });
    }

    /// `true` while a cartridge worker is live AND not terminated (used by the
    /// input handlers to decide whether to forward pointer events). A hung
    /// worker the watchdog killed reports `false` even though its inert handle
    /// is still in the slot.
    pub(super) fn is_active() -> bool {
        WORKER.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|h| !h.terminated.get())
                .unwrap_or(false)
        })
    }

    /// Resolve the worker script URL relative to the page. `web/cartridge-
    /// worker.js` ships next to `index.html`, so an absolute `/cartridge-
    /// worker.js` resolves on apex, tenants, and Vercel previews alike.
    fn worker_url() -> String {
        "/cartridge-worker.js".to_string()
    }

    /// Blit a `{ type:'frame', fb:ArrayBuffer, w, h }` message to the canvas.
    /// The framebuffer is RGBA8888 already in the worker's packing
    /// (0xAABBGGRR little-endian == ImageData byte order R,G,B,A).
    ///
    /// VARIABLE RESOLUTION: the worker stamps every frame with the cartridge's
    /// actual `w`/`h` (its `dims()` export, or the 256×144 default). We size the
    /// canvas backing store to those dims (idempotent: a no-op once it matches,
    /// so steady-state frames don't thrash) and build the `ImageData` at `w`×`h`
    /// — the canvas adapts to the cartridge's chosen size on the FIRST frame.
    /// Falls back to [`FB_W`]/[`FB_H`] if the message omits/garbles the dims.
    fn blit_frame(data: &JsValue, ctx: &CanvasRenderingContext2d) {
        let Ok(fb) = Reflect::get(data, &JsValue::from_str("fb")) else { return };
        let Ok(buffer) = fb.dyn_into::<ArrayBuffer>() else { return };
        let w = Reflect::get(data, &JsValue::from_str("w"))
            .ok()
            .and_then(|v| v.as_f64())
            .map(|n| n as u32)
            .filter(|&n| n > 0)
            .unwrap_or(FB_W);
        let h = Reflect::get(data, &JsValue::from_str("h"))
            .ok()
            .and_then(|v| v.as_f64())
            .map(|n| n as u32)
            .filter(|&n| n > 0)
            .unwrap_or(FB_H);
        let clamped = Uint8ClampedArray::new(&buffer);
        // ImageData wants a &Clamped<&[u8]>; copy the transferred buffer out.
        let mut bytes = vec![0u8; clamped.length() as usize];
        clamped.copy_to(&mut bytes[..]);
        // Resize the canvas backing store to the cartridge's framebuffer dims
        // (setting width/height to the same value is cheap; only a real change
        // reallocates + clears). The CSS keeps the element aspect-scaled.
        let canvas = ctx.canvas();
        if let Some(canvas) = canvas {
            if canvas.width() != w {
                canvas.set_width(w);
            }
            if canvas.height() != h {
                canvas.set_height(h);
            }
        }
        if let Ok(img) =
            ImageData::new_with_u8_clamped_array_and_sh(Clamped(&bytes[..]), w, h)
        {
            let _ = ctx.put_image_data(&img, 0.0, 0.0);
        }
    }

    /// Play a `{ type:'audio', op, args }` message on the main-thread audio
    /// engine (AudioContext can't run in a worker). Mirrors the `host_audio`
    /// ABI; the return handle is dropped (the worker tracks its own local
    /// handles).
    fn handle_audio(data: &JsValue) {
        let op = Reflect::get(data, &JsValue::from_str("op"))
            .ok()
            .and_then(|v| v.as_string())
            .unwrap_or_default();
        let args = Reflect::get(data, &JsValue::from_str("args")).unwrap_or(JsValue::NULL);
        let arg = |i: u32| -> i32 {
            Reflect::get_u32(&args, i)
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as i32
        };
        match op.as_str() {
            "tone" => { audio::play_tone(arg(0), arg(1), arg(2), 0); }
            "tone_at" => { audio::play_tone(arg(0), arg(1), arg(2), arg(3)); }
            "noise" => { audio::play_noise(arg(0)); }
            "stop" => audio::stop_handle(arg(0)),
            "set_volume" => audio::set_master_volume(arg(0)),
            _ => {}
        }
    }

    /// Arm a polling watchdog: every `WATCHDOG_TICK_MS`, if the last frame is
    /// older than `WATCHDOG_MS`, the cartridge is hung → terminate the worker,
    /// paint a "stopped" overlay, and clear its own interval. Crucially this
    /// runs on the MAIN thread, which is never blocked (the cartridge runs in
    /// the worker), so it can always fire — that is what makes a hung cartridge
    /// terminable + the subdomain un-brickable.
    ///
    /// SAFETY: the watchdog must NOT drop its own `WorkerHandle` from inside the
    /// callback (that would drop the `Closure` currently executing). So on a
    /// hang it sets `terminated` (making the slot inert + `is_active()` false),
    /// terminates the worker, and clears its interval via the SHARED `interval_id`
    /// cell — but leaves the `WorkerHandle` in place. The next `spawn_cartridge`
    /// / `stop_worker` drops it safely (off the callback stack); `Drop` clears
    /// whatever id remains, and the `take()` here ensures it sees `None`.
    fn arm_watchdog(
        worker: Worker,
        ctx: CanvasRenderingContext2d,
        last_frame: Rc<Cell<f64>>,
        terminated: Rc<Cell<bool>>,
        interval_id: Rc<Cell<Option<i32>>>,
        run_gen: u32,
    ) -> Option<Closure<dyn FnMut()>> {
        let cb = {
            let interval_id = interval_id.clone();
            Closure::<dyn FnMut()>::new(move || {
                if terminated.get() {
                    return;
                }
                if js_sys::Date::now() - last_frame.get() > WATCHDOG_MS {
                    terminated.set(true);
                    worker.terminate();
                    // LH1001 = frame timeout (the hang the watchdog catches).
                    // Record it as the run outcome too, so a cartridge that
                    // never produced its FIRST frame reports the kill to the
                    // awaiting run_cartridge tool, not just the overlay.
                    record_outcome(
                        run_gen,
                        RunOutcome::Failed {
                            code: Some(crate::error_codes::FRAME_TIMEOUT),
                            detail: format!(
                                "no frame within {WATCHDOG_MS}ms — the watchdog \
                                 terminated the hung cartridge"
                            ),
                        },
                    );
                    paint_stopped_overlay_coded(&ctx, crate::error_codes::FRAME_TIMEOUT);
                    // Self-clear the interval (don't drop the handle here). The
                    // shared `take()` leaves `None` so `Drop` won't re-clear a
                    // recycled id.
                    if let Some(id) = interval_id.take() {
                        if let Ok(win) = dom::window() {
                            win.clear_interval_with_handle(id);
                        }
                    }
                }
            })
        };
        let id = dom::window().ok().and_then(|win| {
            win.set_interval_with_callback_and_timeout_and_arguments_0(
                cb.as_ref().unchecked_ref(),
                WATCHDOG_TICK_MS,
            )
            .ok()
        });
        interval_id.set(id);
        Some(cb)
    }

    /// Paint a monochrome "cartridge stopped" message into the framebuffer
    /// (pixels, via the shared 5x7 font — no DOM), so the canvas shows WHY it
    /// went dark instead of just freezing — now carrying the stable LH1xxx
    /// runtime code + its registry meaning. The rest of the app is reachable.
    /// The font has only uppercase/digits/limited punctuation, so we uppercase
    /// the meaning and keep the lines short.
    fn paint_stopped_overlay_coded(ctx: &CanvasRenderingContext2d, code: u16) {
        let mut buf = vec![0u8; (FB_W * FB_H * 4) as usize];
        for px in buf.chunks_exact_mut(4) {
            px[3] = 255; // opaque black
        }
        let vp = crate::raster::Viewport::full(FB_W as i32, FB_H as i32);
        // Line 1: "CARTRIDGE STOPPED LHxxxx"; line 2: the code's meaning.
        let label = crate::error_codes::fmt_label(code);
        let meaning = crate::error_codes::lookup(code)
            .map(|e| e.meaning.to_uppercase())
            .unwrap_or_else(|| "RELOAD TO RETRY".to_string());
        let header = format!("CARTRIDGE STOPPED {label}");
        let owned = [header, meaning];
        let lines: [&str; 2] = [owned[0].as_str(), owned[1].as_str()];
        let mut y = (FB_H as i32) / 2 - 8;
        for line in lines {
            let advance = 6; // 5px glyph + 1px gap at scale 1
            let width = line.len() as i32 * advance;
            let mut x = ((FB_W as i32) - width) / 2;
            for ch in line.chars() {
                crate::raster::blit_glyph(
                    &mut buf, FB_W as i32, &vp, x, y, ch as u32, (200, 200, 200), 1,
                );
                x += advance;
            }
            y += 12;
        }
        if let Ok(img) =
            ImageData::new_with_u8_clamped_array_and_sh(Clamped(&buf[..]), FB_W, FB_H)
        {
            let _ = ctx.put_image_data(&img, 0.0, 0.0);
        }
    }
}

// --- HTML → framebuffer rendering --------------------------------------
//
// A deliberately tiny renderer: enough to show what an `index.html`
// "says" on the screen, not a browser engine. We extract block-level text
// (headings/paragraphs/lists), drop `<head>`/`<script>`/`<style>`, decode
// the common entities, then word-wrap and blit with the bitmap font.

/// One laid-out block of text. `scale` drives glyph size (headings are
/// bigger); `bullet` prefixes a list dash.
struct HtmlBlock {
    text: String,
    scale: i32,
    bullet: bool,
}

/// Extract the lowercased tag name from the inside of a `<...>` (handles a
/// leading `/` for close tags and trailing attributes/`/`).
fn tag_name(inner: &str) -> String {
    let t = inner.trim().trim_start_matches('/').trim_start();
    let end = t
        .find(|ch: char| ch.is_whitespace() || ch == '/')
        .unwrap_or(t.len());
    t[..end].to_ascii_lowercase()
}

/// Decode the handful of HTML entities that show up in plain prose.
fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        // `&amp;` last so a literal "&amp;lt;" doesn't double-decode.
        .replace("&amp;", "&")
}

/// Collapse runs of whitespace to single spaces and trim — HTML source
/// whitespace is not significant for our layout.
fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim_end().to_string()
}

/// Push the accumulated text run as a block (decoded + collapsed), then
/// clear it. No-op for an empty run.
fn flush_block(blocks: &mut Vec<HtmlBlock>, cur: &mut String, scale: i32, bullet: bool) {
    let text = collapse_ws(&decode_entities(cur));
    cur.clear();
    if !text.is_empty() {
        blocks.push(HtmlBlock { text, scale, bullet });
    }
}

/// Parse a subset of HTML into renderable text blocks. Inline tags
/// (`a`, `span`, `b`, `code`, …) are ignored — their text just flows into
/// the current block. `head`/`script`/`style` content is skipped wholesale.
fn html_to_blocks(src: &str) -> Vec<HtmlBlock> {
    let chars: Vec<char> = src.chars().collect();
    let mut blocks: Vec<HtmlBlock> = Vec::new();
    let mut cur = String::new();
    let mut scale: i32 = 1;
    let mut bullet = false;
    let mut skip_tag: Option<String> = None;

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '<' {
            // Read up to the closing '>'.
            let mut j = i + 1;
            let mut inner = String::new();
            while j < chars.len() && chars[j] != '>' {
                inner.push(chars[j]);
                j += 1;
            }
            i = if j < chars.len() { j + 1 } else { j };

            let closing = inner.trim_start().starts_with('/');
            let name = tag_name(&inner);

            // Inside a skipped region, ignore everything but its close.
            if let Some(skip) = skip_tag.clone() {
                if closing && name == skip {
                    skip_tag = None;
                }
                continue;
            }

            match name.as_str() {
                "script" | "style" | "head" => {
                    if !closing {
                        skip_tag = Some(name);
                    }
                }
                "br" | "hr" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    bullet = false;
                }
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    bullet = false;
                    scale = if closing {
                        1
                    } else if name == "h1" {
                        3
                    } else {
                        2
                    };
                }
                "li" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    scale = 1;
                    bullet = !closing;
                }
                "p" | "div" | "ul" | "ol" | "section" | "article" | "header" | "footer"
                | "nav" | "main" | "blockquote" | "pre" | "table" | "tr" | "title" | "body"
                | "html" | "figure" | "figcaption" => {
                    flush_block(&mut blocks, &mut cur, scale, bullet);
                    bullet = false;
                    scale = 1;
                }
                _ => { /* inline tag — let its text flow into the block */ }
            }
            continue;
        }

        if skip_tag.is_some() {
            i += 1;
            continue;
        }
        cur.push(c);
        i += 1;
    }
    flush_block(&mut blocks, &mut cur, scale, bullet);
    blocks
}

/// Word-wrap `text` to at most `max_chars` per line, hard-breaking any
/// single word longer than the line.
fn wrap_text(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= max_chars {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
        }
        // Hard-break a word that overflows the line on its own.
        while line.chars().count() > max_chars {
            let head: String = line.chars().take(max_chars).collect();
            let tail: String = line.chars().skip(max_chars).collect();
            lines.push(head);
            line = tail;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Fill a fresh framebuffer with an opaque colour.
fn filled_framebuffer(color: (u8, u8, u8)) -> Vec<u8> {
    let (r, g, b) = color;
    let mut buf = vec![0u8; FB_BYTES];
    let mut i = 0;
    while i + 3 < buf.len() {
        buf[i] = r;
        buf[i + 1] = g;
        buf[i + 2] = b;
        buf[i + 3] = 255;
        i += 4;
    }
    buf
}

/// Lay out parsed blocks into the framebuffer. Monochrome: near-black
/// background, light text, white headings. Clips at the bottom edge (no
/// scrolling — this is a screenshot, not a scroll view).
fn paint_html_fb(blocks: &[HtmlBlock]) -> Vec<u8> {
    let mut buf = filled_framebuffer((13, 13, 13));
    let left = 6i32;
    let right = FB_W as i32 - 6;
    let mut y = 6i32;

    for block in blocks {
        let scale = block.scale.clamp(1, 3);
        let advance = 6 * scale; // 5px glyph + 1px gap
        let line_h = 8 * scale; // 7px glyph + 1px gap
        let max_chars = (((right - left) / advance).max(1)) as usize;
        let color = if scale > 1 { (245, 245, 245) } else { (205, 205, 205) };
        let text = if block.bullet {
            format!("- {}", block.text)
        } else {
            block.text.clone()
        };

        for line in wrap_text(&text, max_chars) {
            if y + line_h > FB_H as i32 {
                return buf; // out of vertical room
            }
            let mut x = left;
            let vp = crate::raster::Viewport::full(FB_W as i32, FB_H as i32);
            for ch in line.chars() {
                crate::raster::blit_glyph(&mut buf, FB_W as i32, &vp, x, y, ch as u32, color, scale);
                x += advance;
            }
            y += line_h;
        }
        y += 3; // gap between blocks
    }
    buf
}

// --- host_audio: Web Audio (AudioContext) cartridge sound ---------------
//
// The audio analog of host_display's framebuffer: integer-only host fns a
// rustlite cartridge calls, no DOM. One AudioContext per tab (browsers cap
// context count) lives in a thread_local, lazily created + resumed on the
// first call (an AudioContext is silent until a user gesture — and a
// cartridge only runs after the user opened it, so the first tone resumes
// it). Voices are osc/buffer -> per-voice gain -> shared master gain ->
// destination, and auto-free on `onended` so the handle table can't grow
// unbounded. Mirrors `mod net`'s poll/fire-and-forget style + handle table.
mod audio {
    use std::cell::RefCell;

    use js_sys::{Function, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{AudioContext, GainNode, OscillatorType};

    /// Cap concurrent voices so a runaway cartridge can't spawn thousands of
    /// nodes; the oldest live voice is stopped first (mirrors host_net's
    /// MAX_INBOX bound).
    const MAX_VOICES: usize = 64;

    thread_local! {
        /// One shared AudioContext + master gain per tab, created lazily on
        /// the first audio host call.
        static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    }

    struct Engine {
        ctx: AudioContext,
        master: GainNode,
        /// Live voices by handle index; a stopped voice becomes `None` so
        /// handles never alias (same scheme as host_net's socket table).
        voices: Vec<Option<Voice>>,
    }

    struct Voice {
        /// The scheduled source node (oscillator or buffer source) as a
        /// `JsValue`, so `stop` can call `.stop()` on it early regardless of
        /// the concrete type.
        node: JsValue,
        /// Keeps the `onended` closure alive for the voice's lifetime.
        _onended: Closure<dyn FnMut()>,
    }

    /// Get-or-create the shared engine, resuming the context (a no-op if
    /// already running). Returns `None` only if the browser has no
    /// AudioContext or node creation fails.
    fn with_engine<R>(f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
        ENGINE.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.is_none() {
                let ctx = AudioContext::new().ok()?;
                let master = ctx.create_gain().ok()?;
                master.gain().set_value(0.3);
                let _ = master.connect_with_audio_node(&ctx.destination());
                *slot = Some(Engine { ctx, master, voices: Vec::new() });
            }
            let eng = slot.as_mut()?;
            let _ = eng.ctx.resume();
            Some(f(eng))
        })
    }

    /// Insert a voice, capping the table at `MAX_VOICES`; returns its handle.
    /// The oldest live voice is stopped if we're at the cap.
    fn push_voice(eng: &mut Engine, voice: Voice) -> i32 {
        let live = eng.voices.iter().filter(|v| v.is_some()).count();
        if live >= MAX_VOICES {
            if let Some(slot) = eng.voices.iter_mut().find(|s| s.is_some()) {
                if let Some(old) = slot.take() {
                    stop_node(&old.node);
                }
            }
        }
        if let Some(i) = eng.voices.iter().position(|s| s.is_none()) {
            eng.voices[i] = Some(voice);
            i as i32
        } else {
            eng.voices.push(Some(voice));
            (eng.voices.len() - 1) as i32
        }
    }

    /// Call `.stop()` on an oscillator/buffer-source `JsValue`, ignoring
    /// errors (the node may already have ended).
    fn stop_node(node: &JsValue) {
        if let Ok(f) = Reflect::get(node, &JsValue::from_str("stop")) {
            if let Ok(f) = f.dyn_into::<Function>() {
                let _ = f.call0(node);
            }
        }
    }

    fn osc_type(wave: i32) -> OscillatorType {
        match wave {
            1 => OscillatorType::Square,
            2 => OscillatorType::Sawtooth,
            3 => OscillatorType::Triangle,
            _ => OscillatorType::Sine,
        }
    }

    /// Schedule a tone `delay_ms` in the future for `dur_ms`. Shared by
    /// `tone` (delay 0) and `tone_at`. Returns a voice handle or -1.
    /// `pub(super)` so the cartridge-worker bridge can play tones forwarded
    /// from the worker (an AudioContext can't run in a worker, so audio host
    /// calls round-trip to the main thread).
    pub(super) fn play_tone(freq: i32, dur_ms: i32, wave: i32, delay_ms: i32) -> i32 {
        with_engine(|eng| {
            let osc = match eng.ctx.create_oscillator() {
                Ok(o) => o,
                Err(_) => return -1,
            };
            let gain = match eng.ctx.create_gain() {
                Ok(g) => g,
                Err(_) => return -1,
            };
            osc.set_type(osc_type(wave));
            osc.frequency().set_value(freq.max(1) as f32);

            let t0 = eng.ctx.current_time() + (delay_ms.max(0) as f64) / 1000.0;
            let dur = (dur_ms.max(1) as f64) / 1000.0;
            // 4ms attack / release so notes don't click.
            let g = gain.gain();
            let _ = g.set_value_at_time(0.0, t0);
            let _ = g.linear_ramp_to_value_at_time(1.0, t0 + 0.004);
            let _ = g.set_value_at_time(1.0, (t0 + dur - 0.004).max(t0 + 0.004));
            let _ = g.linear_ramp_to_value_at_time(0.0, t0 + dur);

            let _ = osc.connect_with_audio_node(&gain);
            let _ = gain.connect_with_audio_node(&eng.master);
            let _ = osc.start_with_when(t0);
            let _ = osc.stop_with_when(t0 + dur);

            let node: JsValue = osc.clone().into();
            let onended = Closure::<dyn FnMut()>::new(move || {});
            osc.set_onended(Some(onended.as_ref().unchecked_ref()));
            push_voice(eng, Voice { node, _onended: onended })
        })
        .unwrap_or(-1)
    }

    /// White-noise burst for `dur_ms`. Extracted so the cartridge-worker bridge
    /// can play `host_audio::noise` forwarded from the worker. Returns a voice
    /// handle or -1.
    pub(super) fn play_noise(dur_ms: i32) -> i32 {
        with_engine(|eng| {
            let sr = eng.ctx.sample_rate();
            let frames = sr as u32; // 1s of noise (truncated by duration)
            let buf = match eng.ctx.create_buffer(1, frames, sr) {
                Ok(b) => b,
                Err(_) => return -1,
            };
            let mut data = vec![0f32; frames as usize];
            // Cheap LCG white noise (getrandom not needed for audio).
            let mut s: u32 = 0x2545_F491;
            for x in data.iter_mut() {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                *x = ((s >> 8) as f32 / 8_388_608.0) - 1.0;
            }
            if buf.copy_to_channel(&data, 0).is_err() {
                return -1;
            }
            let src = match eng.ctx.create_buffer_source() {
                Ok(s) => s,
                Err(_) => return -1,
            };
            src.set_buffer(Some(&buf));
            let gain = match eng.ctx.create_gain() {
                Ok(g) => g,
                Err(_) => return -1,
            };
            let t0 = eng.ctx.current_time();
            let dur = (dur_ms.max(1) as f64) / 1000.0;
            let g = gain.gain();
            let _ = g.set_value_at_time(0.8, t0);
            let _ = g.linear_ramp_to_value_at_time(0.0, t0 + dur);
            let _ = src.connect_with_audio_node(&gain);
            let _ = gain.connect_with_audio_node(&eng.master);
            let _ = src.start_with_when(t0);
            // stop_with_when/set_onended live on the AudioScheduledSourceNode
            // base class in current web-sys; the same-named methods directly on
            // AudioBufferSourceNode are deprecated duplicates.
            let scheduled: &web_sys::AudioScheduledSourceNode = src.as_ref();
            let _ = scheduled.stop_with_when(t0 + dur);
            let node: JsValue = src.clone().into();
            let onended = Closure::<dyn FnMut()>::new(move || {});
            scheduled.set_onended(Some(onended.as_ref().unchecked_ref()));
            push_voice(eng, Voice { node, _onended: onended })
        })
        .unwrap_or(-1)
    }

    /// Stop one voice by handle, or all voices when `handle < 0`. Extracted so
    /// the cartridge-worker bridge can forward `host_audio::stop`.
    pub(super) fn stop_handle(handle: i32) {
        ENGINE.with(|cell| {
            if let Some(eng) = cell.borrow_mut().as_mut() {
                if handle < 0 {
                    for slot in eng.voices.iter_mut() {
                        if let Some(v) = slot.take() {
                            stop_node(&v.node);
                        }
                    }
                } else if let Some(slot) = eng.voices.get_mut(handle as usize) {
                    if let Some(v) = slot.take() {
                        stop_node(&v.node);
                    }
                }
            }
        });
    }

    /// Set the master gain (`pct` 0..=100). Extracted so the cartridge-worker
    /// bridge can forward `host_audio::set_volume`.
    pub(super) fn set_master_volume(pct: i32) {
        with_engine(|eng| {
            eng.master.gain().set_value((pct.clamp(0, 100) as f32) / 100.0);
        });
    }

    // NOTE: the in-thread `build_host_audio` import builder was removed with the
    // in-thread cartridge runtime (issue #77). The cartridge runs in a Web Worker
    // now, which implements `host_audio` itself and FORWARDS each op to the main
    // thread (only the main thread has an AudioContext); the worker bridge calls
    // `play_tone`/`play_noise`/`stop_handle`/`set_master_volume`/`stop_all` here.

    /// Stop every scheduled voice + suspend the context (called on cartridge
    /// swap / `display::stop`) so a swap never leaves a drone playing.
    pub(super) fn stop_all() {
        ENGINE.with(|cell| {
            if let Some(eng) = cell.borrow_mut().as_mut() {
                for slot in eng.voices.iter_mut() {
                    if let Some(v) = slot.take() {
                        stop_node(&v.node);
                    }
                }
                let _ = eng.ctx.suspend();
            }
        });
    }
}
