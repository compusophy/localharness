// p05_http_status_probe — reference solution.
//
// A one-shot HTTPS GET through host::http, surfaced on the framebuffer using
// the host::http POLL MODEL (mirrors host::net): fire get() once, poll ready()
// each frame, then read status()/body_len() out once it lands. rustlite has no
// globals, so the phase + handle live in the 64 state slots (slot 0 = phase,
// slot 1 = handle). The URL is a string literal — the only pointer rustlite
// can produce — passed as a length-prefixed pointer.

fn frame(t: i32) {
    host::display::clear(1054744); // 0x101018

    let mut phase: i32 = host::display::state_get(0);

    // Phase 0: fire the GET exactly once, then move to "fetching".
    if phase == 0 {
        let h: i32 = host::http::get("https://example.com/", 20);
        host::display::state_set(1, h);
        if h < 0 {
            host::display::state_set(0, 3); // bad url / cap -> error
        } else {
            host::display::state_set(0, 1);
        }
        phase = host::display::state_get(0);
    }

    // Phase 1: poll the handle each frame until it resolves.
    if phase == 1 {
        let h: i32 = host::display::state_get(1);
        let r: i32 = host::http::ready(h);
        if r == 1 {
            host::display::state_set(0, 2); // ready
        } else {
            if r < 0 {
                host::display::state_set(0, 3); // fetch failed / denied
            }
        }
        phase = host::display::state_get(0);
    }

    // A status bar across the top: amber pending, green ready, red error.
    let mut bar: i32 = 11171584; // 0xAA7700 amber
    if phase == 2 {
        bar = 1153809; // 0x119911 green
    }
    if phase == 3 {
        bar = 10031889; // 0x991111 red
    }
    host::display::fill_rect(0, 0, 256, 6, bar);

    // Once the body lands, show the upstream HTTP status + body byte length.
    if phase == 2 {
        let h: i32 = host::display::state_get(1);
        let code: i32 = host::http::status(h);
        let len: i32 = host::http::body_len(h);
        host::display::draw_char(8, 30, 72, 16777215, 3);   // 'H'
        host::display::draw_number(40, 30, code, 6741247, 3);
        host::display::draw_char(8, 70, 76, 16777215, 3);   // 'L' (bytes)
        host::display::draw_number(40, 70, len, 16777215, 3);
    }

    host::display::present();
}
