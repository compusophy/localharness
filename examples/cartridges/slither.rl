// slither.localharness.xyz — MULTIPLAYER SLITHER (eat-or-be-eaten), 512x512.
//
// A proof-of-spec for the host::mp low-latency stack: anyone opens or joins a room
// (4-digit code), steers a snake toward the cursor, eats the glowing food to grow,
// and dies if their HEAD touches ANY OTHER snake's body (slither rules — you can't
// die on yourself). The host plays SOLO too. All integer math (rustlite has no
// floats/trig): motion from a fixed-point direction table, steering by dot-product
// argmax (no atan2), collision by squared distance (no sqrt), food from a
// deterministic LCG everyone agrees on.
//
// NETWORK STATE (each peer owns its 32 mp slots = its snake; peer-authoritative):
//   slot 0      meta = alive(0/1) + length*256 + head_ptr*65536
//   slot 1..30  ring of 30 body waypoints, packed pos = x*512 + y (newest at 1+ptr)
//   slot 31     eat_count (score; the sum across all peers picks the shared food)
// LOCAL STATE (host::display::state, 64 slots): 0 head_x_fp, 1 head_y_fp, 2 dir,
//   3/4 last-waypoint px, 5 started, 6 length, 7 head_ptr, 8 alive, 9 eat_count,
//   10 invuln, 40 mode(0 lobby/1 host/2 keypad), 41 entered_code, 42 my_code,
//   43 prev_pointer_down.

fn dims() -> i32 { 512 * 65536 + 512 }

// Deterministic LCG (glibc multiplier); i32 mul WRAPS in wasm so every peer agrees.
fn rng(s: i32) -> i32 { s * 1103515245 + 12345 }

fn food_x(k: i32) -> i32 {
    let h: i32 = rng(12345 + k * 1000003);
    let m: i32 = h % 480;
    16 + ((m + 480) % 480)
}

fn food_y(k: i32) -> i32 {
    let h: i32 = rng(rng(98765 + k * 1000003));
    let m: i32 = h % 480;
    16 + ((m + 480) % 480)
}

// alive in {0,1}, len 0..255, head 0..255 — pure arithmetic packing (no bit ops).
fn pack_meta(a: i32, l: i32, hd: i32) -> i32 { a + l * 256 + hd * 65536 }

// A round-ish body dot (a filled square is round enough at this radius).
fn draw_disc(cx: i32, cy: i32, r: i32, rgb: i32) {
    host::display::fill_rect(cx - r, cy - r, 2 * r, 2 * r, rgb);
}

fn frame(t: i32) {
    let w: i32 = host::display::width();
    let h: i32 = host::display::height();
    host::display::clear(657426); // 0x0a0e12

    // 16-direction fixed-point velocity table (scale 256, speed 3px/frame):
    // vx[i]=round(768*cos(2pi i/16)), vy[i]=round(768*sin(...)); 0=right,4=down,8=left,12=up.
    let vx = [768, 709, 544, 294, 0, -294, -544, -709, -768, -709, -544, -294, 0, 294, 544, 709];
    let vy = [0, 294, 544, 709, 768, 709, 544, 294, 0, -294, -544, -709, -768, -709, -544, -294];
    let col = [3407752, 16763955, 3394815, 16733559, 13404415, 16777215, 8978244, 16746564];

    let down: i32 = host::display::pointer_down();
    let prevdown: i32 = host::display::state_get(43);
    let mut click: i32 = 0;
    if down == 1 {
        if prevdown == 0 { click = 1; }
    }

    let mode: i32 = host::display::state_get(40);
    let mut playing: i32 = 0;
    if mode == 1 { playing = 1; }
    if host::mp::connected() == 1 { playing = 1; }

    if playing == 0 {
        // ===================== LOBBY =====================
        let px: i32 = host::display::pointer_x();
        let py: i32 = host::display::pointer_y();
        if mode == 0 {
            host::display::draw_string(70, 110, "SLITHER", 3407752, 7);
            host::display::draw_string(120, 250, "TAP TOP TO HOST", 16777215, 2);
            host::display::draw_string(108, 300, "TAP BOTTOM TO JOIN", 16763955, 2);
            if click == 1 {
                if py < 256 {
                    host::display::state_set(42, host::mp::open());
                    host::display::state_set(40, 1);
                } else {
                    host::display::state_set(40, 2);
                    host::display::state_set(41, 0);
                }
            }
        } else {
            // mode 2: joiner keypad — enter the host's 4-digit code.
            host::display::draw_string(120, 40, "ENTER CODE", 16777215, 3);
            host::display::draw_number(170, 110, host::display::state_get(41), 3407752, 7);
            let bw: i32 = 51;
            let mut d: i32 = 0;
            while d < 10 {
                host::display::fill_rect(d * bw + 2, 380, bw - 4, 70, 2240576);
                host::display::draw_number(d * bw + 16, 402, d, 16777215, 3);
                d = d + 1;
            }
            host::display::fill_rect(0, 300, 512, 64, 2775098);
            host::display::draw_string(150, 320, "TAP TO JOIN", 10485952, 3);
            if click == 1 {
                if py >= 380 {
                    let dig: i32 = px / bw;
                    if dig >= 0 {
                        if dig < 10 {
                            let cur: i32 = host::display::state_get(41);
                            if cur < 1000 {
                                host::display::state_set(41, cur * 10 + dig);
                            }
                        }
                    }
                } else {
                    if py >= 300 {
                        if py < 364 {
                            host::mp::join(host::display::state_get(41));
                        }
                    }
                }
            }
            // tap the code readout to clear a mistype
            if click == 1 {
                if py < 200 {
                    host::display::state_set(41, 0);
                }
            }
        }
    } else {
        // ===================== GAME =====================
        let mut me: i32 = host::mp::self_index();
        if me < 0 { me = 0; }

        // (a) seed my snake on the first game frame (and on respawn)
        if host::display::state_get(5) == 0 {
            let sx: i32 = 96 + (me % 4) * 110;
            let sy: i32 = 150 + (me / 4) * 210;
            host::display::state_set(0, sx * 256);
            host::display::state_set(1, sy * 256);
            host::display::state_set(2, 0);
            host::display::state_set(3, sx);
            host::display::state_set(4, sy);
            host::display::state_set(6, 5);
            host::display::state_set(7, 0);
            host::display::state_set(8, 1);
            host::display::state_set(10, 45);
            let pk: i32 = sx * 512 + sy;
            let mut i: i32 = 0;
            while i < 30 {
                host::mp::set(1 + i, pk);
                i = i + 1;
            }
            host::mp::set(31, host::display::state_get(9));
            host::mp::set(0, pack_meta(1, 5, 0));
            host::display::state_set(5, 1);
        }

        let mut alive: i32 = host::display::state_get(8);
        let mut nx: i32 = host::display::state_get(0) / 256;
        let mut ny: i32 = host::display::state_get(1) / 256;

        if alive == 1 {
            // (b) steer toward the cursor: dot-product argmax over the 16 dirs.
            let cx: i32 = host::display::pointer_x();
            let cy: i32 = host::display::pointer_y();
            let dirc: i32 = host::display::state_get(2);
            let ddx: i32 = cx - nx;
            let ddy: i32 = cy - ny;
            let mut best: i32 = dirc;
            let mut bestdot: i32 = -2000000000;
            let mut i: i32 = 0;
            while i < 16 {
                let dot: i32 = ddx * vx[i] + ddy * vy[i];
                if dot > bestdot {
                    bestdot = dot;
                    best = i;
                }
                i = i + 1;
            }
            // turn at most one step toward `best`, the short way around the ring.
            let mut delta: i32 = best - dirc;
            if delta > 8 { delta = delta - 16; }
            if delta < -8 { delta = delta + 16; }
            let mut ndir: i32 = dirc;
            if delta > 0 { ndir = dirc + 1; }
            if delta < 0 { ndir = dirc - 1; }
            if ndir < 0 { ndir = 15; }
            if ndir > 15 { ndir = 0; }
            host::display::state_set(2, ndir);

            // (c) integrate head in fixed-point + toroidal wrap.
            let mut xfp: i32 = host::display::state_get(0) + vx[ndir];
            let mut yfp: i32 = host::display::state_get(1) + vy[ndir];
            if xfp < 0 { xfp = xfp + 131072; }
            if xfp >= 131072 { xfp = xfp - 131072; }
            if yfp < 0 { yfp = yfp + 131072; }
            if yfp >= 131072 { yfp = yfp - 131072; }
            host::display::state_set(0, xfp);
            host::display::state_set(1, yfp);
            nx = xfp / 256;
            ny = yfp / 256;

            // (d) waypoint ring: advance every ~16px; newest slot tracks the live head.
            let mut hp: i32 = host::display::state_get(7);
            let lwx: i32 = host::display::state_get(3);
            let lwy: i32 = host::display::state_get(4);
            let wdx: i32 = nx - lwx;
            let wdy: i32 = ny - lwy;
            if wdx * wdx + wdy * wdy >= 256 {
                hp = (hp + 1) % 30;
                host::display::state_set(7, hp);
                host::display::state_set(3, nx);
                host::display::state_set(4, ny);
            }
            host::mp::set(1 + hp, nx * 512 + ny);

            // (e) eat the single deterministic food (target k = sum of eat_counts).
            let mut ek: i32 = 0;
            let mut ep: i32 = 0;
            while ep < 8 {
                ek = ek + host::mp::get(ep, 31);
                ep = ep + 1;
            }
            let fx: i32 = food_x(ek);
            let fy: i32 = food_y(ek);
            let edx: i32 = nx - fx;
            let edy: i32 = ny - fy;
            if edx * edx + edy * edy < 256 {
                let ec: i32 = host::display::state_get(9) + 1;
                host::display::state_set(9, ec);
                host::mp::set(31, ec);
                let mut nl: i32 = host::display::state_get(6) + 1;
                if nl > 30 { nl = 30; }
                host::display::state_set(6, nl);
            }

            // (f) death: my head vs every OTHER live snake's body (skip while invuln).
            let mut iv: i32 = host::display::state_get(10);
            if iv > 0 {
                host::display::state_set(10, iv - 1);
            } else {
                let mut dq: i32 = 0;
                while dq < 8 {
                    if dq != me {
                        let qm: i32 = host::mp::get(dq, 0);
                        if (qm % 256) == 1 {
                            let ql: i32 = (qm / 256) % 256;
                            let qh: i32 = qm / 65536;
                            let mut ds: i32 = 0;
                            while ds < ql {
                                let dslot: i32 = 1 + ((qh - ds + 30) % 30);
                                let dwp: i32 = host::mp::get(dq, dslot);
                                let bx: i32 = dwp / 512;
                                let by: i32 = dwp % 512;
                                let cdx: i32 = nx - bx;
                                let cdy: i32 = ny - by;
                                if cdx * cdx + cdy * cdy < 64 {
                                    alive = 0;
                                    ds = ql;
                                    dq = 7;
                                }
                                ds = ds + 1;
                            }
                        }
                    }
                    dq = dq + 1;
                }
            }
            host::display::state_set(8, alive);

            // (g) publish my meta every frame (1 slot).
            host::mp::set(0, pack_meta(alive, host::display::state_get(6), host::display::state_get(7)));
        }

        // (h) render every live snake from network state (alive-gated; pre/post relay).
        let mut rq: i32 = 0;
        while rq < 8 {
            let rm: i32 = host::mp::get(rq, 0);
            if (rm % 256) == 1 {
                let rln: i32 = (rm / 256) % 256;
                let rhpr: i32 = rm / 65536;
                let rc: i32 = col[rq];
                let mut rs: i32 = 0;
                while rs < rln {
                    let rslot: i32 = 1 + ((rhpr - rs + 30) % 30);
                    let rwp: i32 = host::mp::get(rq, rslot);
                    draw_disc(rwp / 512, rwp % 512, 8, rc);
                    rs = rs + 1;
                }
                let rhwp: i32 = host::mp::get(rq, 1 + rhpr);
                draw_disc(rhwp / 512, rhwp % 512, 9, 16777215);
                rq = rq + 1;
            } else {
                rq = rq + 1;
            }
        }

        // (i) food.
        let mut gk: i32 = 0;
        let mut gp: i32 = 0;
        while gp < 8 {
            gk = gk + host::mp::get(gp, 31);
            gp = gp + 1;
        }
        draw_disc(food_x(gk), food_y(gk), 5, 16724804);

        // (j) HUD + death banner + tap-to-respawn (keeps score; resets length).
        host::display::draw_number(12, 12, host::display::state_get(9), 16777215, 4);
        if alive == 0 {
            host::display::draw_string(150, 220, "YOU DIED", 16724787, 5);
            host::display::draw_string(132, 280, "TAP TO RESPAWN", 8030086, 2);
            if click == 1 {
                host::display::state_set(5, 0);
            }
        }
    }

    host::display::state_set(43, down);
    host::display::present();
}
