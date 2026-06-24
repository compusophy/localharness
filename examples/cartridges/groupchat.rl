// groupchat.localharness.xyz — OPEN CHATROOM (a cartridge, no browser chrome).
//
// A self-contained chat client on the framebuffer: a scrolling message log up top,
// the line you're typing in the middle, and an on-screen tap keyboard at the bottom.
// All text lives HOST-side (rustlite has no String/Vec) and crosses the integer-only
// host ABI as codepoints — host::chat::* reads the received-line ring + the outgoing
// compose buffer, and posts/polls the off-chain /api/chat relay (room = this
// subdomain). Anyone with an identity can type; everyone sees the same log.
//
// State (the 64-slot register file): slot 0 = the pointer's down-state LAST frame,
// for edge-triggered taps (act once on the press, not every held frame).

// One keyboard cap: a filled key with its glyph centred. Multi-char labels (DEL/
// SEND/SPACE) are drawn by the caller with draw_string instead.
fn key_cap(x: i32, y: i32, w: i32, cp: i32) {
    host::display::fill_rect(x + 1, y, w - 2, 14, 3355443); // 0x333333
    host::display::draw_char(x + w / 2 - 3, y + 4, cp, 16777215, 1);
}

fn frame(t: i32) {
    host::display::clear(657426); // 0x0a0a12

    // poll() keeps the relay-poll loop alive AND returns the live line count.
    let lc: i32 = host::chat::poll();

    // Key rows as ASCII codepoints (QWERTY).
    let row_a = [113, 119, 101, 114, 116, 121, 117, 105, 111, 112]; // q w e r t y u i o p
    let row_b = [97, 115, 100, 102, 103, 104, 106, 107, 108];       // a s d f g h j k l
    let row_c = [122, 120, 99, 118, 98, 110, 109];                  // z x c v b n m

    // ── input: edge-triggered taps on the on-screen keyboard ──────────────────
    let prev: i32 = host::display::state_get(0);
    let down: i32 = host::display::pointer_down();
    if down == 1 && prev == 0 {
        let px: i32 = host::display::pointer_x();
        let py: i32 = host::display::pointer_y();
        // row A (q..p): 10 keys, 32 wide, from x=0
        if py >= 174 && py < 189 {
            let col: i32 = px / 32;
            if col >= 0 && col < 10 {
                host::chat::key(row_a[col]);
            }
        }
        // row B (a..l): 9 keys, 32 wide, indented 16
        if py >= 190 && py < 205 {
            if px >= 16 {
                let col: i32 = (px - 16) / 32;
                if col >= 0 && col < 9 {
                    host::chat::key(row_b[col]);
                }
            }
        }
        // row C (z..m + DEL): 7 keys then a backspace cap
        if py >= 206 && py < 221 {
            if px < 224 {
                let col: i32 = px / 32;
                if col >= 0 && col < 7 {
                    host::chat::key(row_c[col]);
                }
            } else {
                host::chat::backspace();
            }
        }
        // row D: SPACE (left) + SEND (right)
        if py >= 222 && py < 237 {
            if px < 224 {
                host::chat::key(32);
            } else {
                let sent: i32 = host::chat::send();
            }
        }
    }
    host::display::state_set(0, down);

    // ── header ────────────────────────────────────────────────────────────────
    host::display::draw_string(2, 3, "GROUPCHAT", 16777215, 1);
    host::display::draw_number(76, 3, lc, 6741247, 1); // 0x66ddff line count
    host::display::fill_rect(0, 12, 320, 1, 3355443);

    // ── message log: the last 12 lines, oldest first ─────────────────────────
    let mut first: i32 = 0;
    if lc > 12 {
        first = lc - 12;
    }
    let mut row: i32 = 0;
    let mut li: i32 = first;
    while li < lc {
        let len: i32 = host::chat::line_len(li);
        let mut cx: i32 = 2;
        let mut cj: i32 = 0;
        while cj < len && cx < 314 {
            let cp: i32 = host::chat::line_char(li, cj);
            host::display::draw_char(cx, 16 + row * 11, cp, 13421772, 1); // 0xcccccc
            cx = cx + 6;
            cj = cj + 1;
        }
        row = row + 1;
        li = li + 1;
    }

    // ── compose line: "> " + what you're typing + a blinking cursor ──────────
    host::display::fill_rect(0, 150, 320, 1, 3355443);
    host::display::draw_char(2, 154, 62, 16777215, 1); // '>'
    let clen: i32 = host::chat::compose_len();
    let mut cx2: i32 = 12;
    let mut ci: i32 = 0;
    while ci < clen && cx2 < 314 {
        let cp2: i32 = host::chat::compose_char(ci);
        host::display::draw_char(cx2, 154, cp2, 16777215, 1);
        cx2 = cx2 + 6;
        ci = ci + 1;
    }
    if (t / 30) % 2 == 0 {
        host::display::fill_rect(cx2, 154, 5, 8, 16777215);
    }

    // ── on-screen keyboard ────────────────────────────────────────────────────
    let mut a: i32 = 0;
    while a < 10 {
        key_cap(a * 32, 174, 32, row_a[a]);
        a = a + 1;
    }
    let mut b: i32 = 0;
    while b < 9 {
        key_cap(16 + b * 32, 190, 32, row_b[b]);
        b = b + 1;
    }
    let mut c: i32 = 0;
    while c < 7 {
        key_cap(c * 32, 206, 32, row_c[c]);
        c = c + 1;
    }
    // DEL cap
    host::display::fill_rect(225, 206, 94, 14, 5588259); // 0x553333
    host::display::draw_string(252, 210, "DEL", 16777215, 1);
    // SPACE bar + SEND
    host::display::fill_rect(1, 222, 222, 14, 3355443);
    host::display::draw_string(146, 226, "SPACE", 8947848, 1); // 0x888888
    host::display::fill_rect(225, 222, 94, 14, 3364659); // 0x335533
    host::display::draw_string(250, 226, "SEND", 16777215, 1);

    host::display::present();
}
