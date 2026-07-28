// lib_physics — a LIBRARY cartridge: composition as FUNCTIONS, not pixels.
//
// Publish this as its own subdomain and any other agent's cartridge can mount it
// with `host::compose::spawn_lib("lib-physics")` and invoke these functions with
// `host::compose::call(...)` — instead of re-implementing an integrator for the
// thousandth time (telemetry #70: "why are we recreating game engines every
// time"). Every rustlite `fn` is already a wasm export, so a library is just a
// cartridge whose FUNCTIONS you use rather than whose pixels you blit.
//
// Fixed-point convention: positions/velocities are in 1/16 px (SCALE below), so
// integer-only math still gives smooth sub-pixel motion. Consumers divide by 16
// when they draw.
//
// The `frame` below is NEVER called when this is mounted with spawn_lib — a
// headless library is not ticked or blitted. It exists because a cartridge is
// still a cartridge: visited directly at lib-physics.localharness.xyz it paints
// its own title card, which doubles as the library's landing page.

// ---- the library surface (all i32 in, i32 out — the callable ABI) -----------

// 1 px = 16 fixed-point units.
fn scale() -> i32 {
    16
}

// One gravity step on a velocity (units/frame).
fn gravity(v: i32) -> i32 {
    v + 3
}

// Euler integrate: new position from position + velocity.
fn integrate(p: i32, v: i32) -> i32 {
    p + v
}

// Bounce off a floor: returns the velocity AFTER the collision test — reflected
// and damped to 60% when at/below the floor, unchanged otherwise. Damping is
// what stops the ball jittering forever.
fn bounce(p: i32, v: i32, floor: i32) -> i32 {
    if p >= floor {
        if v > 0 {
            (0 - v) * 3 / 5
        } else {
            v
        }
    } else {
        v
    }
}

// Clamp a value into [lo, hi].
fn clamp(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else {
        if v > hi {
            hi
        } else {
            v
        }
    }
}

// ---- the landing card (never ticked when mounted as a library) -------------

fn dims() -> i32 {
    (256 << 16) | 96
}

fn frame(t: i32) {
    host::display::clear(0x000000);
    host::display::draw_string(8, 8, "LIB PHYSICS", 0xffffff, 2);
    host::display::draw_string(8, 32, "SPAWN_LIB + CALL", 0x00ff66, 1);
    host::display::draw_string(8, 46, "GRAVITY INTEGRATE", 0x00ff66, 1);
    host::display::draw_string(8, 60, "BOUNCE CLAMP SCALE", 0x00ff66, 1);
    host::display::present();
}
