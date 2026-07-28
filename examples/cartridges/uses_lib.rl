// uses_lib — the CONSUMER half of the callable-library layer (telemetry #70).
//
// It owns no physics. It mounts `lib-physics` HEADLESS and calls its exports for
// every step of the simulation. Nothing is blitted from the library — this
// cartridge draws all its own pixels; the library only computes.
//
// The pattern, in order:
//   1. spawn_lib(name) once (latched in a state slot) -> handle
//   2. poll status(handle) == 1 (the mount is async: bytes come off-chain)
//   3. call(handle, "fn", a0..a3) and CHECK call_ok() == 0 before trusting the
//      result — the call returns the export's own i32, so 0 is ambiguous.
//
// State slots: 0 = spawned latch, 1 = handle, 2 = y (fixed-point), 3 = v.

fn dims() -> i32 {
    (256 << 16) | 144
}

fn frame(t: i32) {
    host::display::clear(0x000000);

    // 1. Mount the library ONCE. No rect, no framebuffer — spawn_lib takes only
    //    a name because a library has nothing to draw into.
    if host::display::state_get(0) == 0 {
        host::display::state_set(0, 1);
        host::display::state_set(1, host::compose::spawn_lib("lib-physics"));
        host::display::state_set(2, 8 * 16); // start at y=8px, in 1/16 units
        host::display::state_set(3, 0);
    }

    let h: i32 = host::display::state_get(1);
    let st: i32 = host::compose::status(h);

    // 2. The mount is asynchronous — show the state instead of silently drawing
    //    a frozen ball while the library is still loading.
    if st == 0 {
        host::display::draw_string(8, 8, "LOADING LIB", 0xffff00, 1);
    }
    if st == 2 {
        host::display::draw_string(8, 8, "LIB FAILED", 0xff0000, 1);
    }

    if st == 1 {
        // 3. Call the library. Each call returns the export's own i32; call_ok()
        //    is what says whether it ran at all.
        let v0: i32 = host::display::state_get(3);
        let y0: i32 = host::display::state_get(2);

        let v1: i32 = host::compose::call(h, "gravity", v0, 0, 0, 0);
        let ok: i32 = host::compose::call_ok();

        if ok == 0 {
            let y1: i32 = host::compose::call(h, "integrate", y0, v1, 0, 0);
            // Floor at y=136px in fixed-point units.
            let v2: i32 = host::compose::call(h, "bounce", y1, v1, 136 * 16, 0);
            // `call` always takes 4 int args; the host forwards only as many as
            // the export declares (clamp takes 3), so pad the tail with 0.
            let y2: i32 = host::compose::call(h, "clamp", y1, 0, 136 * 16, 0);
            host::display::state_set(2, y2);
            host::display::state_set(3, v2);

            // Draw the ball the library moved (fixed-point -> pixels).
            let py: i32 = y2 / 16;
            host::display::fill_rect(120, py, 16, 8, 0x00ff66);
            host::display::draw_string(8, 8, "LIB PHYSICS OK", 0x00ff66, 1);
            host::display::draw_number(8, 22, py, 0xffffff, 1);
        } else {
            // A failed call is VISIBLE, never a silent zero.
            host::display::draw_string(8, 8, "CALL FAILED", 0xff0000, 1);
            host::display::draw_number(8, 22, ok, 0xff0000, 1);
        }
    }

    host::display::fill_rect(0, 140, 256, 4, 0x333333); // the floor
    host::display::present();
}
