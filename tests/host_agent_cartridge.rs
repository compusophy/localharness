//! Proof that a cartridge using the host_agent imports compiles end-to-end
//! (lex → parse → typecheck → wasm) and emits the host_agent.* imports.
use localharness::rustlite;

#[test]
fn ready_up_cartridge_compiles_with_host_agent_imports() {
    let src = r#"
fn frame(t: i32) {
    host::display::clear(0);
    let owner: i32 = host::agent::viewer_is_owner();
    let has: i32 = host::agent::viewer_has_identity();
    host::display::fill_rect(40, 60, 176, 40, 0xffffff);
    if host::display::pointer_down() == 1 {
        if owner == 1 {
            host::agent::notify("Ready Up!", "The host is ready.");
        }
    }
    host::display::present();
}
"#;
    let wasm = rustlite::compile(src).expect("ready-up cartridge compiles");
    assert_eq!(&wasm[0..4], b"\0asm", "valid wasm magic");
    // The wasm import section must name the host_agent module + its funcs.
    let s = String::from_utf8_lossy(&wasm);
    assert!(s.contains("host_agent"), "host_agent import module present");
    assert!(s.contains("notify"), "notify import present");
    assert!(s.contains("viewer_is_owner"), "viewer_is_owner import present");
}
