//! DISPLAY — a pixel framebuffer surface that runs wasm cartridges.
//!
//! North star: Redox OS's **Orbital** display server. The canvas is the
//! screen (the scanout framebuffer); this module is the compositor /
//! display server; a wasm cartridge is an Orbital-style client app that
//! draws pixels into its own linear memory and hands them to us to
//! present. The cartridge ABI (`host_display`) is the Orbclient analog.
//!
//! A wasm module cannot touch the canvas, the GPU, or the DOM — it only
//! has linear memory and the imports we grant it. So the entire graphics
//! path is: cartridge writes RGBA bytes into its own memory → calls the
//! single imported `present(ptr, w, h)` → we read that memory range and
//! blit it onto a `<canvas>` via `putImageData`. That blit IS the
//! framebuffer; there is no DOM render tree involved.
//!
//! ## Cartridge ABI
//! - imports `host_display.present(ptr: i32, w: i32, h: i32)`
//! - exports `memory`
//! - exports EITHER `frame(t: i32)` (animated — driven by
//!   `requestAnimationFrame`, `t` = elapsed ms) OR `render()` (one-shot).
//!
//! The Closures here (the `present` import and the rAF tick) are the
//! wasm↔host runtime bridge, not UI/DOM event handling — a wasm import
//! *must* be a JS function. They live only in this module and never
//! build DOM, so the app's "no imperative DOM, delegated listeners only"
//! rule is untouched.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use js_sys::{Function, Object, Reflect, Uint8Array, WebAssembly};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{Clamped, JsCast};
use wasm_bindgen_futures::JsFuture;
use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement, ImageData};

use super::dom;
use super::templates;

/// Logical framebuffer resolution the demo cartridge renders at. 16:9.
/// The canvas backing store is this size; CSS scales it up with
/// `image-rendering: pixelated` so individual pixels stay crisp.
const FB_W: u32 = 256;
const FB_H: u32 = 144;

thread_local! {
    /// Generation counter for the animation loop. Each `run_wasm` bumps
    /// it; an in-flight rAF loop self-cancels when it notices the global
    /// generation has moved past the one it started with. Cleaner than
    /// tracking + cancelling rAF handles.
    static FRAME_GEN: Cell<u32> = const { Cell::new(0) };
    /// Keeps the current cartridge's `present` import closure alive for
    /// exactly as long as that cartridge runs. Replacing it on the next
    /// load drops the previous one (its loop is already cancelled).
    static PRESENT_CB: RefCell<Option<Closure<dyn FnMut(i32, i32, i32)>>> =
        const { RefCell::new(None) };
}

/// Run the built-in demo cartridge — a hand-assembled wasm module that
/// renders an animated gradient. Proves the cartridge → pixels path
/// (and the frame loop) end to end without needing a file in OPFS.
pub(crate) async fn run_demo() {
    if let Err(err) = run_wasm(&gradient_cartridge()).await {
        dom::set_status(&format!("display: {err:?}"), true);
    }
}

/// Instantiate `wasm_bytes` as a display cartridge and present its
/// framebuffer. See the module docs for the ABI.
pub(crate) async fn run_wasm(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    // Bump the generation first so any previous cartridge's frame loop
    // stops on its next tick.
    let generation = FRAME_GEN.with(|g| {
        let n = g.get().wrapping_add(1);
        g.set(n);
        n
    });

    let ctx = mount_canvas()?;

    // The `present` closure needs the cartridge's memory, which only
    // exists after instantiation. Share a slot the closure reads at call
    // time; we fill it before invoking the cartridge, so it's always set
    // when the cartridge calls back into us.
    let mem_slot: Rc<RefCell<Option<WebAssembly::Memory>>> = Rc::new(RefCell::new(None));

    let imports = Object::new();
    let host_display = Object::new();
    {
        let mem_slot = mem_slot.clone();
        let present = Closure::<dyn FnMut(i32, i32, i32)>::new(move |ptr: i32, w: i32, h: i32| {
            present_frame(&mem_slot, &ctx, ptr, w, h);
        });
        Reflect::set(
            &host_display,
            &JsValue::from_str("present"),
            present.as_ref().unchecked_ref(),
        )?;
        // Hold the closure alive in the thread-local (drops the previous
        // cartridge's, whose loop is already cancelled).
        PRESENT_CB.with(|cell| *cell.borrow_mut() = Some(present));
    }
    Reflect::set(&imports, &JsValue::from_str("host_display"), &host_display)?;

    let result = JsFuture::from(WebAssembly::instantiate_buffer(wasm_bytes, &imports)).await?;
    let instance = Reflect::get(&result, &JsValue::from_str("instance"))?;
    let exports = Reflect::get(&instance, &JsValue::from_str("exports"))?;

    let memory = Reflect::get(&exports, &JsValue::from_str("memory"))?
        .dyn_into::<WebAssembly::Memory>()?;
    *mem_slot.borrow_mut() = Some(memory);

    // Prefer an animated `frame(t)`; fall back to a one-shot `render()`.
    let frame = export_fn(&exports, "frame");
    if let Some(frame) = frame {
        start_frame_loop(frame, generation);
    } else if let Some(render) = export_fn(&exports, "render") {
        render.call0(&JsValue::NULL)?;
    } else {
        return Err(JsValue::from_str("cartridge exports neither frame nor render"));
    }
    Ok(())
}

/// Look up an exported function by name, returning `None` if it's
/// missing or not callable.
fn export_fn(exports: &JsValue, name: &str) -> Option<Function> {
    Reflect::get(exports, &JsValue::from_str(name))
        .ok()
        .and_then(|v| v.dyn_into::<Function>().ok())
}

/// Drive `frame(t)` once per `requestAnimationFrame` tick, passing
/// elapsed milliseconds since the loop started. The loop self-cancels
/// when the global generation moves past `generation` (i.e. a new
/// cartridge loaded or the surface closed).
fn start_frame_loop(frame: Function, generation: u32) {
    let start = js_sys::Date::now();
    let holder: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let holder2 = holder.clone();

    *holder.borrow_mut() = Some(Closure::wrap(Box::new(move || {
        if FRAME_GEN.with(|g| g.get()) != generation {
            // Superseded — drop our own closure and stop rescheduling.
            let _ = holder2.borrow_mut().take();
            return;
        }
        let t = (js_sys::Date::now() - start) as i32;
        // The cartridge calls `present` from inside `frame`.
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

/// Stop any running cartridge loop (e.g. when the display surface is
/// closed). Bumps the generation so the next rAF tick bails.
pub(crate) fn stop() {
    FRAME_GEN.with(|g| g.set(g.get().wrapping_add(1)));
    PRESENT_CB.with(|cell| *cell.borrow_mut() = None);
}

/// Read `w*h*4` RGBA bytes from the cartridge's memory at `ptr` and blit
/// them onto the canvas. This is the single host-side graphics op — the
/// "scanout" half of the framebuffer.
fn present_frame(
    mem_slot: &Rc<RefCell<Option<WebAssembly::Memory>>>,
    ctx: &CanvasRenderingContext2d,
    ptr: i32,
    w: i32,
    h: i32,
) {
    let slot = mem_slot.borrow();
    let Some(memory) = slot.as_ref() else { return };
    if w <= 0 || h <= 0 || ptr < 0 {
        return;
    }
    let len = (w as u32) * (h as u32) * 4;
    let buffer = memory.buffer();
    let view = Uint8Array::new_with_byte_offset_and_length(&buffer, ptr as u32, len);
    let mut data = vec![0u8; len as usize];
    view.copy_to(&mut data[..]);

    let image = match ImageData::new_with_u8_clamped_array_and_sh(
        Clamped(&data[..]),
        w as u32,
        h as u32,
    ) {
        Ok(img) => img,
        Err(_) => return,
    };
    let _ = ctx.put_image_data(&image, 0.0, 0.0);
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

// --- Built-in demo cartridge -----------------------------------------

/// Hand-assembled wasm: an animated cartridge. Exports `frame(t)` which
/// writes a gradient (`r = x + t/8`, so it scrolls horizontally over
/// time; `g = y`, `b = 128`, `a = 255`) into linear memory and calls
/// `host_display.present(0, 256, 144)`.
///
/// We hand-roll the bytes (no `wasm-encoder` dep — same discipline as
/// the rustlite codegen) so the foundation has zero external surface.
fn gradient_cartridge() -> Vec<u8> {
    // frame(t) body. Param `t` is local 0; declared local `p` is local 1.
    #[rustfmt::skip]
    let code: Vec<u8> = vec![
        0x02, 0x40,                   // block (void)
          0x03, 0x40,                 //   loop (void)
            0x20, 0x01,               //     local.get p
            0x41, 0x80, 0xA0, 0x02,   //     i32.const 36864  (256*144)
            0x4E,                     //     i32.ge_s
            0x0D, 0x01,               //     br_if 1  -> break block
            // red = (p % 256) + (t / 8)  @ p*4 + 0
            0x20, 0x01,               //     local.get p
            0x41, 0x04, 0x6C,         //     i32.const 4 ; i32.mul  (addr = p*4)
            0x20, 0x01,               //     local.get p
            0x41, 0x80, 0x02, 0x6F,   //     i32.const 256 ; i32.rem_s   (x)
            0x20, 0x00,               //     local.get t
            0x41, 0x08, 0x6D,         //     i32.const 8 ; i32.div_s     (t/8)
            0x6A,                     //     i32.add                     (x + t/8)
            0x3A, 0x00, 0x00,         //     i32.store8 align=0 off=0
            // green = p / 256  @ p*4 + 1
            0x20, 0x01,
            0x41, 0x04, 0x6C,
            0x20, 0x01,
            0x41, 0x80, 0x02, 0x6D,   //     i32.const 256 ; i32.div_s
            0x3A, 0x00, 0x01,
            // blue = 128  @ p*4 + 2
            0x20, 0x01,
            0x41, 0x04, 0x6C,
            0x41, 0x80, 0x01,         //     i32.const 128
            0x3A, 0x00, 0x02,
            // alpha = 255  @ p*4 + 3
            0x20, 0x01,
            0x41, 0x04, 0x6C,
            0x41, 0xFF, 0x01,         //     i32.const 255
            0x3A, 0x00, 0x03,
            // p += 1
            0x20, 0x01,
            0x41, 0x01, 0x6A,         //     i32.const 1 ; i32.add
            0x21, 0x01,               //     local.set p
            0x0C, 0x00,               //     br 0  -> continue loop
          0x0B,                       //   end loop
        0x0B,                         // end block
        // present(0, 256, 144)
        0x41, 0x00,                   // i32.const 0
        0x41, 0x80, 0x02,             // i32.const 256
        0x41, 0x90, 0x01,             // i32.const 144
        0x10, 0x00,                   // call 0  (imported present)
        0x0B,                         // end function
    ];

    // Function body = local declarations + code. One i32 local (`p`).
    let mut body: Vec<u8> = Vec::new();
    body.push(0x01); // 1 local group
    body.push(0x01); // 1 local in the group
    body.push(0x7F); // of type i32
    body.extend_from_slice(&code);

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"\0asm");
    out.extend_from_slice(&[1, 0, 0, 0]);

    // Type section (id 1):
    //   type 0: (i32,i32,i32) -> ()   [present]
    //   type 1: (i32) -> ()           [frame]
    let mut types: Vec<u8> = Vec::new();
    leb_u(2, &mut types);
    types.extend_from_slice(&[0x60, 0x03, 0x7F, 0x7F, 0x7F, 0x00]);
    types.extend_from_slice(&[0x60, 0x01, 0x7F, 0x00]);
    section(1, &types, &mut out);

    // Import section (id 2): host_display.present : type 0.
    let mut imports: Vec<u8> = Vec::new();
    leb_u(1, &mut imports);
    push_name("host_display", &mut imports);
    push_name("present", &mut imports);
    imports.push(0x00); // import kind: func
    leb_u(0, &mut imports); // type index 0
    section(2, &imports, &mut out);

    // Function section (id 3): one local func, type 1.
    let mut funcs: Vec<u8> = Vec::new();
    leb_u(1, &mut funcs);
    leb_u(1, &mut funcs);
    section(3, &funcs, &mut out);

    // Memory section (id 5): 1 memory, min 4 pages (256 KiB), no max.
    let mut mem: Vec<u8> = Vec::new();
    leb_u(1, &mut mem);
    mem.push(0x00); // flags: no maximum
    leb_u(4, &mut mem);
    section(5, &mem, &mut out);

    // Export section (id 7): memory + frame.
    let mut exports: Vec<u8> = Vec::new();
    leb_u(2, &mut exports);
    push_name("memory", &mut exports);
    exports.push(0x02); // kind: memory
    leb_u(0, &mut exports);
    push_name("frame", &mut exports);
    exports.push(0x00); // kind: func
    leb_u(1, &mut exports); // func index 1 (import is func 0)
    section(7, &exports, &mut out);

    // Code section (id 10): one body.
    let mut codesec: Vec<u8> = Vec::new();
    leb_u(1, &mut codesec);
    leb_u(body.len() as u32, &mut codesec);
    codesec.extend_from_slice(&body);
    section(10, &codesec, &mut out);

    out
}

fn section(id: u8, payload: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    leb_u(payload.len() as u32, out);
    out.extend_from_slice(payload);
}

fn push_name(name: &str, out: &mut Vec<u8>) {
    leb_u(name.len() as u32, out);
    out.extend_from_slice(name.as_bytes());
}

fn leb_u(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let mut byte = (v & 0x7F) as u8;
        v >>= 7;
        if v != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if v == 0 {
            break;
        }
    }
}
