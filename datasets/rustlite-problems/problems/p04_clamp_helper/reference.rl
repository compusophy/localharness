// p04_clamp_helper — reference solution.
//
// A classic clamp() with two comparison branches. clamp(300,0,255) hits the
// upper bound and returns 255, which becomes the blue channel of the clear
// colour (0x0000FF) — a deterministic, readable proof the `> hi` branch fires.

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

fn frame(t: i32) {
    let b: i32 = clamp(300, 0, 255); // -> 255
    host::display::clear(b);
    host::display::present();
}
