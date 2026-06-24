// multiplayer.rl — a 2-player shared-cursor demo over host::mp (WebRTC P2P,
// off-chain-signaled). Open it in two browsers:
//   • one taps the TOP half → HOST → shows a room CODE
//   • the other taps the BOTTOM half, types that code on the keypad, taps JOIN
// Once connected, move your cursor — both players see both cursors live.
//
// State slots (host::display::state_*, local UI only):
//   100 = mode (0 lobby · 1 hosting · 2 joining)
//   101 = the code being entered (joiner)
//   102 = my room code (host)
// host::mp shared slots: 0 = my cursor x, 1 = my cursor y.

fn frame(t: i32) {
    let w: i32 = host::display::width();
    let h: i32 = host::display::height();
    host::display::clear(0x101418);

    if host::mp::connected() == 1 {
        // CONNECTED — publish my cursor, draw everyone's.
        host::mp::set(0, host::display::pointer_x());
        host::mp::set(1, host::display::pointer_y());
        let n: i32 = host::mp::peer_count();
        let me: i32 = host::mp::self_index();
        let mut i: i32 = 0;
        while i < n {
            let cx: i32 = host::mp::get(i, 0);
            let cy: i32 = host::mp::get(i, 1);
            let mut col: i32 = 0x33ff88;
            if i == me {
                col = 0xffcc33;
            }
            host::display::fill_rect(cx - 6, cy - 6, 12, 12, col);
            i = i + 1;
        }
        host::display::draw_string(8, 8, "CONNECTED - move your cursor", 0x7a8493, 1);
        host::display::present();
    } else {
        let mode: i32 = host::display::state_get(100);
        let px: i32 = host::display::pointer_x();
        let py: i32 = host::display::pointer_y();
        let down: i32 = host::display::pointer_down();

        if mode == 0 {
            host::display::draw_string(8, 20, "TAP TOP = HOST", 0xffffff, 2);
            host::display::draw_string(8, 56, "TAP BOTTOM = JOIN", 0xffffff, 2);
            if down == 1 {
                if py < h / 2 {
                    let code: i32 = host::mp::open();
                    host::display::state_set(102, code);
                    host::display::state_set(100, 1);
                } else {
                    host::display::state_set(100, 2);
                    host::display::state_set(101, 0);
                }
            }
        }
        if mode == 1 {
            host::display::draw_string(8, 16, "ROOM CODE", 0xffffff, 2);
            host::display::draw_number(8, 52, host::display::state_get(102), 0x33ff88, 5);
            host::display::draw_string(8, 120, "waiting for a joiner...", 0x7a8493, 1);
        }
        if mode == 2 {
            host::display::draw_string(8, 8, "ENTER CODE:", 0xffffff, 1);
            host::display::draw_number(8, 24, host::display::state_get(101), 0xffcc33, 3);
            let bw: i32 = w / 10;
            let mut d: i32 = 0;
            while d < 10 {
                let bx: i32 = d * bw;
                host::display::fill_rect(bx + 1, h - 30, bw - 2, 28, 0x223040);
                host::display::draw_number(bx + bw / 2 - 3, h - 22, d, 0xffffff, 1);
                d = d + 1;
            }
            host::display::fill_rect(0, h - 62, w, 26, 0x2a5a3a);
            host::display::draw_string(8, h - 54, "TAP HERE TO JOIN", 0x9fffc0, 1);
            if down == 1 {
                if py >= h - 30 {
                    let digit: i32 = px / bw;
                    host::display::state_set(101, host::display::state_get(101) * 10 + digit);
                } else {
                    if py >= h - 62 {
                        host::mp::join(host::display::state_get(101));
                    }
                }
            }
        }
        host::display::present();
    }
}
