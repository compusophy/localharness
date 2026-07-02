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
//! ## Cartridge ABI (`host_audio`) — Web Audio playback (see `bridge::audio`)
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
//!
//! ## Module map
//! - [`worker`] — worker spawn / watchdog / run-outcome lifecycle + the
//!   onmessage router (the off-main-thread containment, the brick fix).
//! - [`surface`] — canvas mount, overlay chrome, pointer/touch state,
//!   embed-card plumbing, and the broadcast-composer UI.
//! - [`bridge`] — one module per host capability the worker round-trips to
//!   the main thread (feed / compose / http / mp / chat / audio).
//! - The pure HTML→framebuffer rasterizer lives at [`crate::html_fb`]
//!   (native-tested); this module only blits its output.

use std::cell::{Cell, RefCell};

use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

mod bridge;
mod surface;
mod worker;

pub(crate) use bridge::feed::prime_feed_permission_on_gesture;
pub(crate) use surface::{
    broadcast_composer_open, broadcast_send, cartridge_canvas_present, close_broadcast_composer,
    is_cartridge_canvas_id, launch_pending_embed, next_embed_canvas_id,
    relaunch_last_in_fullscreen, set_pointer, set_pointer_down, snapshot_data_url,
    stash_pending_embed,
};

/// DEFAULT logical framebuffer resolution: 512×512 (square). A cartridge that
/// does NOT export `dims()` renders at this size (backward compatible). The canvas
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
const FB_W: u32 = 512;
const FB_H: u32 = 512;

thread_local! {
    /// Generation counter for cartridge launches. Each launch bumps it so a
    /// stale launch's deferred work self-cancels when the global generation
    /// moves past the one it started with.
    static FRAME_GEN: Cell<u32> = const { Cell::new(0) };
}

/// Instantiate `wasm_bytes` as a display cartridge in the display
/// overlay (swaps in the overlay + surface). Used by the `run_cartridge`
/// tool and opening a `.wasm`/`.rl` from the files modal.
pub(crate) async fn run_wasm(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    let ctx = surface::mount_canvas()?;
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
    surface::remember_last_cartridge(wasm_bytes);
    stash_pending_embed(wasm_bytes.to_vec());
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
    let ctx = surface::size_and_get_ctx()?;
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
/// The canvas BACKING STORE is sized to the DEFAULT [`FB_W`]×[`FB_H`] (512×512)
/// here as an initial size, but it is RESIZED to the cartridge's actual dims
/// the moment its first `frame` message arrives ([`worker::blit_frame`] reads
/// the `w`/`h` the worker stamped on each frame and resizes the canvas + builds
/// the `ImageData` at `w`×`h`). A cartridge declares its size by exporting
/// `dims() -> i32` (packed `(w<<16)|h`); with no such export it stays 512×512
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
    let ctx = surface::mount_canvas()?;
    let blocks = crate::html_fb::html_to_blocks(source);
    let buf = crate::html_fb::paint_html_fb(&blocks, FB_W as i32, FB_H as i32);
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
    let ctx = surface::size_and_get_ctx()?;
    let blocks = crate::html_fb::html_to_blocks(source);
    let buf = crate::html_fb::paint_html_fb(&blocks, FB_W as i32, FB_H as i32);
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
    surface::set_active_canvas(canvas_id);
    // Bump the generation first so any previous launch's deferred work stops,
    // and tear down the previous worker (terminate + drop its closures).
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    worker::stop_worker();
    // Silence the prior cartridge's scheduled voices so a long note can't
    // drone into the new one.
    bridge::audio::stop_all();

    // Fresh cartridge starts with cleared input (the worker owns the 64-slot
    // state register file and zeroes it on load).
    surface::reset_pointer_down();

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
    let ctx = surface::size_and_get_ctx()?;
    // Bump the generation so any previous launch's deferred work stops, and
    // record the overlay canvas so pointer events route to it. (The worker
    // teardown happens inside `spawn_composition` → `spawn_worker`.)
    surface::set_active_canvas("display-canvas");
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    // Silence any prior cartridge's scheduled voices on the shared engine.
    bridge::audio::stop_all();
    surface::reset_pointer_down();

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

thread_local! {
    /// A human-readable reference to the cartridge that's CURRENTLY running, so an
    /// auto-filed crash report (`worker::record_outcome`) can say WHAT crashed —
    /// the `run_cartridge` SOURCE or the `embed_app` NAME — making cartridge
    /// failures REPRODUCIBLE (the source/name lives in the tool ARGS, which the
    /// report's recent-conversation block doesn't carry). Set at each launch site.
    static CARTRIDGE_REF: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Remember what cartridge is about to run (for crash-report context). Call at a
/// launch site BEFORE the worker can fail — `run_cartridge` (the source),
/// `embed_app` (the name), and the history resume path. Capped so a huge source
/// can't crowd the rest of the report (the proxy clamps the body anyway).
pub(crate) fn set_cartridge_ref(reference: Option<String>) {
    let capped = reference.map(|r| {
        if r.chars().count() > 6000 {
            let mut s: String = r.chars().take(6000).collect();
            s.push_str("\n…(truncated)");
            s
        } else {
            r
        }
    });
    CARTRIDGE_REF.with(|c| *c.borrow_mut() = capped);
}

/// The current cartridge reference (for `worker::record_outcome`'s crash report).
pub(super) fn cartridge_ref() -> Option<String> {
    CARTRIDGE_REF.with(|c| c.borrow().clone())
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
    bridge::audio::stop_all();
}
