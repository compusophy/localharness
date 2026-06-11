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
use std::rc::Rc;

use js_sys::{Function, Object, Reflect, WebAssembly};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

use super::dom;
use super::templates;

/// Logical framebuffer resolution. 16:9. The canvas backing store is
/// this size; CSS scales it up with `image-rendering: pixelated` so
/// individual pixels stay crisp.
const FB_W: u32 = 256;
const FB_H: u32 = 144;
const FB_BYTES: usize = (FB_W * FB_H * 4) as usize;

/// Shared host-owned framebuffer: `FB_W * FB_H` RGBA8888 pixels.
type Framebuffer = Rc<RefCell<Vec<u8>>>;

/// Self-referential holder for the rAF tick closure: the closure needs a
/// handle to itself to reschedule, so it lives behind a shared `Option`
/// that it clears to stop the loop.
type FrameLoopHolder = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

thread_local! {
    /// Generation counter for the animation loop. Each `run_wasm` bumps
    /// it; an in-flight rAF loop self-cancels when the global generation
    /// moves past the one it started with. Cleaner than tracking handles.
    static FRAME_GEN: Cell<u32> = const { Cell::new(0) };
    /// Holds the current cartridge's host-import closures alive for as
    /// long as it runs. Replaced on the next load (dropping the prior
    /// set, whose loop is already cancelled).
    static RUNTIME: RefCell<Option<CartridgeRuntime>> = const { RefCell::new(None) };
    /// Latest cursor position in framebuffer coordinates. Updated by the
    /// delegated `mousemove` listener (see `events.rs`), read by the
    /// `pointer_x`/`pointer_y` host imports. Poll model — cartridges read
    /// it each frame rather than receiving events.
    static POINTER: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    /// 1 while the primary mouse button is down over the canvas. Updated
    /// by the delegated mousedown/mouseup listeners, read by `pointer_down`.
    static POINTER_DOWN: Cell<i32> = const { Cell::new(0) };
    /// A 64-slot integer register file the cartridge can read/write to
    /// keep state across frames (rustlite has no globals). Zeroed when a
    /// new cartridge loads.
    static STATE: RefCell<[i32; 64]> = const { RefCell::new([0; 64]) };
}

/// Keeps every host import closure alive. wasm holds JS function
/// references into these; they must outlive the instance. Covers both the
/// `host_display` draw API and the `host_net` WebSocket API.
#[allow(dead_code)]
struct CartridgeRuntime {
    clear: Closure<dyn FnMut(i32)>,
    set_pixel: Closure<dyn FnMut(i32, i32, i32)>,
    fill_rect: Closure<dyn FnMut(i32, i32, i32, i32, i32)>,
    draw_char: Closure<dyn FnMut(i32, i32, i32, i32, i32)>,
    draw_number: Closure<dyn FnMut(i32, i32, i32, i32, i32)>,
    draw_line: Closure<dyn FnMut(i32, i32, i32, i32, i32)>,
    // host_display::fill_triangle's ABI is genuinely 7 i32s (x0,y0,x1,y1,x2,y2,z).
    #[allow(clippy::type_complexity)]
    fill_triangle: Closure<dyn FnMut(i32, i32, i32, i32, i32, i32, i32)>,
    present: Closure<dyn FnMut()>,
    width: Closure<dyn FnMut() -> i32>,
    height: Closure<dyn FnMut() -> i32>,
    pointer_x: Closure<dyn FnMut() -> i32>,
    pointer_y: Closure<dyn FnMut() -> i32>,
    pointer_down: Closure<dyn FnMut() -> i32>,
    state_get: Closure<dyn FnMut(i32) -> i32>,
    state_set: Closure<dyn FnMut(i32, i32)>,
    net: net::NetRuntime,
    audio: audio::AudioRuntime,
}

/// Where a cartridge's `pointer_*` / `state_*` host imports read from. The
/// single-cartridge path uses [`InputSource::Global`] (the shared thread-locals,
/// driven by the delegated DOM listeners) — identical to before host::compose.
/// A composed child uses [`InputSource::Local`]: its own pointer + 64-slot
/// register file, which the compositor fills focus-gated each frame so siblings
/// stay isolated (roadmap Track A / Phase 1a + 1c).
enum InputSource {
    // Retained as the in-thread single-cartridge fallback (reads the shared
    // POINTER/POINTER_DOWN/STATE thread-locals). The default single-cartridge
    // path now runs in a Web Worker (`mod worker`) which owns its own input +
    // state cells, so `Global` is currently only the documented fallback and
    // not constructed — composition uses `Local`. Keep it: it's the contract
    // for re-enabling an in-thread run, and the thread-locals it reads are
    // still live (the worker-input forwarder reads POINTER/POINTER_DOWN).
    #[allow(dead_code)]
    Global,
    Local {
        pointer: Rc<Cell<(i32, i32)>>,
        down: Rc<Cell<i32>>,
        state: Rc<RefCell<[i32; 64]>>,
    },
}

/// One composited child held in the [`crate::compose::ModuleTable`]: its
/// `frame`/`render` entry point, its focus-gated input cells (written by the
/// compositor each tick), and the runtime/memory kept alive for its lifetime.
struct ChildHandle {
    frame: Function,
    pointer: Rc<Cell<(i32, i32)>>,
    down: Rc<Cell<i32>>,
    _runtime: CartridgeRuntime,
    _mem: SharedMemory,
}

/// Shared handle to the cartridge's linear memory. The `host_net`
/// closures read/write length-prefixed strings through it, but memory
/// only exists after instantiation — so the closures hold this cell and
/// `run_with_ctx` fills it in once the instance is live.
type SharedMemory = Rc<RefCell<JsValue>>;

/// Instantiate `wasm_bytes` as a display cartridge in the display
/// overlay (swaps in the overlay + surface). Used by the `run_cartridge`
/// tool and opening a `.wasm`/`.rl` from the files modal.
pub(crate) async fn run_wasm(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    let ctx = mount_canvas()?;
    run_with_ctx(wasm_bytes, ctx).await
}

/// Instantiate `wasm_bytes` against an existing `#display-canvas`
/// already in the DOM (app mode — the subdomain booted straight into a
/// fullscreen cartridge, no overlay swap).
pub(crate) async fn run_in_root_canvas(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    let ctx = size_and_get_ctx()?;
    run_with_ctx(wasm_bytes, ctx).await
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
/// able to kill it", which the worker + watchdog provide. The old in-thread
/// `start_frame_loop` + `build_host_display` closures remain for the
/// composition path (`mount_composition`), which is owner-driven and not the
/// brick vector — see the note there.
async fn run_with_ctx(
    wasm_bytes: &[u8],
    ctx: CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    // Bump the generation first so any previous cartridge's loop stops, and
    // tear down the previous worker (terminate + drop its closures).
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    worker::stop_worker();
    // Drop any in-thread runtime (composition path) so its closures release.
    RUNTIME.with(|cell| *cell.borrow_mut() = None);
    // Silence the prior cartridge's scheduled voices so a long note can't
    // drone into the new one.
    audio::stop_all();

    // Fresh cartridge starts with cleared input + state.
    POINTER_DOWN.with(|d| d.set(0));
    STATE.with(|s| *s.borrow_mut() = [0; 64]);

    worker::spawn_cartridge(wasm_bytes, ctx)
}

/// Composite several cartridges into ONE framebuffer, iframe-free — the live
/// host::compose path (roadmap Track A), proven first in `scripts/render-
/// compose.js`. Each module gets its own wasm `Instance` + `Memory`, its own
/// 64-slot state, and a grid-cell [`crate::raster::Viewport`] it draws into via
/// the SAME `build_host_display` closures the single-cartridge path uses (a
/// child can't reach outside its rect — clipping is structural). Children are
/// held in the native-tested [`crate::compose::ModuleTable`]; the compositor
/// ticks each into the shared framebuffer and presents ONCE per frame (the
/// present-ownership inversion of Phase 0a is what makes that single present
/// possible). Admission is capped by [`crate::compose::ComposeBudget`] so an
/// attacker-chosen `?compose=` graph can't exhaust host memory.
///
/// `host_net` is wired per-child as today; a per-child URL allowlist (so a
/// composed module can't beacon under the compositor's origin) is the documented
/// follow-up gate (roadmap A4 / cross-cutting risk #1), tracked before any
/// agent-driven `host_compose` spawn ABI lands.
pub(crate) async fn mount_composition(modules: Vec<Option<Vec<u8>>>) -> Result<(), JsValue> {
    let ctx = size_and_get_ctx()?;
    // Bump the generation so any previous cartridge/compositor loop stops.
    let generation = FRAME_GEN.with(|g| {
        let n = g.get().wrapping_add(1);
        g.set(n);
        n
    });
    RUNTIME.with(|cell| *cell.borrow_mut() = None);
    // Silence any prior cartridge's scheduled voices on the shared engine.
    audio::stop_all();

    let fb: Framebuffer = Rc::new(RefCell::new(black_framebuffer()));
    POINTER_DOWN.with(|d| d.set(0));

    // Lay out a grid slot for EVERY requested module — including ones that failed
    // to fetch (passed as `None`) — so an unavailable module leaves its cell black
    // instead of shifting its siblings out of position. Layout is native-tested in
    // `crate::compose`.
    let viewports = crate::compose::grid_viewports(modules.len(), FB_W as i32, FB_H as i32);
    let budget = crate::compose::ComposeBudget::v1();
    let mut table: crate::compose::ModuleTable<ChildHandle> = crate::compose::ModuleTable::new();
    let mut total_bytes = 0usize;

    for (slot, vp) in modules.into_iter().zip(viewports) {
        let Some(bytes) = slot else { continue }; // unavailable module -> black cell
        if let Err(reason) = budget.admit(table.len(), total_bytes, bytes.len()) {
            web_sys::console::warn_1(&JsValue::from_str(&reason));
            continue;
        }
        let pointer = Rc::new(Cell::new((-1, -1)));
        let down = Rc::new(Cell::new(0));
        let state = Rc::new(RefCell::new([0i32; 64]));
        let mem: SharedMemory = Rc::new(RefCell::new(JsValue::NULL));
        let input = InputSource::Local { pointer: pointer.clone(), down: down.clone(), state };
        let (imports, runtime) = build_host_display(&fb, &mem, vp, input)?;

        let result = JsFuture::from(WebAssembly::instantiate_buffer(&bytes, &imports)).await?;
        let instance = Reflect::get(&result, &JsValue::from_str("instance"))?;
        let exports = Reflect::get(&instance, &JsValue::from_str("exports"))?;
        *mem.borrow_mut() = Reflect::get(&exports, &JsValue::from_str("memory"))?;

        let Some(frame) = export_fn(&exports, "frame").or_else(|| export_fn(&exports, "render"))
        else {
            web_sys::console::warn_1(&JsValue::from_str("compose: a module exports neither frame nor render — skipped"));
            continue;
        };
        total_bytes += bytes.len();
        table.push(ChildHandle { frame, pointer, down, _runtime: runtime, _mem: mem }, vp);
    }

    if table.is_empty() {
        return Err(JsValue::from_str("compose: no module could be mounted"));
    }
    start_compose_loop(table, generation, fb, ctx);
    Ok(())
}

/// Set every framebuffer pixel to opaque black (the compositor clears the root
/// once per frame before ticking children, so inter-cell gaps stay black).
fn clear_black(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        px[0] = 0;
        px[1] = 0;
        px[2] = 0;
        px[3] = 255;
    }
}

/// The compositor rAF loop: read the global pointer, hit-test it to the topmost
/// child via [`crate::compose::ModuleTable::focus_at`], feed THAT child local
/// pointer coords (siblings see `(-1,-1)`/up), clear the root, tick every child
/// into the shared framebuffer, present once. Self-cancels when the generation
/// moves past `generation` (a new load / `stop`), mirroring [`start_frame_loop`].
fn start_compose_loop(
    table: crate::compose::ModuleTable<ChildHandle>,
    generation: u32,
    fb: Framebuffer,
    ctx: CanvasRenderingContext2d,
) {
    let start = js_sys::Date::now();
    let table = Rc::new(RefCell::new(table));
    let holder: FrameLoopHolder = Rc::new(RefCell::new(None));
    let holder2 = holder.clone();

    *holder.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        if FRAME_GEN.with(|g| g.get()) != generation {
            let _ = holder2.borrow_mut().take();
            return;
        }
        let t = (js_sys::Date::now() - start) as i32;
        let (gx, gy) = POINTER.with(|p| p.get());
        let gdown = POINTER_DOWN.with(|d| d.get());

        let mut tb = table.borrow_mut();
        let focus = tb.focus_at(gx, gy);
        {
            let mut buf = fb.borrow_mut();
            clear_black(&mut buf);
        }
        tb.tick(|i, child, _vp, _pending| {
            match focus {
                Some((fi, lx, ly)) if fi == i => {
                    child.pointer.set((lx, ly));
                    child.down.set(gdown);
                }
                _ => {
                    child.pointer.set((-1, -1));
                    child.down.set(0);
                }
            }
            // The child's host_display closures draw into `fb` through its own
            // viewport; the compositor holds no `fb` borrow here.
            let _ = child.frame.call1(&JsValue::NULL, &JsValue::from(t));
        });
        drop(tb);
        present_framebuffer(&fb, &ctx);

        if let Some(cb) = holder2.borrow().as_ref() {
            let _ = request_af(cb);
        }
    }) as Box<dyn FnMut()>));

    if let Some(cb) = holder.borrow().as_ref() {
        let _ = request_af(cb);
    }
}

/// Blit the host-owned framebuffer to the canvas. The host owns presenting now
/// (the cartridge `present` import is a no-op) — called once after each
/// `frame()`/`render()`, and once per compositor frame after every child has
/// drawn. See `design/host-compose.md` (roadmap 0a).
fn present_framebuffer(fb: &Framebuffer, ctx: &CanvasRenderingContext2d) {
    let buf = fb.borrow();
    if let Ok(img) = ImageData::new_with_u8_clamped_array_and_sh(Clamped(&buf[..]), FB_W, FB_H) {
        let _ = ctx.put_image_data(&img, 0.0, 0.0);
    }
}

/// Build the `host_display` import object (the Orbclient-style draw API)
/// over a shared host-owned framebuffer, plus the runtime that keeps the
/// closures alive.
fn build_host_display(
    fb: &Framebuffer,
    mem: &SharedMemory,
    vp: crate::raster::Viewport,
    input: InputSource,
) -> Result<(Object, CartridgeRuntime), JsValue> {
    // The viewport translates+clips this cartridge's draws into its sub-rect of
    // the shared framebuffer: a full-screen identity transform for a single
    // cartridge, a grid cell for a composed child (host::compose / Track A). The
    // pixel math lives in `crate::raster` (pure, native-tested) so the child path
    // is the same closures over a different rect. See `src/raster.rs`.

    let clear = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32)>::new(move |rgb: i32| {
            let mut buf = fb.borrow_mut();
            crate::raster::clear(&mut buf, FB_W as i32, &vp, rgb_components(rgb));
        })
    };

    let set_pixel = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32)>::new(move |x: i32, y: i32, rgb: i32| {
            let mut buf = fb.borrow_mut();
            crate::raster::set_pixel(&mut buf, FB_W as i32, &vp, x, y, rgb_components(rgb));
        })
    };

    let fill_rect = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32)>::new(
            move |x: i32, y: i32, w: i32, h: i32, rgb: i32| {
                let mut buf = fb.borrow_mut();
                crate::raster::fill_rect(&mut buf, FB_W as i32, &vp, x, y, w, h, rgb_components(rgb));
            },
        )
    };

    // present-ownership inversion (roadmap Phase 0a): the cartridge's `present`
    // import is now a NO-OP — the HOST presents once after each `frame()` (see
    // `present_framebuffer` + `start_frame_loop`). A cartridge calling present()
    // mid-frame no longer blits the whole canvas, which is what lets a future
    // compositor draw several module framebuffers before a single present.
    // Validated by scripts/render-cartridge.js (the present-after-frame model
    // renders the real bitmask cartridge correctly).
    let present = Closure::<dyn FnMut()>::new(move || {});

    let draw_char = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32)>::new(
            move |x: i32, y: i32, code: i32, rgb: i32, scale: i32| {
                let mut buf = fb.borrow_mut();
                crate::raster::blit_glyph(
                    &mut buf, FB_W as i32, &vp, x, y, code as u32, rgb_components(rgb), scale,
                );
            },
        )
    };

    let draw_number = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32)>::new(
            move |x: i32, y: i32, value: i32, rgb: i32, scale: i32| {
                let mut buf = fb.borrow_mut();
                crate::raster::draw_number(
                    &mut buf, FB_W as i32, &vp, x, y, value, rgb_components(rgb), scale,
                );
            },
        )
    };

    // --- software-3D primitives (FB#12b): line + filled triangle + z-tested
    // triangle over the SAME pixel/viewport model as every other primitive
    // (no WebGL, no iframe — pure writes into the shared framebuffer). All
    // i32 ABI; the pixel math is in `crate::raster` (pure, native-tested).
    let draw_line = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32)>::new(
            move |x0: i32, y0: i32, x1: i32, y1: i32, rgb: i32| {
                let mut buf = fb.borrow_mut();
                crate::raster::draw_line(
                    &mut buf, FB_W as i32, &vp, x0, y0, x1, y1, rgb_components(rgb),
                );
            },
        )
    };

    let fill_triangle = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32, i32, i32)>::new(
            move |x0: i32, y0: i32, x1: i32, y1: i32, x2: i32, y2: i32, rgb: i32| {
                let mut buf = fb.borrow_mut();
                crate::raster::fill_triangle(
                    &mut buf, FB_W as i32, &vp, x0, y0, x1, y1, x2, y2, rgb_components(rgb),
                );
            },
        )
    };

    // A child sees a display the size of its viewport, like Orbclient.
    let width = Closure::<dyn FnMut() -> i32>::new(move || vp.w);
    let height = Closure::<dyn FnMut() -> i32>::new(move || vp.h);

    // Input + per-cartridge state: the single cartridge reads the shared
    // thread-local pointer/state (`Global` — byte-identical to before the
    // refactor); a composed child reads ITS OWN cells (`Local`), which the
    // compositor populates per frame, focus-gated, so a click in one panel
    // can't drive a sibling and each child keeps its own 64-slot register file.
    #[allow(clippy::type_complexity)] // 5 distinct host-input closures bound at once
    let (pointer_x, pointer_y, pointer_down, state_get, state_set): (
        Closure<dyn FnMut() -> i32>,
        Closure<dyn FnMut() -> i32>,
        Closure<dyn FnMut() -> i32>,
        Closure<dyn FnMut(i32) -> i32>,
        Closure<dyn FnMut(i32, i32)>,
    ) = match input {
        InputSource::Global => (
            Closure::<dyn FnMut() -> i32>::new(move || POINTER.with(|p| p.get().0)),
            Closure::<dyn FnMut() -> i32>::new(move || POINTER.with(|p| p.get().1)),
            Closure::<dyn FnMut() -> i32>::new(move || POINTER_DOWN.with(|d| d.get())),
            Closure::<dyn FnMut(i32) -> i32>::new(move |slot: i32| {
                if !(0..64).contains(&slot) {
                    return 0;
                }
                STATE.with(|s| s.borrow()[slot as usize])
            }),
            Closure::<dyn FnMut(i32, i32)>::new(move |slot: i32, value: i32| {
                if !(0..64).contains(&slot) {
                    return;
                }
                STATE.with(|s| s.borrow_mut()[slot as usize] = value);
            }),
        ),
        InputSource::Local { pointer, down, state } => {
            let (px, py, pd) = (pointer.clone(), pointer.clone(), down);
            let (sg, ss) = (state.clone(), state);
            (
                Closure::<dyn FnMut() -> i32>::new(move || px.get().0),
                Closure::<dyn FnMut() -> i32>::new(move || py.get().1),
                Closure::<dyn FnMut() -> i32>::new(move || pd.get()),
                Closure::<dyn FnMut(i32) -> i32>::new(move |slot: i32| {
                    if !(0..64).contains(&slot) {
                        return 0;
                    }
                    sg.borrow()[slot as usize]
                }),
                Closure::<dyn FnMut(i32, i32)>::new(move |slot: i32, value: i32| {
                    if !(0..64).contains(&slot) {
                        return;
                    }
                    ss.borrow_mut()[slot as usize] = value;
                }),
            )
        }
    };

    let host_display = Object::new();
    set_fn(&host_display, "clear", &clear)?;
    set_fn(&host_display, "set_pixel", &set_pixel)?;
    set_fn(&host_display, "fill_rect", &fill_rect)?;
    set_fn(&host_display, "draw_char", &draw_char)?;
    set_fn(&host_display, "draw_number", &draw_number)?;
    set_fn(&host_display, "draw_line", &draw_line)?;
    set_fn(&host_display, "fill_triangle", &fill_triangle)?;
    set_fn(&host_display, "present", &present)?;
    set_fn(&host_display, "width", &width)?;
    set_fn(&host_display, "height", &height)?;
    set_fn(&host_display, "pointer_x", &pointer_x)?;
    set_fn(&host_display, "pointer_y", &pointer_y)?;
    set_fn(&host_display, "pointer_down", &pointer_down)?;
    set_fn(&host_display, "state_get", &state_get)?;
    set_fn(&host_display, "state_set", &state_set)?;

    let imports = Object::new();
    Reflect::set(&imports, &JsValue::from_str("host_display"), &host_display)?;

    // host_net — WebSocket-backed multiplayer / sync I/O (poll model).
    let net = net::build_host_net(&imports, mem)?;

    // host_audio — Web Audio (AudioContext) playback (fire-and-forget).
    let audio = audio::build_host_audio(&imports)?;

    Ok((
        imports,
        CartridgeRuntime {
            clear, set_pixel, fill_rect, draw_char, draw_number, draw_line, fill_triangle,
            present, width, height,
            pointer_x, pointer_y, pointer_down, state_get, state_set, net, audio,
        },
    ))
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
/// worker if one is live. The single-cartridge path runs off-thread, so its
/// `pointer_*` host imports read cells INSIDE the worker — we keep them fresh
/// by posting on every pointer event. (The thread-local cells are still
/// updated for the in-thread composition path.) No-op when no worker is active.
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
    let Some(el) = dom::by_id("display-canvas") else { return };
    let Ok(canvas) = el.dyn_into::<HtmlCanvasElement>() else { return };
    let rect = canvas.get_bounding_client_rect();
    let (w, h) = (rect.width(), rect.height());
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let fx = (((client_x - rect.left()) / w) * FB_W as f64).clamp(0.0, (FB_W - 1) as f64) as i32;
    let fy = (((client_y - rect.top()) / h) * FB_H as f64).clamp(0.0, (FB_H - 1) as f64) as i32;
    POINTER.with(|p| p.set((fx, fy)));
    forward_pointer_to_worker();
}

fn set_fn<T: ?Sized>(obj: &Object, name: &str, closure: &Closure<T>) -> Result<(), JsValue> {
    Reflect::set(obj, &JsValue::from_str(name), closure.as_ref().unchecked_ref())?;
    Ok(())
}

/// Decode a `0xRRGGBB` colour into `(r, g, b)` bytes.
fn rgb_components(c: i32) -> (u8, u8, u8) {
    let u = c as u32;
    (((u >> 16) & 0xff) as u8, ((u >> 8) & 0xff) as u8, (u & 0xff) as u8)
}

fn black_framebuffer() -> Vec<u8> {
    let mut buf = vec![0u8; FB_BYTES];
    // opaque black: alpha 255
    let mut i = 3;
    while i < buf.len() {
        buf[i] = 255;
        i += 4;
    }
    buf
}

/// Look up an exported function by name, `None` if missing/not callable.
fn export_fn(exports: &JsValue, name: &str) -> Option<Function> {
    Reflect::get(exports, &JsValue::from_str(name))
        .ok()
        .and_then(|v| v.dyn_into::<Function>().ok())
}

// NOTE: the old single-cartridge in-thread `start_frame_loop` was removed — the
// single-cartridge path now runs in a Web Worker (see `mod worker`) so a hung
// `frame()` can't freeze the main thread. The composition path keeps its own
// in-thread loop (`start_compose_loop`) because it draws several owner-chosen
// modules into one framebuffer via offset viewports, which the worker (identity
// viewport, one cartridge) doesn't model yet. Composition is owner-driven, not
// the brick vector; moving it off-thread is tracked as follow-up.

fn request_af(cb: &Closure<dyn FnMut()>) -> Result<i32, JsValue> {
    dom::window()?.request_animation_frame(cb.as_ref().unchecked_ref())
}

/// Stop any running cartridge loop (e.g. when the surface is closed).
pub(crate) fn stop() {
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    RUNTIME.with(|cell| *cell.borrow_mut() = None);
    // Terminate + drop the cartridge worker (the single-cartridge path runs
    // off-thread now). Idempotent — a no-op when no worker is live.
    worker::stop_worker();
    // Dropping RUNTIME drops the per-cartridge GainNodes; stop_all also halts
    // any voices already scheduled on the shared thread_local engine so a swap
    // never leaves a drone playing.
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
/// or the encode fails. Cheap: the backing store is the 256x144 logical
/// framebuffer, so the PNG is a few KB.
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

    /// Spawn a worker for `wasm_bytes`, wire its message handler to the canvas
    /// `ctx`, post the cartridge, and arm the watchdog. Replaces any previous
    /// worker (its `Drop` terminates it + clears its interval).
    pub(super) fn spawn_cartridge(
        wasm_bytes: &[u8],
        ctx: CanvasRenderingContext2d,
    ) -> Result<(), JsValue> {
        // Tear down the previous worker first (idempotent).
        stop_worker();

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
            Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                let data = e.data();
                let ty = Reflect::get(&data, &JsValue::from_str("type"))
                    .ok()
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                match ty.as_str() {
                    "frame" => {
                        last_frame.set(js_sys::Date::now());
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
                    "done" => {
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

        // Post the wasm. `instantiate` copies the bytes, so we don't need to
        // transfer ownership of this buffer.
        let bytes = Uint8Array::from(wasm_bytes);
        let msg = Object::new();
        Reflect::set(&msg, &JsValue::from_str("type"), &JsValue::from_str("load"))?;
        Reflect::set(&msg, &JsValue::from_str("wasm"), &bytes.buffer())?;
        worker
            .post_message(&msg)
            .map_err(|e| JsValue::from_str(&format!("worker post failed: {e:?}")))?;

        // Arm the watchdog: terminate the worker if no frame lands in time.
        let watchdog_cb = arm_watchdog(
            worker.clone(),
            ctx,
            last_frame.clone(),
            terminated.clone(),
            watchdog_id.clone(),
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
    fn blit_frame(data: &JsValue, ctx: &CanvasRenderingContext2d) {
        let Ok(fb) = Reflect::get(data, &JsValue::from_str("fb")) else { return };
        let Ok(buffer) = fb.dyn_into::<ArrayBuffer>() else { return };
        let clamped = Uint8ClampedArray::new(&buffer);
        // ImageData wants a &Clamped<&[u8]>; copy the transferred buffer out.
        let mut bytes = vec![0u8; clamped.length() as usize];
        clamped.copy_to(&mut bytes[..]);
        if let Ok(img) =
            ImageData::new_with_u8_clamped_array_and_sh(Clamped(&bytes[..]), FB_W, FB_H)
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

// --- host_net: WebSocket-backed cartridge networking --------------------
//
// A cartridge is a sandbox — linear memory plus the imports we grant it,
// no DOM. `host_net` grants it a **poll-model WebSocket**, the network
// analog of the `host_display` framebuffer: integer-only host functions,
// with strings (the URL and message bodies) passed as length-prefixed
// pointers into cartridge memory. The cartridge opens a socket, sends
// strings, and drains its inbox each `frame`. That's enough to build
// multi-device sync and multiplayer apps without any DOM access.
//
// Cartridge ABI (`host_net`, all under `host::net::`):
//   open(url_ptr) -> handle        connect; handle >= 0, or -1 on error
//   send(handle, ptr) -> ok        send the string at `ptr`; 1 ok / 0 not
//   poll(handle, out_ptr, max)     next inbound message into `out_ptr`
//        -> len                    (length-prefixed, <= `max` payload bytes);
//                                  returns payload len, 0 if empty, -1 bad handle
//   status(handle) -> i32          0 connecting / 1 open / 2 closing /
//                                  3 closed / -1 bad handle
//   close(handle)                  close + drop the socket's inbox
mod net {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use js_sys::{Object, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;
    use web_sys::{MessageEvent, WebSocket};

    use super::SharedMemory;

    /// Cap the inbox so a chatty peer can't grow memory unbounded; oldest
    /// messages drop first.
    const MAX_INBOX: usize = 256;

    /// Cap live sockets per cartridge. A `frame` loop calling `open` every
    /// tick would otherwise flood connections (fd exhaustion / turn the
    /// victim's tab into a connection-flood amplifier against a target). Once
    /// at the cap, `open` refuses until the cartridge `close`s one.
    const MAX_SOCKETS: usize = 8;

    /// Reject any WebSocket URL a cartridge must NOT be able to open. A
    /// cartridge is UNTRUSTED wasm published by any agent / fetched on-chain
    /// and run in the visitor's tab, so `open(url)` is an SSRF surface:
    /// without this gate it could connect to loopback / LAN / internal hosts
    /// (router admin, dev servers, metadata endpoints) from inside the
    /// victim's browser + network, or beacon to an arbitrary external host
    /// under the victim's origin. Policy:
    ///   * scheme MUST be `wss://` (encrypted). Plain `ws://` is refused —
    ///     it's the loopback-SSRF vector browsers DON'T mixed-content-block,
    ///     and it's cleartext exfil otherwise.
    ///   * host must not be empty, an IP literal, `localhost`, `*.localhost`,
    ///     or a `.local` mDNS name (no LAN / loopback reach).
    ///
    /// This is a deliberately conservative allowlist-by-shape — public TLS
    /// endpoints only — matching the multiplayer/sync use case. (`design/
    /// host-compose.md` A4 tracked a per-child variant; this lands the base
    /// gate for both the single-cartridge and composed paths.)
    fn url_is_allowed(url: &str) -> bool {
        // Scheme: wss:// only (case-insensitive), and require the `//`.
        let rest = match url
            .split_once("://")
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("wss"))
        {
            Some((_, rest)) => rest,
            None => return false,
        };
        // Authority is everything up to the first '/', '?' or '#'.
        let authority = rest
            .split(['/', '?', '#'])
            .next()
            .unwrap_or("");
        // Strip userinfo (`user:pass@host`) — judge the real host only, and
        // drop credentials-in-URL while we're here.
        let hostport = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);
        // Host is hostport minus an optional `:port`. Bracketed IPv6 (`[::1]`)
        // is rejected outright (it's an IP literal anyway).
        if hostport.starts_with('[') {
            return false; // IPv6 literal — never a public hostname
        }
        let host = hostport.split(':').next().unwrap_or("");
        if host.is_empty() {
            return false;
        }
        let lower = host.to_ascii_lowercase();
        // Block loopback / LAN names.
        if lower == "localhost"
            || lower.ends_with(".localhost")
            || lower.ends_with(".local")
        {
            return false;
        }
        // Block bare IPv4 literals (loopback 127/8, RFC-1918, link-local,
        // metadata 169.254.169.254, etc. — a published cartridge has no
        // legitimate reason to dial a raw IP, so refuse them all).
        if lower.split('.').count() == 4
            && lower.split('.').all(|o| !o.is_empty() && o.bytes().all(|b| b.is_ascii_digit()))
        {
            return false;
        }
        // Require at least one dot (a real DNS name like `host.example.com`);
        // a bare single label can resolve to an intranet host.
        lower.contains('.')
    }

    /// One open socket: the live `WebSocket` plus its not-yet-polled inbox
    /// of received text messages.
    struct Socket {
        ws: WebSocket,
        inbox: Rc<RefCell<VecDeque<String>>>,
        _on_message: Closure<dyn FnMut(MessageEvent)>,
    }

    /// Handle-indexed socket table; closed sockets become `None` so handles
    /// never alias.
    type SocketTable = Rc<RefCell<Vec<Option<Socket>>>>;

    /// Keeps the `host_net` import closures + socket table alive for the
    /// cartridge's lifetime. wasm holds JS references into the closures.
    #[allow(dead_code)]
    pub(super) struct NetRuntime {
        sockets: SocketTable,
        open: Closure<dyn FnMut(i32) -> i32>,
        send: Closure<dyn FnMut(i32, i32) -> i32>,
        poll: Closure<dyn FnMut(i32, i32, i32) -> i32>,
        status: Closure<dyn FnMut(i32) -> i32>,
        close: Closure<dyn FnMut(i32)>,
    }

    /// Build the `host_net` import object on `imports` and return the
    /// runtime that owns its closures (must outlive the wasm instance).
    pub(super) fn build_host_net(
        imports: &Object,
        mem: &SharedMemory,
    ) -> Result<NetRuntime, JsValue> {
        let sockets: SocketTable = Rc::new(RefCell::new(Vec::new()));

        let open = {
            let sockets = sockets.clone();
            let mem = mem.clone();
            Closure::<dyn FnMut(i32) -> i32>::new(move |url_ptr: i32| {
                let url = match read_string(&mem.borrow(), url_ptr) {
                    Some(u) => u,
                    None => return -1,
                };
                // SSRF/abuse gate: only public `wss://` hosts (no loopback /
                // LAN / IP literals). UNTRUSTED cartridge -> refuse anything
                // it shouldn't be able to dial before touching the network.
                if !url_is_allowed(&url) {
                    return -1;
                }
                // Connection cap: refuse once at MAX_SOCKETS live so a frame
                // loop can't flood connections. Reuse a freed (`None`) slot if
                // one exists so handles stay bounded.
                let free_slot = {
                    let table = sockets.borrow();
                    let live = table.iter().filter(|s| s.is_some()).count();
                    if live >= MAX_SOCKETS {
                        return -1;
                    }
                    table.iter().position(|s| s.is_none())
                };
                let ws = match WebSocket::new(&url) {
                    Ok(ws) => ws,
                    Err(_) => return -1,
                };
                ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

                let inbox: Rc<RefCell<VecDeque<String>>> =
                    Rc::new(RefCell::new(VecDeque::new()));
                let on_message = {
                    let inbox = inbox.clone();
                    Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
                        if let Some(text) = e.data().as_string() {
                            let mut q = inbox.borrow_mut();
                            if q.len() >= MAX_INBOX {
                                q.pop_front();
                            }
                            q.push_back(text);
                        }
                    })
                };
                ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

                let socket = Socket { ws, inbox, _on_message: on_message };
                let mut table = sockets.borrow_mut();
                match free_slot {
                    Some(i) => {
                        table[i] = Some(socket);
                        i as i32
                    }
                    None => {
                        let handle = table.len() as i32;
                        table.push(Some(socket));
                        handle
                    }
                }
            })
        };

        let send = {
            let sockets = sockets.clone();
            let mem = mem.clone();
            Closure::<dyn FnMut(i32, i32) -> i32>::new(move |handle: i32, ptr: i32| {
                let msg = match read_string(&mem.borrow(), ptr) {
                    Some(m) => m,
                    None => return 0,
                };
                let table = sockets.borrow();
                match table.get(handle as usize).and_then(|s| s.as_ref()) {
                    Some(sock) => match sock.ws.send_with_str(&msg) {
                        Ok(()) => 1,
                        Err(_) => 0,
                    },
                    None => 0,
                }
            })
        };

        let poll = {
            let sockets = sockets.clone();
            let mem = mem.clone();
            Closure::<dyn FnMut(i32, i32, i32) -> i32>::new(
                move |handle: i32, out_ptr: i32, max: i32| {
                    let table = sockets.borrow();
                    let sock = match table.get(handle as usize).and_then(|s| s.as_ref()) {
                        Some(s) => s,
                        None => return -1,
                    };
                    let msg = match sock.inbox.borrow_mut().pop_front() {
                        Some(m) => m,
                        None => return 0,
                    };
                    write_string(&mem.borrow(), out_ptr, &msg, max.max(0) as usize)
                },
            )
        };

        let status = {
            let sockets = sockets.clone();
            Closure::<dyn FnMut(i32) -> i32>::new(move |handle: i32| {
                let table = sockets.borrow();
                match table.get(handle as usize).and_then(|s| s.as_ref()) {
                    Some(sock) => sock.ws.ready_state() as i32,
                    None => -1,
                }
            })
        };

        let close = {
            let sockets = sockets.clone();
            Closure::<dyn FnMut(i32)>::new(move |handle: i32| {
                let mut table = sockets.borrow_mut();
                if let Some(slot) = table.get_mut(handle as usize) {
                    if let Some(sock) = slot.take() {
                        let _ = sock.ws.close();
                    }
                }
            })
        };

        let host_net = Object::new();
        super::set_fn(&host_net, "open", &open)?;
        super::set_fn(&host_net, "send", &send)?;
        super::set_fn(&host_net, "poll", &poll)?;
        super::set_fn(&host_net, "status", &status)?;
        super::set_fn(&host_net, "close", &close)?;
        Reflect::set(imports, &JsValue::from_str("host_net"), &host_net)?;

        Ok(NetRuntime { sockets, open, send, poll, status, close })
    }

    /// Read a length-prefixed UTF-8 string from cartridge memory at `ptr`
    /// (4 bytes LE length, then payload) — the same layout the loader's
    /// `read_string` uses. `None` on missing memory / bad length.
    fn read_string(memory: &JsValue, ptr: i32) -> Option<String> {
        if ptr < 0 || memory.is_null() {
            return None;
        }
        let buffer = Reflect::get(memory, &JsValue::from_str("buffer")).ok()?;
        let array = js_sys::Uint8Array::new(&buffer);
        let cap = array.length() as u64;
        let ptr = ptr as u64;
        // Bounds the whole [ptr, ptr+4) length prefix against the cartridge's
        // own memory. (OOB reads on a Uint8Array yield 0 in JS rather than
        // host memory — the cartridge can only see its OWN linear memory — but
        // we check explicitly so the read is well-defined and the `u32` adds
        // below can't wrap.)
        if ptr + 4 > cap {
            return None;
        }
        let mut len_bytes = [0u8; 4];
        for (i, b) in len_bytes.iter_mut().enumerate() {
            *b = array.get_index(ptr as u32 + i as u32);
        }
        let len = u32::from_le_bytes(len_bytes) as u64;
        if len > 65536 || ptr + 4 + len > cap {
            return None;
        }
        let mut bytes = vec![0u8; len as usize];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = array.get_index(ptr as u32 + 4 + i as u32);
        }
        String::from_utf8(bytes).ok()
    }

    /// Write `s` into cartridge memory at `out_ptr` as a length-prefixed
    /// UTF-8 string, truncating the payload to `max` bytes on a char
    /// boundary. Returns the payload byte length written, or -1 if memory
    /// is missing.
    fn write_string(memory: &JsValue, out_ptr: i32, s: &str, max: usize) -> i32 {
        if out_ptr < 0 || memory.is_null() {
            return -1;
        }
        let buffer = match Reflect::get(memory, &JsValue::from_str("buffer")) {
            Ok(b) => b,
            Err(_) => return -1,
        };
        let array = js_sys::Uint8Array::new(&buffer);
        let cap = array.length() as u64;
        let ptr = out_ptr as u64;

        let mut end = s.len().min(max);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let bytes = &s.as_bytes()[..end];
        let len = bytes.len() as u32;
        // The full [out_ptr, out_ptr+4+len) write region must fit the
        // cartridge's own memory. (An OOB `set_index` is a silent no-op in JS,
        // so a wild pointer can never reach host memory — but check explicitly
        // so a partial write can't land and the `u32` adds can't wrap.)
        if ptr + 4 + len as u64 > cap {
            return -1;
        }
        let ptr = ptr as u32;
        for (i, b) in len.to_le_bytes().iter().enumerate() {
            array.set_index(ptr + i as u32, *b);
        }
        for (i, b) in bytes.iter().enumerate() {
            array.set_index(ptr + 4 + i as u32, *b);
        }
        len as i32
    }
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

    use js_sys::{Function, Object, Reflect};
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

    /// Keeps the `host_audio` import closures alive for the cartridge's life
    /// (wasm holds JS references into them after instantiation).
    #[allow(dead_code)]
    pub(super) struct AudioRuntime {
        tone: Closure<dyn FnMut(i32, i32, i32) -> i32>,
        tone_at: Closure<dyn FnMut(i32, i32, i32, i32) -> i32>,
        noise: Closure<dyn FnMut(i32) -> i32>,
        stop: Closure<dyn FnMut(i32)>,
        set_volume: Closure<dyn FnMut(i32)>,
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

    /// Build the `host_audio` import object on `imports` and return the
    /// runtime that owns its closures (must outlive the wasm instance).
    pub(super) fn build_host_audio(imports: &Object) -> Result<AudioRuntime, JsValue> {
        let tone = Closure::<dyn FnMut(i32, i32, i32) -> i32>::new(
            move |freq: i32, dur_ms: i32, wave: i32| play_tone(freq, dur_ms, wave, 0),
        );
        let tone_at = Closure::<dyn FnMut(i32, i32, i32, i32) -> i32>::new(
            move |freq: i32, dur_ms: i32, wave: i32, delay_ms: i32| {
                play_tone(freq, dur_ms, wave, delay_ms)
            },
        );
        let noise = Closure::<dyn FnMut(i32) -> i32>::new(move |dur_ms: i32| play_noise(dur_ms));
        let stop = Closure::<dyn FnMut(i32)>::new(move |handle: i32| stop_handle(handle));
        let set_volume = Closure::<dyn FnMut(i32)>::new(move |pct: i32| set_master_volume(pct));

        let host_audio = Object::new();
        super::set_fn(&host_audio, "tone", &tone)?;
        super::set_fn(&host_audio, "tone_at", &tone_at)?;
        super::set_fn(&host_audio, "noise", &noise)?;
        super::set_fn(&host_audio, "stop", &stop)?;
        super::set_fn(&host_audio, "set_volume", &set_volume)?;
        Reflect::set(imports, &JsValue::from_str("host_audio"), &host_audio)?;

        Ok(AudioRuntime { tone, tone_at, noise, stop, set_volume })
    }

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
