//! Proof that a VARIABLE-RESOLUTION cartridge — one that declares its own
//! framebuffer dimensions via an exported `dims() -> i32` — compiles
//! end-to-end (lex → parse → typecheck → wasm) and emits the `dims` export
//! the worker reads after instantiate.
//!
//! The convention: `dims()` returns a PACKED `(width << 16) | height` (width in
//! the high 16 bits, height in the low 16). The worker (`web/cartridge-worker.js`)
//! calls it ONCE after instantiate, validates/clamps each dimension to
//! `[16, 1024]`, allocates a framebuffer of that size, and stamps every frame
//! with the chosen `w`/`h`. A cartridge with NO `dims()` export keeps the
//! 256×144 default (backward compatible) — covered by the rest of the corpus.
use localharness::rustlite;

/// Parse the wasm EXPORT section (id 7) and return the names of FUNCTION
/// exports (kind 0x00). Format: `count` then `count` × (`len` name `kind`
/// `index`). A precise check (not a substring) that `dims` is a real export.
fn function_export_names(wasm: &[u8]) -> Vec<String> {
    fn leb_u32(wasm: &[u8], i: &mut usize) -> u32 {
        let mut v = 0u32;
        let mut shift = 0;
        loop {
            let b = wasm[*i];
            *i += 1;
            v |= ((b & 0x7f) as u32) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        v
    }
    const SEC_EXPORT: u8 = 7;
    let mut i = 8usize; // skip magic (4) + version (4)
    let mut names = Vec::new();
    while i < wasm.len() {
        let id = wasm[i];
        i += 1;
        let size = leb_u32(wasm, &mut i) as usize;
        let end = i + size;
        if id == SEC_EXPORT {
            let mut p = i;
            let count = leb_u32(wasm, &mut p);
            for _ in 0..count {
                let nlen = leb_u32(wasm, &mut p) as usize;
                let name = String::from_utf8_lossy(&wasm[p..p + nlen]).to_string();
                p += nlen;
                let kind = wasm[p];
                p += 1;
                let _ = leb_u32(wasm, &mut p); // index
                if kind == 0x00 {
                    names.push(name);
                }
            }
            return names;
        }
        i = end;
    }
    names
}

#[test]
fn variable_resolution_cartridge_exports_dims() {
    // A 320×240 (4:3) cartridge: declare dims() = (320<<16)|240 and draw a
    // full-surface fill. This is the shape an agent authors to opt into a
    // non-default framebuffer size.
    let src = r#"
fn dims() -> i32 {
    (320 << 16) | 240
}
fn frame(t: i32) {
    host::display::clear(0x101010);
    host::display::fill_rect(0, 0, 320, 240, 0x3060ff);
    host::display::present();
}
"#;
    let wasm = rustlite::compile(src).expect("variable-resolution cartridge compiles");
    assert_eq!(&wasm[0..4], b"\0asm", "valid wasm magic");
    let exports = function_export_names(&wasm);
    assert!(
        exports.iter().any(|n| n == "dims"),
        "dims() must be exported so the worker can read it (got: {exports:?})",
    );
    assert!(
        exports.iter().any(|n| n == "frame"),
        "frame must still be exported (got: {exports:?})",
    );
}

#[test]
fn square_and_portrait_dims_pack_correctly() {
    // The packing is host-agnostic arithmetic, but verify a few aspect ratios
    // compile (1:1, 9:16 portrait) — the same `(w<<16)|h` shape, different
    // values. We assert compilation + the dims export; the worker validates the
    // clamp range at runtime.
    for src in [
        // 1:1 square, 512×512
        "fn dims() -> i32 { (512 << 16) | 512 } fn frame(t: i32) { host::display::clear(0); }",
        // 9:16 portrait, 144×256
        "fn dims() -> i32 { (144 << 16) | 256 } fn frame(t: i32) { host::display::clear(0); }",
        // 2:1 wide, 256×128
        "fn dims() -> i32 { (256 << 16) | 128 } fn frame(t: i32) { host::display::clear(0); }",
    ] {
        let wasm = rustlite::compile(src).expect("dims cartridge compiles");
        let exports = function_export_names(&wasm);
        assert!(
            exports.iter().any(|n| n == "dims"),
            "dims export present for: {src}",
        );
    }
}
