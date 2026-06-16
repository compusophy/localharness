// mario.rl — a tiny side-scrolling MARIO scene, authored to run as a CARTRIDGE
// loaded INTO the `console` Game Boy (host::compose). It renders at the real
// Game Boy native resolution (160 x 144), so the `console` parent composes it
// into its screen recess at identity scale (no blur). It is ALSO a complete
// standalone face at mario.localharness.xyz — byte-for-byte the same app.wasm.
//
// What it draws each frame(t): a blue sky with drifting clouds, green bushes, a
// row of "?" blocks, a brick ground, and Mario WALKING left→right (two-frame
// leg cycle) on a loop. Pure function of t — deterministic, no input needed.

fn dims() -> i32 {
    // (width << 16) | height — the Game Boy screen is 160 x 144.
    (160 * 65536) + 144
}

// A fluffy white cloud: three overlapping blocks so it reads rounded.
fn cloud(x: i32, y: i32) {
    host::display::fill_rect(x + 6, y, 14, 8, 0xffffff);
    host::display::fill_rect(x, y + 6, 26, 8, 0xffffff);
    host::display::fill_rect(x + 4, y + 4, 6, 4, 0xffffff);
}

// A green bush on the ground line (three bumps).
fn bush(x: i32, y: i32) {
    host::display::fill_rect(x + 4, y - 6, 8, 6, 0x00a800);
    host::display::fill_rect(x, y - 3, 24, 4, 0x00a800);
    host::display::fill_rect(x + 14, y - 7, 8, 7, 0x00a800);
}

// A floating "?" question block: a yellow tile with a dark border, rivets, and
// a "?" glyph (font code 63).
fn qblock(x: i32, y: i32) {
    host::display::fill_rect(x, y, 16, 16, 0x000000);
    host::display::fill_rect(x + 1, y + 1, 14, 14, 0xf8b800);
    host::display::fill_rect(x + 2, y + 2, 12, 12, 0xfca044);
    host::display::fill_rect(x + 3, y + 3, 10, 10, 0xf8b800);
    host::display::draw_char(x + 4, y + 4, 63, 0x000000, 1);
}

// Mario, drawn as a chunky sprite at (x, y) with pixel unit p = 2 (so ~26x32
// device px). `step` (0 or 1) flips the leg pose for the walk cycle.
fn mario(x: i32, y: i32, step: i32) {
    // cap
    host::display::fill_rect(x + 8, y, 12, 4, 0xd82800);
    host::display::fill_rect(x + 8, y + 4, 16, 2, 0xd82800);
    // back hair / sideburn
    host::display::fill_rect(x + 4, y + 4, 4, 2, 0x5c2e0e);
    host::display::fill_rect(x + 4, y + 6, 4, 6, 0x5c2e0e);
    // face
    host::display::fill_rect(x + 8, y + 6, 12, 8, 0xfca868);
    // eye
    host::display::fill_rect(x + 14, y + 6, 2, 4, 0x101010);
    // big mustache
    host::display::fill_rect(x + 10, y + 10, 10, 4, 0x5c2e0e);
    // shirt / torso (red)
    host::display::fill_rect(x + 6, y + 14, 16, 6, 0xd82800);
    // arms + skin hands
    host::display::fill_rect(x + 2, y + 14, 4, 6, 0xd82800);
    host::display::fill_rect(x + 2, y + 20, 4, 2, 0xfca868);
    host::display::fill_rect(x + 22, y + 14, 4, 6, 0xd82800);
    host::display::fill_rect(x + 22, y + 20, 4, 2, 0xfca868);
    // overalls (blue) with straps + buttons
    host::display::fill_rect(x + 8, y + 18, 12, 8, 0x2038ec);
    host::display::fill_rect(x + 10, y + 14, 2, 6, 0x2038ec);
    host::display::fill_rect(x + 16, y + 14, 2, 6, 0x2038ec);
    host::display::fill_rect(x + 10, y + 20, 2, 2, 0xf8b800);
    host::display::fill_rect(x + 16, y + 20, 2, 2, 0xf8b800);
    // legs + shoes (brown), animated by step
    if step == 0 {
        host::display::fill_rect(x + 6, y + 26, 6, 6, 0x5c2e0e);
        host::display::fill_rect(x + 16, y + 26, 6, 4, 0x5c2e0e);
    } else {
        host::display::fill_rect(x + 6, y + 26, 6, 4, 0x5c2e0e);
        host::display::fill_rect(x + 16, y + 26, 6, 6, 0x5c2e0e);
    }
}

// The brick ground band across the bottom, with mortar lines.
fn ground(top: i32) {
    host::display::fill_rect(0, top, 160, 144 - top, 0xc06030);
    host::display::fill_rect(0, top, 160, 2, 0xe0a070);
    // vertical mortar lines (offset by row for a brick-laid look)
    let mut bx: i32 = 0;
    while bx < 160 {
        host::display::fill_rect(bx, top + 2, 1, 10, 0x803010);
        host::display::fill_rect(bx + 8, top + 12, 1, 12, 0x803010);
        bx = bx + 16;
    }
    // horizontal mortar line between the two brick courses
    host::display::fill_rect(0, top + 12, 160, 1, 0x803010);
}

// Triangle wave: folds `t` into 0..span and back (period 2*span). Used for the
// hop arc so the jump rises and falls smoothly.
fn triangle(t: i32, span: i32) -> i32 {
    let period: i32 = span * 2;
    let phase: i32 = t % period;
    if phase < span {
        phase
    } else {
        period - phase
    }
}

fn frame(t: i32) {
    // NOTE: t is MILLISECONDS (the host clock), so every motion is scaled by a
    // big divisor — a walk, not a teleport.

    // sky
    host::display::clear(0x5c94fc);

    // clouds drift slowly leftward (parallax), wrapping across the sky
    let c0: i32 = 150 - (t / 60) % 230;
    cloud(c0, 16);
    let c1: i32 = 120 - (t / 90) % 250;
    cloud(c1, 40);

    let gtop: i32 = 120;

    // bushes near the ground line
    bush(20, gtop);
    bush(108, gtop);

    // a little row of "?" blocks
    qblock(48, 60);
    qblock(72, 60);
    qblock(96, 60);

    // ground
    ground(gtop);

    // Mario strolls left→right (~25 px/sec) and wraps around.
    let span: i32 = 210;
    let mx: i32 = (t / 40) % span - 30;
    // legs alternate ~2.3x/sec
    let step: i32 = (t / 220) % 2;
    // a gentle hop every ~2.6 seconds (peaks ~16px), else a 1px walk bob.
    let jt: i32 = t % 2600;
    let mut hop: i32 = step;
    if jt < 520 {
        hop = triangle(jt, 260) * 16 / 260;
    }
    let my: i32 = gtop - 32 - hop;
    mario(mx, my, step);

    host::display::present();
}
