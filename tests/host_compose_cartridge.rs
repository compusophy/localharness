//! Proof that a PARENT cartridge using the host_compose imports compiles
//! end-to-end (lex → parse → typecheck → wasm), emits the host_compose.*
//! imports, and produces structurally-valid wasm. This is the rustlite half of
//! cartridge-in-cartridge composition (the worker/display compositor pass is
//! proven separately by `scripts/test-compose-wiring.mjs`).
use localharness::rustlite;

#[test]
fn compositor_cartridge_compiles_with_host_compose_imports() {
    // A window-manager parent: mount a child into a sub-rect on the first frame
    // (state slot 0 latches "spawned"; slot 1 holds the handle), focus it so it
    // gets pointer input, drive chrome off its status, and tear it down on a
    // click in the parent's own strip. The child name is a string literal — the
    // same length-prefixed-pointer ABI as host::agent::notify.
    let src = r#"
fn frame(t: i32) {
    host::display::clear(0);
    if host::display::state_get(0) == 0 {
        let h: i32 = host::compose::spawn_module("bitmask", 96, 0, 160, 144);
        host::display::state_set(0, 1);
        host::display::state_set(1, h);
        host::compose::focus_module(h);
    }
    let h: i32 = host::display::state_get(1);
    let st: i32 = host::compose::status(h);
    // Left chrome strip the parent draws itself.
    host::display::fill_rect(0, 0, 96, 144, 0x111111);
    if st == 1 {
        host::display::draw_char(8, 8, 79, 0xffffff, 1);
    }
    let n: i32 = host::compose::module_count();
    host::display::draw_number(8, 24, n, 0xffffff, 1);
    // A click in the parent's own strip closes the panel and re-focuses parent.
    if host::display::pointer_down() == 1 {
        if host::display::pointer_x() < 96 {
            host::compose::close_module(h);
            host::compose::focus_module(0 - 1);
            host::display::state_set(0, 0);
        }
    }
    host::display::present();
}
"#;
    let wasm = rustlite::compile(src).expect("compositor cartridge compiles");
    assert_eq!(&wasm[0..4], b"\0asm", "valid wasm magic");
    let s = String::from_utf8_lossy(&wasm);
    // The wasm import section must name the host_compose module + every op used.
    assert!(s.contains("host_compose"), "host_compose import module present");
    for name in ["spawn_module", "status", "focus_module", "close_module", "module_count"] {
        assert!(s.contains(name), "import {name} present");
    }
}

#[test]
fn library_consumer_cartridge_compiles_with_call_imports() {
    // The CALLABLE-LIBRARY half (telemetry #70): mount a published cartridge
    // headless with spawn_lib, poll status, then invoke its exports with call —
    // checking call_ok(), because `call` returns the export's own i32 and 0 is
    // ambiguous. Composition as functions, not pixels.
    let src = r#"
fn frame(t: i32) {
    host::display::clear(0);
    if host::display::state_get(0) == 0 {
        host::display::state_set(0, 1);
        host::display::state_set(1, host::compose::spawn_lib("lib-physics"));
    }
    let h: i32 = host::display::state_get(1);
    if host::compose::status(h) == 1 {
        let v: i32 = host::compose::call(h, "integrate", 10, 3, 0, 0);
        if host::compose::call_ok() == 0 {
            host::display::draw_number(8, 8, v, 0xffffff, 1);
        }
    }
    host::display::present();
}
"#;
    let wasm = rustlite::compile(src).expect("library consumer compiles");
    assert_eq!(&wasm[0..4], b"\0asm", "valid wasm magic");
    let s = String::from_utf8_lossy(&wasm);
    assert!(s.contains("host_compose"), "host_compose import module present");
    for name in ["spawn_lib", "call", "call_ok"] {
        assert!(s.contains(name), "import {name} present");
    }
}

#[test]
fn library_cartridge_exports_are_callable_by_name() {
    // A LIBRARY is just a cartridge whose functions you use instead of whose
    // pixels you blit — every rustlite `fn` is already a wasm export, so no new
    // language feature is needed. It still carries a frame() (that is its
    // landing card; a headless mount never calls it).
    let src = r#"
fn gravity(v: i32) -> i32 { v + 3 }
fn integrate(p: i32, v: i32) -> i32 { p + v }
fn frame(t: i32) { host::display::present(); }
"#;
    let wasm = rustlite::compile(src).expect("library compiles");
    let s = String::from_utf8_lossy(&wasm);
    for name in ["gravity", "integrate", "frame"] {
        assert!(s.contains(name), "export {name} named in the wasm");
    }
    // The ABI can pass at most MAX_CALL_ARGS integers, so the host refuses a
    // wider export rather than calling it with garbage.
    assert_eq!(localharness::compose::MAX_CALL_ARGS, 4);
    assert_eq!(localharness::compose::call_status::OK, 0);
}

#[test]
fn compositor_cartridge_wasm_validates() {
    // Structural validation: every emitted cartridge must pass the magic+version
    // check (the codegen regression gate). A host_compose-using parent exercising
    // move_module + focused() is no exception. Full instantiation is deferred to
    // the node gate (scripts/test-compose-wiring.mjs).
    let src = r#"
fn frame(t: i32) {
    let h: i32 = host::compose::spawn_module("pong", 0, 0, 128, 144);
    host::compose::move_module(h, 0, 0, 256, 144);
    let f: i32 = host::compose::focused();
    host::display::draw_number(0, 0, f, 0xffffff, 1);
    host::display::present();
}
"#;
    let wasm = rustlite::compile(src).expect("compiles");
    assert_eq!(&wasm[0..4], b"\0asm", "magic");
    assert_eq!(&wasm[4..8], &[1, 0, 0, 0], "wasm version 1");
}
