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

#[test]
fn ready_up_feed_cartridge_compiles() {
    // The full "Ready Up" surface: subscribe toggle + member count + a
    // broadcast button + identity gate. This is the shape a Gemini-Flash
    // agent should be able to author.
    let src = r#"
fn frame(t: i32) {
    host::display::clear(0);
    let has: i32 = host::agent::viewer_has_identity();
    if has == 0 {
        host::agent::request_identity();
    }
    let subbed: i32 = host::agent::is_subscribed();
    let count: i32 = host::agent::subscriber_count();
    host::display::draw_number(8, 8, count, 0xffffff, 2);
    if host::display::pointer_down() == 1 {
        if subbed == 0 {
            host::agent::subscribe();
        } else {
            host::agent::unsubscribe();
        }
        host::agent::broadcast("Ready Up!", "Time to play.");
    }
    host::display::present();
}
"#;
    let wasm = rustlite::compile(src).expect("ready-up feed cartridge compiles");
    let s = String::from_utf8_lossy(&wasm);
    for name in ["host_agent", "subscribe", "unsubscribe", "is_subscribed", "subscriber_count", "broadcast", "request_identity"] {
        assert!(s.contains(name), "import {name} present");
    }
}
