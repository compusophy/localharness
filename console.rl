// console.rl — a CONSOLE shell (a Game Boy) that LOADS A CARTRIDGE. The chrome
// (body, dot-matrix screen bezel, "GAME BOY" wordmark, D-pad, A/B buttons,
// START/SELECT, speaker) is drawn by THIS cartridge; the game on the screen is a
// DIFFERENT subdomain's published app.wasm, mounted with host::compose into the
// screen recess. By default it loads `mario` — proving embedded cartridges +
// cross-subdomain modularity: console.localharness.xyz runs mario.localharness.xyz
// inside its own framebuffer, no iframes, two isolated wasm instances, one canvas.
//
// The screen recess is exactly 160x144 (real Game Boy resolution) at offset
// (12,28), so a 160x144 cartridge like `mario` composites at identity scale.

fn dims() -> i32 {
    // A tall, Game-Boy-shaped surface: 184 wide x 300 tall.
    (184 * 65536) + 300
}

// A filled "disc" (octagon — square with its 4 corners trimmed to `bg`), used for
// the round A/B buttons. `bg` is whatever shows through the trimmed corners.
fn fill_disc(cx: i32, cy: i32, r: i32, rgb: i32, bg: i32) {
    host::display::fill_rect(cx - r, cy - r, 2 * r, 2 * r, rgb);
    let t: i32 = r / 2;
    host::display::fill_triangle(cx - r, cy - r, cx - r + t, cy - r, cx - r, cy - r + t, bg);
    host::display::fill_triangle(cx + r, cy - r, cx + r - t, cy - r, cx + r, cy - r + t, bg);
    host::display::fill_triangle(cx - r, cy + r, cx - r + t, cy + r, cx - r, cy + r - t, bg);
    host::display::fill_triangle(cx + r, cy + r, cx + r - t, cy + r, cx + r, cy + r - t, bg);
}

// A horizontal pill (rounded short ends), used for START / SELECT.
fn pill(cx: i32, cy: i32, hw: i32, hh: i32, rgb: i32, bg: i32) {
    host::display::fill_rect(cx - hw, cy - hh, 2 * hw, 2 * hh, rgb);
    host::display::fill_triangle(cx - hw, cy - hh, cx - hw + hh, cy - hh, cx - hw, cy - hh + hh, bg);
    host::display::fill_triangle(cx + hw, cy - hh, cx + hw - hh, cy - hh, cx + hw, cy - hh + hh, bg);
    host::display::fill_triangle(cx - hw, cy + hh, cx - hw + hh, cy + hh, cx - hw, cy + hh - hh, bg);
    host::display::fill_triangle(cx + hw, cy + hh, cx + hw - hh, cy + hh, cx + hw, cy + hh - hh, bg);
}

fn frame(t: i32) {
    // --- the cartridge LIBRARY: mount BOTH games once, show one at a time ------
    // The console keeps two cartridges live (mario in slot 1, tetris in slot 2)
    // and parks the off-screen one far outside the framebuffer so it isn't
    // blitted — but it KEEPS TICKING, so when it cycles back in it has carried on
    // (tetris is mid-game). Swapping which one fills the screen every ~7s proves
    // the console can host several different subdomains' cartridges, hot.
    let spawned: i32 = host::display::state_get(0);
    if spawned == 0 {
        host::display::state_set(0, 1);
        // NB: distinct names from hm/ht below — rustlite miscompiles a `let` that
        // shadows an outer-scope `let` of the same name (the handle aliased the
        // wrong local), so the spawn temporaries are h0/h1, not hm/ht.
        let h0: i32 = host::compose::spawn_module("mario", 12, 28, 160, 144);
        let h1: i32 = host::compose::spawn_module("tetris", 400, 400, 160, 144);
        host::display::state_set(1, h0);
        host::display::state_set(2, h1);
    }
    let hm: i32 = host::display::state_get(1);
    let ht: i32 = host::display::state_get(2);
    let show: i32 = (t / 7000) % 2; // 0 = mario, 1 = tetris (t is milliseconds)
    if show == 0 {
        host::compose::move_module(hm, 12, 28, 160, 144);
        host::compose::move_module(ht, 400, 400, 160, 144);
        host::compose::focus_module(hm);
    } else {
        host::compose::move_module(ht, 12, 28, 160, 144);
        host::compose::move_module(hm, 400, 400, 160, 144);
        host::compose::focus_module(ht);
    }

    // --- body (the grey plastic shell) -----------------------------------------
    host::display::clear(0xc8c5bc);
    // bevel: light top/left, dark bottom/right (a hint of moulded plastic)
    host::display::fill_rect(0, 0, 184, 2, 0xe4e1d8);
    host::display::fill_rect(0, 0, 2, 300, 0xe4e1d8);
    host::display::fill_rect(0, 298, 184, 2, 0x8c897f);
    host::display::fill_rect(182, 0, 2, 300, 0x8c897f);

    // --- power switch + LED (top band) -----------------------------------------
    host::display::fill_rect(64, 6, 56, 8, 0xb4b1a8);
    host::display::fill_rect(66, 6, 14, 8, 0x4a4a4a);
    fill_disc(16, 11, 4, 0xe02020, 0xc8c5bc);

    // --- screen bezel (dark dot-matrix surround) -------------------------------
    host::display::fill_rect(8, 22, 168, 170, 0x383838);
    // recessed inset around the LCD
    host::display::fill_rect(10, 26, 164, 148, 0x101010);
    // the LCD itself — Game Boy green; the mario cartridge composites OVER this.
    host::display::fill_rect(12, 28, 160, 144, 0x9bbc0f);

    // "DOT MATRIX WITH STEREO SOUND" silkscreen, centered below the LCD.
    let msg = [68, 79, 84, 32, 77, 65, 84, 82, 73, 88, 32, 87, 73, 84, 72, 32, 83, 84, 69, 82, 69, 79, 32, 83, 79, 85, 78, 68];
    let mut mi: i32 = 0;
    while mi < 28 {
        let mc: i32 = msg[mi];
        if mc != 32 {
            host::display::draw_char(8 + mi * 6, 174, mc, 0x8c8c8c, 1);
        }
        mi = mi + 1;
    }
    // the two iconic accent stripes (indigo over maroon), lower-left of the bezel
    host::display::draw_line(14, 188, 64, 184, 0x101878);
    host::display::draw_line(14, 190, 64, 186, 0x101878);
    host::display::draw_line(14, 191, 64, 187, 0x901038);
    host::display::draw_line(14, 193, 64, 189, 0x901038);

    // --- wordmark: "Nintendo" over a big "GAME BOY" -----------------------------
    let nin = [78, 105, 110, 116, 101, 110, 100, 111];
    let mut ni: i32 = 0;
    while ni < 8 {
        host::display::draw_char(68 + ni * 6, 196, nin[ni], 0x2a2a3a, 1);
        ni = ni + 1;
    }
    let gb = [71, 65, 77, 69, 32, 66, 79, 89];
    let mut gi: i32 = 0;
    while gi < 8 {
        let gc: i32 = gb[gi];
        if gc != 32 {
            host::display::draw_char(44 + gi * 12, 208, gc, 0x2a2a3a, 2);
        }
        gi = gi + 1;
    }

    // --- D-pad (left) -----------------------------------------------------------
    host::display::fill_rect(40 - 7, 256 - 20, 14, 40, 0x2a2a2a);
    host::display::fill_rect(40 - 20, 256 - 7, 40, 14, 0x2a2a2a);
    host::display::fill_rect(40 - 5, 256 - 5, 10, 10, 0x1a1a1a);

    // --- A / B buttons (right, diagonal) ---------------------------------------
    fill_disc(150, 250, 12, 0xa8a59c, 0xc8c5bc);
    fill_disc(150, 250, 11, 0x9b1030, 0xc8c5bc);
    fill_disc(122, 264, 12, 0xa8a59c, 0xc8c5bc);
    fill_disc(122, 264, 11, 0x9b1030, 0xc8c5bc);
    host::display::draw_char(164, 252, 65, 0x2a2a3a, 1); // 'A'
    host::display::draw_char(136, 266, 66, 0x2a2a3a, 1); // 'B'

    // --- speaker grille (angled grid of holes, bottom-right) -------------------
    let mut sr: i32 = 0;
    while sr < 4 {
        let mut sc: i32 = 0;
        while sc < 5 {
            host::display::fill_rect(132 + sc * 6 + sr * 2, 268 + sr * 4, 2, 2, 0x9a978e);
            sc = sc + 1;
        }
        sr = sr + 1;
    }

    // --- START / SELECT (center) -----------------------------------------------
    pill(74, 286, 13, 4, 0x5a5a5a, 0xc8c5bc);
    pill(108, 286, 13, 4, 0x5a5a5a, 0xc8c5bc);
    let sel = [83, 69, 76, 69, 67, 84]; // "SELECT"
    let mut ci: i32 = 0;
    while ci < 6 {
        host::display::draw_char(56 + ci * 6, 292, sel[ci], 0x2a2a3a, 1);
        ci = ci + 1;
    }
    let sta = [83, 84, 65, 82, 84]; // "START"
    let mut ti: i32 = 0;
    while ti < 5 {
        host::display::draw_char(95 + ti * 6, 292, sta[ti], 0x2a2a3a, 1);
        ti = ti + 1;
    }

    // --- rounded shell corners (carve to the black letterbox) ------------------
    host::display::fill_triangle(0, 0, 8, 0, 0, 8, 0x000000);
    host::display::fill_triangle(184, 0, 176, 0, 184, 8, 0x000000);
    host::display::fill_triangle(0, 300, 8, 300, 0, 292, 0x000000);
    // the big asymmetric bottom-right curve (the Game Boy silhouette)
    host::display::fill_triangle(184, 300, 144, 300, 184, 260, 0x000000);

    host::display::present();
}
