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
//! - `present()` — flush the framebuffer to the canvas
//! - `width() -> i32`, `height() -> i32`
//! - `pointer_x() -> i32`, `pointer_y() -> i32` — cursor position in
//!   framebuffer coordinates (poll model, like Orbclient's event queue)
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
}

/// Keeps every `host_display` import closure alive. wasm holds JS
/// function references into these; they must outlive the instance.
#[allow(dead_code)]
struct CartridgeRuntime {
    clear: Closure<dyn FnMut(i32)>,
    set_pixel: Closure<dyn FnMut(i32, i32, i32)>,
    fill_rect: Closure<dyn FnMut(i32, i32, i32, i32, i32)>,
    present: Closure<dyn FnMut()>,
    width: Closure<dyn FnMut() -> i32>,
    height: Closure<dyn FnMut() -> i32>,
    pointer_x: Closure<dyn FnMut() -> i32>,
    pointer_y: Closure<dyn FnMut() -> i32>,
}

/// Run the built-in demo: a rustlite cartridge compiled in-browser.
/// Proves the whole loop — Rust source → wasm → host draw calls → pixels.
pub(crate) async fn run_demo() {
    const SRC: &str = r#"
use host::display;
fn frame(t: i32) {
    display::clear(1118481);
    let bar: i32 = t / 16 % 256;
    display::fill_rect(bar, 0, 4, 144, 4473924);
    let px: i32 = display::pointer_x();
    let py: i32 = display::pointer_y();
    display::fill_rect(px - 16, py - 16, 32, 32, 16777215);
    display::present();
}
"#;
    match crate::rustlite::compile(SRC) {
        Ok(wasm) => {
            if let Err(err) = run_wasm(&wasm).await {
                dom::set_status(&format!("display: {err:?}"), true);
            }
        }
        Err(e) => dom::set_status(&format!("display compile: {e}"), true),
    }
}

/// Instantiate `wasm_bytes` as a display cartridge and run it. See the
/// module docs for the ABI.
pub(crate) async fn run_wasm(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    // Bump the generation first so any previous cartridge's frame loop
    // stops on its next tick.
    let generation = FRAME_GEN.with(|g| {
        let n = g.get().wrapping_add(1);
        g.set(n);
        n
    });

    let ctx = mount_canvas()?;
    let fb: Framebuffer = Rc::new(RefCell::new(black_framebuffer()));

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

    let width = Closure::<dyn FnMut() -> i32>::new(move || FB_W as i32);
    let height = Closure::<dyn FnMut() -> i32>::new(move || FB_H as i32);
    let pointer_x = Closure::<dyn FnMut() -> i32>::new(move || POINTER.with(|p| p.get().0));
    let pointer_y = Closure::<dyn FnMut() -> i32>::new(move || POINTER.with(|p| p.get().1));

    let host_display = Object::new();
    set_fn(&host_display, "clear", &clear)?;
    set_fn(&host_display, "set_pixel", &set_pixel)?;
    set_fn(&host_display, "fill_rect", &fill_rect)?;
    set_fn(&host_display, "present", &present)?;
    set_fn(&host_display, "width", &width)?;
    set_fn(&host_display, "height", &height)?;
    set_fn(&host_display, "pointer_x", &pointer_x)?;
    set_fn(&host_display, "pointer_y", &pointer_y)?;

    let imports = Object::new();
    Reflect::set(&imports, &JsValue::from_str("host_display"), &host_display)?;

    Ok((
        imports,
        CartridgeRuntime {
            clear, set_pixel, fill_rect, present, width, height, pointer_x, pointer_y,
        },
    ))
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
    let holder: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
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

/// Render the canvas template into the center view-panel, size its
/// backing store to the logical framebuffer, and return its 2D context.
fn mount_canvas() -> Result<CanvasRenderingContext2d, JsValue> {
    dom::swap_inner("view-content", &templates::display_surface().into_string());
    super::opfs::set_view_collapsed(false);

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
