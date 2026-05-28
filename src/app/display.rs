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

/// Keeps every `host_display` import closure alive. wasm holds JS
/// function references into these; they must outlive the instance.
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
}

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

    let (imports, runtime) = build_host_display(&fb, &ctx)?;
    // Hold the closures alive (drops the previous cartridge's set).
    RUNTIME.with(|cell| *cell.borrow_mut() = Some(runtime));

    let result = JsFuture::from(WebAssembly::instantiate_buffer(wasm_bytes, &imports)).await?;
    let instance = Reflect::get(&result, &JsValue::from_str("instance"))?;
    let exports = Reflect::get(&instance, &JsValue::from_str("exports"))?;

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
) -> Result<(Object, CartridgeRuntime), JsValue> {
    let clear = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32)>::new(move |rgb: i32| {
            let (r, g, b) = rgb_components(rgb);
            let mut buf = fb.borrow_mut();
            let mut i = 0;
            while i < buf.len() {
                buf[i] = r;
                buf[i + 1] = g;
                buf[i + 2] = b;
                buf[i + 3] = 255;
                i += 4;
            }
        })
    };

    let set_pixel = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32)>::new(move |x: i32, y: i32, rgb: i32| {
            if x < 0 || y < 0 || x >= FB_W as i32 || y >= FB_H as i32 {
                return;
            }
            let (r, g, b) = rgb_components(rgb);
            let mut buf = fb.borrow_mut();
            let idx = ((y as usize) * (FB_W as usize) + (x as usize)) * 4;
            buf[idx] = r;
            buf[idx + 1] = g;
            buf[idx + 2] = b;
            buf[idx + 3] = 255;
        })
    };

    let fill_rect = {
        let fb = fb.clone();
        Closure::<dyn FnMut(i32, i32, i32, i32, i32)>::new(
            move |x: i32, y: i32, w: i32, h: i32, rgb: i32| {
                let (r, g, b) = rgb_components(rgb);
                let x0 = x.max(0);
                let y0 = y.max(0);
                let x1 = x.saturating_add(w).min(FB_W as i32);
                let y1 = y.saturating_add(h).min(FB_H as i32);
                let mut buf = fb.borrow_mut();
                let mut yy = y0;
                while yy < y1 {
                    let mut xx = x0;
                    while xx < x1 {
                        let idx = ((yy as usize) * (FB_W as usize) + (xx as usize)) * 4;
                        buf[idx] = r;
                        buf[idx + 1] = g;
                        buf[idx + 2] = b;
                        buf[idx + 3] = 255;
                        xx += 1;
                    }
                    yy += 1;
                }
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

    Ok((
        imports,
        CartridgeRuntime {
            clear, set_pixel, fill_rect, draw_char, draw_number, present, width, height,
            pointer_x, pointer_y, pointer_down, state_get, state_set,
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
/// Covers digits, A-Z, space, and the operators a calculator/label app
/// needs; unknown codes render as a hollow box. Hand-encoded (no font
/// dep) and verified by rendering every glyph to ASCII art.
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
        0x2B => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],  // +
        0x2D => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],  // -
        0x2A => [0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00],  // *
        0x2F => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],  // /
        0x3D => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],  // =
        0x2E => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],  // .
        0x28 => [0x04, 0x08, 0x10, 0x10, 0x10, 0x08, 0x04],  // (
        0x29 => [0x04, 0x02, 0x01, 0x01, 0x01, 0x02, 0x04],  // )
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
