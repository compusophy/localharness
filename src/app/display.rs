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
//! This is the foundation atom. Windows + compositing + input events
//! (the rest of the Orbital model) build on top of this present path.

use std::cell::RefCell;
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

/// Run the built-in demo cartridge — a hand-assembled wasm module that
/// fills the framebuffer with a gradient. Proves the cartridge → pixels
/// path end to end without needing a file in OPFS.
pub(crate) async fn run_demo() {
    if let Err(err) = run_wasm(&gradient_cartridge()).await {
        dom::set_status(&format!("display: {err:?}"), true);
    }
}

/// Instantiate `wasm_bytes` as a display cartridge and present its
/// framebuffer. The cartridge must export `memory` and a no-arg
/// `render` function, and may import `host_display.present(ptr, w, h)`.
pub(crate) async fn run_wasm(wasm_bytes: &[u8]) -> Result<(), JsValue> {
    let ctx = mount_canvas()?;

    // The `present` closure needs the cartridge's memory, which only
    // exists after instantiation. Share a slot the closure reads at call
    // time; we fill it before invoking `render`, so it's always set when
    // the cartridge calls back into us.
    let mem_slot: Rc<RefCell<Option<WebAssembly::Memory>>> = Rc::new(RefCell::new(None));

    let imports = Object::new();
    let host_display = Object::new();
    {
        let mem_slot = mem_slot.clone();
        let ctx = ctx.clone();
        let present = Closure::<dyn FnMut(i32, i32, i32)>::new(move |ptr: i32, w: i32, h: i32| {
            present_frame(&mem_slot, &ctx, ptr, w, h);
        });
        Reflect::set(
            &host_display,
            &JsValue::from_str("present"),
            present.as_ref().unchecked_ref(),
        )?;
        // The cartridge may call `present` on its own schedule later, so
        // keep the closure alive for the life of the page.
        present.forget();
    }
    Reflect::set(&imports, &JsValue::from_str("host_display"), &host_display)?;

    let result = JsFuture::from(WebAssembly::instantiate_buffer(wasm_bytes, &imports)).await?;
    let instance = Reflect::get(&result, &JsValue::from_str("instance"))?;
    let exports = Reflect::get(&instance, &JsValue::from_str("exports"))?;

    let memory = Reflect::get(&exports, &JsValue::from_str("memory"))?
        .dyn_into::<WebAssembly::Memory>()?;
    *mem_slot.borrow_mut() = Some(memory);

    let render = Reflect::get(&exports, &JsValue::from_str("render"))?
        .dyn_into::<Function>()?;
    render.call0(&JsValue::NULL)?;
    Ok(())
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

/// Hand-assembled wasm: a cartridge that writes a gradient into its
/// linear memory and calls `host_display.present(0, 256, 144)`.
///
/// We hand-roll the bytes (no `wasm-encoder` dep — same discipline as
/// the rustlite codegen) so the foundation has zero external surface.
/// The module imports `present`, declares 4 pages of memory, and exports
/// `memory` + `render`. `render` loops over every pixel writing
/// `r = x, g = y, b = 128, a = 255`.
fn gradient_cartridge() -> Vec<u8> {
    // render() body opcodes. Locals: one i32 (pixel index `p`).
    #[rustfmt::skip]
    let code: Vec<u8> = vec![
        0x02, 0x40,                   // block (void)
          0x03, 0x40,                 //   loop (void)
            0x20, 0x00,               //     local.get p
            0x41, 0x80, 0xA0, 0x02,   //     i32.const 36864  (256*144)
            0x4E,                     //     i32.ge_s
            0x0D, 0x01,               //     br_if 1  -> break block
            // red = p % 256  @ p*4 + 0
            0x20, 0x00,               //     local.get p
            0x41, 0x04, 0x6C,         //     i32.const 4 ; i32.mul  (addr = p*4)
            0x20, 0x00,               //     local.get p
            0x41, 0x80, 0x02, 0x6F,   //     i32.const 256 ; i32.rem_s
            0x3A, 0x00, 0x00,         //     i32.store8 align=0 off=0
            // green = p / 256  @ p*4 + 1
            0x20, 0x00,
            0x41, 0x04, 0x6C,
            0x20, 0x00,
            0x41, 0x80, 0x02, 0x6D,   //     i32.const 256 ; i32.div_s
            0x3A, 0x00, 0x01,
            // blue = 128  @ p*4 + 2
            0x20, 0x00,
            0x41, 0x04, 0x6C,
            0x41, 0x80, 0x01,         //     i32.const 128
            0x3A, 0x00, 0x02,
            // alpha = 255  @ p*4 + 3
            0x20, 0x00,
            0x41, 0x04, 0x6C,
            0x41, 0xFF, 0x01,         //     i32.const 255
            0x3A, 0x00, 0x03,
            // p += 1
            0x20, 0x00,
            0x41, 0x01, 0x6A,         //     i32.const 1 ; i32.add
            0x21, 0x00,               //     local.set p
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

    // Function body = local declarations + code.
    let mut body: Vec<u8> = Vec::new();
    body.push(0x01); // 1 local group
    body.push(0x01); // 1 local in the group
    body.push(0x7F); // of type i32
    body.extend_from_slice(&code);

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"\0asm");
    out.extend_from_slice(&[1, 0, 0, 0]);

    // Type section (id 1): two func types.
    //   type 0: (i32,i32,i32) -> ()   [present]
    //   type 1: () -> ()              [render]
    let mut types: Vec<u8> = Vec::new();
    leb_u(2, &mut types);
    types.extend_from_slice(&[0x60, 0x03, 0x7F, 0x7F, 0x7F, 0x00]);
    types.extend_from_slice(&[0x60, 0x00, 0x00]);
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

    // Export section (id 7): memory + render.
    let mut exports: Vec<u8> = Vec::new();
    leb_u(2, &mut exports);
    push_name("memory", &mut exports);
    exports.push(0x02); // kind: memory
    leb_u(0, &mut exports);
    push_name("render", &mut exports);
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
