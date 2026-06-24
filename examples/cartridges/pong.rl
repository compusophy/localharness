// pong.rl — 2-player Pong over host::mp (WebRTC P2P, off-chain-signaled).
// Open in two browsers:
//   • one taps the TOP half → HOST (left paddle) → shows a room CODE
//   • the other taps BOTTOM, types the code on the keypad, taps JOIN (right paddle)
// Move your cursor up/down to move your paddle. The HOST simulates the ball
// (authoritative) and broadcasts it; both render the same field. First to score!
//
// host::mp shared slots:
//   peer 0 (HOST, left):  0 = paddle-top-y · 1 = ball x · 2 = ball y · 3 = host score · 4 = joiner score
//   peer 1 (JOIN, right): 0 = paddle-top-y
// host::display::state_* (local only): 10 = ball vx · 12 = ball vy · 11 = ball-inited
//   100/101/102 = lobby ui (mode / entered code / my code)

fn frame(t: i32) {
    let w: i32 = host::display::width();
    let h: i32 = host::display::height();
    let ph: i32 = 44; // paddle height
    host::display::clear(0x0a0e12);

    if host::mp::connected() == 1 {
        let me: i32 = host::mp::self_index();

        // My paddle follows my cursor (centered, clamped). Host = left (peer 0),
        // joiner = right (peer 1) — each sets its OWN slot 0.
        let mut py: i32 = host::display::pointer_y() - ph / 2;
        if py < 0 {
            py = 0;
        }
        if py > h - ph {
            py = h - ph;
        }
        host::mp::set(0, py);

        // HOST is authoritative for the ball.
        if me == 0 {
            if host::display::state_get(11) == 0 {
                host::display::state_set(11, 1);
                host::mp::set(1, w / 2);
                host::mp::set(2, h / 2);
                host::display::state_set(10, 4);
                host::display::state_set(12, 3);
            }
            let mut bx: i32 = host::mp::get(0, 1);
            let mut by: i32 = host::mp::get(0, 2);
            let mut vx: i32 = host::display::state_get(10);
            let mut vy: i32 = host::display::state_get(12);
            bx = bx + vx;
            by = by + vy;
            if by < 0 {
                by = 0;
                vy = 0 - vy;
            }
            if by > h - 8 {
                by = h - 8;
                vy = 0 - vy;
            }
            // Left paddle (host) at x 12..18.
            let lp: i32 = host::mp::get(0, 0);
            if bx < 18 {
                if by + 8 > lp {
                    if by < lp + ph {
                        bx = 18;
                        vx = 0 - vx;
                    }
                }
            }
            // Right paddle (joiner) at x (w-18)..(w-12).
            let rp: i32 = host::mp::get(1, 0);
            if bx > w - 18 {
                if by + 8 > rp {
                    if by < rp + ph {
                        bx = w - 18;
                        vx = 0 - vx;
                    }
                }
            }
            // Scoring + serve.
            if bx < 0 {
                host::mp::set(4, host::mp::get(0, 4) + 1);
                bx = w / 2;
                by = h / 2;
                vx = 4;
                vy = 3;
            }
            if bx > w {
                host::mp::set(3, host::mp::get(0, 3) + 1);
                bx = w / 2;
                by = h / 2;
                vx = 0 - 4;
                vy = 3;
            }
            host::mp::set(1, bx);
            host::mp::set(2, by);
            host::display::state_set(10, vx);
            host::display::state_set(12, vy);
        }

        // RENDER (identical on both: peer 0 = left, peer 1 = right; ball/scores from host).
        let mut yy: i32 = 0;
        while yy < h {
            host::display::fill_rect(w / 2 - 1, yy, 2, 8, 0x223040);
            yy = yy + 16;
        }
        host::display::fill_rect(12, host::mp::get(0, 0), 6, ph, 0xffcc33);
        host::display::fill_rect(w - 18, host::mp::get(1, 0), 6, ph, 0x33ff88);
        host::display::fill_rect(host::mp::get(0, 1), host::mp::get(0, 2), 8, 8, 0xffffff);
        host::display::draw_number(w / 2 - 40, 10, host::mp::get(0, 3), 0xffcc33, 4);
        host::display::draw_number(w / 2 + 16, 10, host::mp::get(0, 4), 0x33ff88, 4);
        host::display::present();
    } else {
        let mode: i32 = host::display::state_get(100);
        let px: i32 = host::display::pointer_x();
        let py: i32 = host::display::pointer_y();
        let down: i32 = host::display::pointer_down();

        if mode == 0 {
            host::display::draw_string(8, 16, "PONG", 0xffffff, 3);
            host::display::draw_string(8, 56, "TAP TOP = HOST", 0xffcc33, 2);
            host::display::draw_string(8, 88, "TAP BOTTOM = JOIN", 0x33ff88, 2);
            if down == 1 {
                if py < h / 2 {
                    host::display::state_set(102, host::mp::open());
                    host::display::state_set(100, 1);
                } else {
                    host::display::state_set(100, 2);
                    host::display::state_set(101, 0);
                }
            }
        }
        if mode == 1 {
            host::display::draw_string(8, 16, "ROOM CODE", 0xffffff, 2);
            host::display::draw_number(8, 52, host::display::state_get(102), 0xffcc33, 5);
            host::display::draw_string(8, 120, "waiting for a joiner...", 0x7a8493, 1);
        }
        if mode == 2 {
            host::display::draw_string(8, 8, "ENTER CODE:", 0xffffff, 1);
            host::display::draw_number(8, 24, host::display::state_get(101), 0x33ff88, 3);
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
