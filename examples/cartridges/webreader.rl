// CORPUS: host::http — a one-shot web fetch shown on the framebuffer (issue #19).
//
// A lightweight in-sandbox web client: fire ONE https GET through the platform's
// /api/fetch proxy (the same CORS-bypassing route the agent web_fetch tool uses),
// poll the poll-model handle each frame, and once the body lands draw the
// upstream HTTP status + the body byte length as on-screen numbers — proof the
// fetch round-tripped. host::http mirrors host::net's poll model exactly:
//   get(url_ptr, url_len) -> handle   (string literal = a length-prefixed ptr)
//   ready(handle) -> 0 pending / 1 ready / <0 error
//   status(handle) -> the UPSTREAM HTTP status once ready
//   body_len(handle) -> the ready body's byte length
// (read_body(handle,out_ptr,max) copies the body into cartridge memory and
//  parse_text(html_ptr,html_len,out_ptr,max) strips tags to plain text — both
//  write length-prefixed strings, same ABI as host::net poll.)
//
// State is persisted across frames in the 64-slot register file (rustlite has no
// globals): slot 0 = phase (0 idle, 1 fetching, 2 done, 3 error), slot 1 = handle.

fn frame(t: i32) {
    host::display::clear(0x101018);

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

    // A small status bar across the top: amber pending, green ready, red error.
    let mut bar: i32 = 0xaa7700;
    if phase == 2 {
        bar = 0x119911;
    }
    if phase == 3 {
        bar = 0x991111;
    }
    host::display::fill_rect(0, 0, 256, 6, bar);

    // Once the body lands, show the upstream HTTP status + the body byte length.
    if phase == 2 {
        let h: i32 = host::display::state_get(1);
        let code: i32 = host::http::status(h);
        let len: i32 = host::http::body_len(h);
        // "HTTP" label-ish: just draw the two numbers big and clear.
        host::display::draw_char(8, 30, 72, 0xffffff, 3);   // H
        host::display::draw_number(40, 30, code, 0x66ddff, 3);
        host::display::draw_char(8, 70, 76, 0xffffff, 3);   // L (bytes)
        host::display::draw_number(40, 70, len, 0xffffff, 3);
    }

    // A blinking dot while pending, so it's obviously alive.
    if phase == 1 {
        if (t / 30) % 2 == 0 {
            host::display::fill_rect(120, 66, 16, 16, 0xffffff);
        }
    }

    host::display::present();
}
