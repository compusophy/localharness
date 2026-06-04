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
    present: Closure<dyn FnMut()>,
    width: Closure<dyn FnMut() -> i32>,
    height: Closure<dyn FnMut() -> i32>,
    pointer_x: Closure<dyn FnMut() -> i32>,
    pointer_y: Closure<dyn FnMut() -> i32>,
    pointer_down: Closure<dyn FnMut() -> i32>,
    state_get: Closure<dyn FnMut(i32) -> i32>,
    state_set: Closure<dyn FnMut(i32, i32)>,
    net: net::NetRuntime,
}

/// Shared handle to the cartridge's linear memory. The `host_net`
/// closures read/write length-prefixed strings through it, but memory
/// only exists after instantiation — so the closures hold this cell and
/// `run_with_ctx` fills it in once the instance is live.
type SharedMemory = Rc<RefCell<JsValue>>;

/// Instantiate `wasm_bytes` as a display cartridge in the workshop's
/// center view panel (swaps in the surface template). Used by the
/// `run_cartridge` tool and opening a `.wasm` from the files panel.
pub(crate) async fn run_wasm(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    let ctx = mount_canvas()?;
    run_with_ctx(wasm_bytes, ctx).await
}

/// Instantiate `wasm_bytes` against an existing `#display-canvas`
/// already in the DOM (app mode — the subdomain booted straight into a
/// fullscreen cartridge, no view-panel swap).
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

/// Shared core: wire the host imports over a fresh framebuffer, reset
/// per-cartridge input/state, instantiate, and start the frame loop (or
/// one-shot render).
async fn run_with_ctx(
    wasm_bytes: &[u8],
    ctx: CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    // Bump the generation first so any previous cartridge's frame loop
    // stops on its next tick.
    let generation = FRAME_GEN.with(|g| {
        let n = g.get().wrapping_add(1);
        g.set(n);
        n
    });

    let fb: Framebuffer = Rc::new(RefCell::new(black_framebuffer()));

    // Fresh cartridge starts with cleared input + state.
    POINTER_DOWN.with(|d| d.set(0));
    STATE.with(|s| *s.borrow_mut() = [0; 64]);

    // `host_net` needs the cartridge's linear memory, which only exists
    // after instantiation. Share a cell the net closures read lazily.
    let mem_cell: SharedMemory = Rc::new(RefCell::new(JsValue::NULL));

    let (imports, runtime) = build_host_display(&fb, &ctx, &mem_cell)?;
    // Hold the closures alive (drops the previous cartridge's set).
    RUNTIME.with(|cell| *cell.borrow_mut() = Some(runtime));

    let result = JsFuture::from(WebAssembly::instantiate_buffer(wasm_bytes, &imports)).await?;
    let instance = Reflect::get(&result, &JsValue::from_str("instance"))?;
    let exports = Reflect::get(&instance, &JsValue::from_str("exports"))?;
    // Wire memory into the `host_net` closures now that it exists.
    *mem_cell.borrow_mut() = Reflect::get(&exports, &JsValue::from_str("memory"))?;

    // Prefer an animated `frame(t)`; fall back to a one-shot `render()`.
    if let Some(frame) = export_fn(&exports, "frame") {
        start_frame_loop(frame, generation);
    } else if let Some(render) = export_fn(&exports, "render") {
        render.call0(&JsValue::NULL)?;
    } else {
        return Err(JsValue::from_str("cartridge exports neither frame nor render"));
    }
    Ok(())
}

/// Build the `host_display` import object (the Orbclient-style draw API)
/// over a shared host-owned framebuffer, plus the runtime that keeps the
/// closures alive.
fn build_host_display(
    fb: &Framebuffer,
    ctx: &CanvasRenderingContext2d,
    mem: &SharedMemory,
) -> Result<(Object, CartridgeRuntime), JsValue> {
    // The single-cartridge path draws through a full-screen viewport (identity
    // transform). The pixel math lives in `crate::raster` (pure, native-tested)
    // so host::compose can give a child a sub-rect viewport without touching
    // these closures' shape. See `src/raster.rs` / design/host-compose.md.
    let vp = crate::raster::Viewport::full(FB_W as i32, FB_H as i32);

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

    let present = {
        let fb = fb.clone();
        let ctx = ctx.clone();
        Closure::<dyn FnMut()>::new(move || {
            let buf = fb.borrow();
            if let Ok(img) =
                ImageData::new_with_u8_clamped_array_and_sh(Clamped(&buf[..]), FB_W, FB_H)
            {
                let _ = ctx.put_image_data(&img, 0.0, 0.0);
            }
        })
    };

    let draw_char = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32)>::new(
            move |x: i32, y: i32, code: i32, rgb: i32, scale: i32| {
                let mut buf = fb.borrow_mut();
                blit_glyph(&mut buf, x, y, code as u32, rgb_components(rgb), scale.max(1));
            },
        )
    };

    let draw_number = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32)>::new(
            move |x: i32, y: i32, value: i32, rgb: i32, scale: i32| {
                let color = rgb_components(rgb);
                let s = scale.max(1);
                let advance = 6 * s; // 5px glyph + 1px gap, scaled
                let mut buf = fb.borrow_mut();
                let mut cx = x;
                let mut n = (value as i64).unsigned_abs();
                if value < 0 {
                    blit_glyph(&mut buf, cx, y, '-' as u32, color, s);
                    cx += advance;
                }
                // Collect digits (least-significant first), then draw reversed.
                let mut digits = [0u8; 20];
                let mut count = 0;
                if n == 0 {
                    digits[0] = b'0';
                    count = 1;
                } else {
                    while n > 0 {
                        digits[count] = b'0' + (n % 10) as u8;
                        n /= 10;
                        count += 1;
                    }
                }
                for i in (0..count).rev() {
                    blit_glyph(&mut buf, cx, y, digits[i] as u32, color, s);
                    cx += advance;
                }
            },
        )
    };

    let width = Closure::<dyn FnMut() -> i32>::new(move || FB_W as i32);
    let height = Closure::<dyn FnMut() -> i32>::new(move || FB_H as i32);
    let pointer_x = Closure::<dyn FnMut() -> i32>::new(move || POINTER.with(|p| p.get().0));
    let pointer_y = Closure::<dyn FnMut() -> i32>::new(move || POINTER.with(|p| p.get().1));
    let pointer_down = Closure::<dyn FnMut() -> i32>::new(move || POINTER_DOWN.with(|d| d.get()));
    let state_get = Closure::<dyn FnMut(i32) -> i32>::new(move |slot: i32| {
        if !(0..64).contains(&slot) {
            return 0;
        }
        STATE.with(|s| s.borrow()[slot as usize])
    });
    let state_set = Closure::<dyn FnMut(i32, i32)>::new(move |slot: i32, value: i32| {
        if !(0..64).contains(&slot) {
            return;
        }
        STATE.with(|s| s.borrow_mut()[slot as usize] = value);
    });

    let host_display = Object::new();
    set_fn(&host_display, "clear", &clear)?;
    set_fn(&host_display, "set_pixel", &set_pixel)?;
    set_fn(&host_display, "fill_rect", &fill_rect)?;
    set_fn(&host_display, "draw_char", &draw_char)?;
    set_fn(&host_display, "draw_number", &draw_number)?;
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

    Ok((
        imports,
        CartridgeRuntime {
            clear, set_pixel, fill_rect, draw_char, draw_number, present, width, height,
            pointer_x, pointer_y, pointer_down, state_get, state_set, net,
        },
    ))
}

/// Draw one 5x7 glyph into `buf` at `(x, y)`, each source pixel expanded
/// to a `scale`x`scale` block. Out-of-bounds pixels are clipped.
fn blit_glyph(buf: &mut [u8], x: i32, y: i32, code: u32, color: (u8, u8, u8), scale: i32) {
    let glyph = glyph_5x7(code);
    let (r, g, b) = color;
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5 {
            if (bits >> (4 - col)) & 1 == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    let px = x + col * scale + dx;
                    let py = y + row as i32 * scale + dy;
                    if px < 0 || py < 0 || px >= FB_W as i32 || py >= FB_H as i32 {
                        continue;
                    }
                    let idx = ((py as usize) * (FB_W as usize) + (px as usize)) * 4;
                    buf[idx] = r;
                    buf[idx + 1] = g;
                    buf[idx + 2] = b;
                    buf[idx + 3] = 255;
                }
            }
        }
    }
}

/// 5x7 bitmap font. Each row's low 5 bits are pixels (bit 4 = leftmost).
/// Covers digits, A-Z, a-z, space, and common punctuation; unknown codes
/// render as a hollow box. Hand-encoded (no font dep) and verified by
/// rendering every glyph to ASCII art. Lowercase + punctuation were added
/// so the HTML renderer (and text-heavy cartridges) read cleanly.
fn glyph_5x7(c: u32) -> [u8; 7] {
    match c {
        0x20 => [0, 0, 0, 0, 0, 0, 0],                       // space
        0x30 => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],  // 0
        0x31 => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],  // 1
        0x32 => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],  // 2
        0x33 => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],  // 3
        0x34 => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],  // 4
        0x35 => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],  // 5
        0x36 => [0x0E, 0x10, 0x10, 0x1E, 0x11, 0x11, 0x0E],  // 6
        0x37 => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],  // 7
        0x38 => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],  // 8
        0x39 => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x0E],  // 9
        // Punctuation / symbols.
        0x21 => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],  // !
        0x22 => [0x0A, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00],  // "
        0x23 => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],  // #
        0x25 => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],  // %
        0x26 => [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D],  // &
        0x27 => [0x04, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00],  // '
        0x28 => [0x04, 0x08, 0x10, 0x10, 0x10, 0x08, 0x04],  // (
        0x29 => [0x04, 0x02, 0x01, 0x01, 0x01, 0x02, 0x04],  // )
        0x2A => [0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00],  // *
        0x2B => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],  // +
        0x2C => [0x00, 0x00, 0x00, 0x00, 0x06, 0x04, 0x08],  // ,
        0x2D => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],  // -
        0x2E => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],  // .
        0x2F => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],  // /
        0x3A => [0x00, 0x06, 0x06, 0x00, 0x06, 0x06, 0x00],  // :
        0x3B => [0x00, 0x06, 0x06, 0x00, 0x06, 0x04, 0x08],  // ;
        0x3C => [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02],  // <
        0x3D => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],  // =
        0x3E => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],  // >
        0x3F => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],  // ?
        0x40 => [0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0E],  // @
        0x5B => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],  // [
        0x5D => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],  // ]
        0x5F => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],  // _
        // Uppercase A-Z.
        0x41 => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],  // A
        0x42 => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],  // B
        0x43 => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],  // C
        0x44 => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],  // D
        0x45 => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],  // E
        0x46 => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],  // F
        0x47 => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],  // G
        0x48 => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],  // H
        0x49 => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],  // I
        0x4A => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],  // J
        0x4B => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],  // K
        0x4C => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],  // L
        0x4D => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],  // M
        0x4E => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],  // N
        0x4F => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],  // O
        0x50 => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],  // P
        0x51 => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],  // Q
        0x52 => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],  // R
        0x53 => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],  // S
        0x54 => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],  // T
        0x55 => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],  // U
        0x56 => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],  // V
        0x57 => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],  // W
        0x58 => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],  // X
        0x59 => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],  // Y
        0x5A => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],  // Z
        // Lowercase a-z.
        0x61 => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],  // a
        0x62 => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E],  // b
        0x63 => [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E],  // c
        0x64 => [0x01, 0x01, 0x0D, 0x13, 0x11, 0x11, 0x0F],  // d
        0x65 => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],  // e
        0x66 => [0x06, 0x09, 0x08, 0x1C, 0x08, 0x08, 0x08],  // f
        0x67 => [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x0E],  // g
        0x68 => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],  // h
        0x69 => [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E],  // i
        0x6A => [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C],  // j
        0x6B => [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],  // k
        0x6C => [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],  // l
        0x6D => [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11],  // m
        0x6E => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],  // n
        0x6F => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],  // o
        0x70 => [0x00, 0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10],  // p
        0x71 => [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x01],  // q
        0x72 => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],  // r
        0x73 => [0x00, 0x00, 0x0F, 0x10, 0x0E, 0x01, 0x1E],  // s
        0x74 => [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06],  // t
        0x75 => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D],  // u
        0x76 => [0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04],  // v
        0x77 => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A],  // w
        0x78 => [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11],  // x
        0x79 => [0x00, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x0E],  // y
        0x7A => [0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F],  // z
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F],     // unknown -> box
    }
}

/// Update the primary-button state from mousedown/mouseup over the
/// canvas. Called from the delegated listeners in `events.rs`.
pub(crate) fn set_pointer_down(down: bool) {
    POINTER_DOWN.with(|d| d.set(if down { 1 } else { 0 }));
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

/// Drive `frame(t)` once per `requestAnimationFrame` tick, passing
/// elapsed milliseconds since the loop started. Self-cancels when the
/// global generation moves past `generation`.
fn start_frame_loop(frame: Function, generation: u32) {
    let start = js_sys::Date::now();
    let holder: FrameLoopHolder = Rc::new(RefCell::new(None));
    let holder2 = holder.clone();

    *holder.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        if FRAME_GEN.with(|g| g.get()) != generation {
            let _ = holder2.borrow_mut().take();
            return;
        }
        let t = (js_sys::Date::now() - start) as i32;
        let _ = frame.call1(&JsValue::NULL, &JsValue::from(t));
        if let Some(cb) = holder2.borrow().as_ref() {
            let _ = request_af(cb);
        }
    }) as Box<dyn FnMut()>));

    if let Some(cb) = holder.borrow().as_ref() {
        let _ = request_af(cb);
    }
}

fn request_af(cb: &Closure<dyn FnMut()>) -> Result<i32, JsValue> {
    dom::window()?.request_animation_frame(cb.as_ref().unchecked_ref())
}

/// Stop any running cartridge loop (e.g. when the surface is closed).
pub(crate) fn stop() {
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    RUNTIME.with(|cell| *cell.borrow_mut() = None);
}

/// Render the workshop canvas template into the center view-panel, then
/// size + grab its 2D context.
fn mount_canvas() -> Result<CanvasRenderingContext2d, JsValue> {
    dom::swap_inner("view-content", &templates::display_surface().into_string());
    super::opfs::set_view_collapsed(false);
    size_and_get_ctx()
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
            for ch in line.chars() {
                blit_glyph(&mut buf, x, y, ch as u32, color, scale);
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

                let mut table = sockets.borrow_mut();
                let handle = table.len() as i32;
                table.push(Some(Socket { ws, inbox, _on_message: on_message }));
                handle
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
        let ptr = ptr as u32;
        let mut len_bytes = [0u8; 4];
        for (i, b) in len_bytes.iter_mut().enumerate() {
            *b = array.get_index(ptr + i as u32);
        }
        let len = u32::from_le_bytes(len_bytes);
        if len > 65536 {
            return None;
        }
        let mut bytes = vec![0u8; len as usize];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = array.get_index(ptr + 4 + i as u32);
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

        let mut end = s.len().min(max);
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        let bytes = &s.as_bytes()[..end];
        let len = bytes.len() as u32;
        let ptr = out_ptr as u32;
        for (i, b) in len.to_le_bytes().iter().enumerate() {
            array.set_index(ptr + i as u32, *b);
        }
        for (i, b) in bytes.iter().enumerate() {
            array.set_index(ptr + 4 + i as u32, *b);
        }
        len as i32
    }
}
