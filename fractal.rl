// Fractal — the canonical cartridge-in-cartridge demo, now that composition is
// RECURSIVE. Every instance draws an animated, bordered frame and then spawns
// ITSELF (the same published app.wasm) into a centered sub-rectangle. The
// compositor runs that child in its own buffer and blits it back in, so you see
// a screen-within-a-screen-within-a-… (a Droste image). The recursion is real:
// the child is a full instance with its own compose table that spawns its own
// child, and so on. It terminates at the compositor's depth cap, where
// spawn_module returns -1 and no deeper level mounts — the fractal is finite,
// bounded by COMPOSE_MAX_DEPTH / the global node + byte caps.

fn dims() -> i32 {
    // 256 x 144, 16:9.
    (256 * 65536) + 144
}

fn frame(t: i32) {
    host::display::clear(0x000000);

    // A bright border so each nested level reads as its own live frame (not a
    // frozen screenshot of itself).
    host::display::fill_rect(0, 0, 256, 4, 0x00ff66);
    host::display::fill_rect(0, 140, 256, 4, 0x00ff66);
    host::display::fill_rect(0, 0, 4, 144, 0x00ff66);
    host::display::fill_rect(252, 0, 4, 144, 0x00ff66);

    // A dot sweeping left→right so motion is visible at every depth. All levels
    // share the same clock t, so the nested dots move in lockstep — the tell
    // that each inner screen is a genuinely running instance.
    let x: i32 = (t / 12) % 240;
    host::display::fill_rect(8 + x, 66, 10, 10, 0xffffff);

    // Spawn myself ONCE (state slot 0 guards against re-spawning every frame),
    // centered, at ~62% size. Each deeper level repeats this, nesting toward the
    // middle, until the compositor's depth cap refuses (returns -1) and stops.
    let spawned: i32 = host::display::state_get(0);
    if spawned == 0 {
        host::display::state_set(0, 1);
        let h: i32 = host::compose::spawn_module("fractal", 48, 27, 160, 90);
        host::compose::focus_module(h);
    }

    host::display::present();
}
