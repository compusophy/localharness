// rl-lib-math — a LIBRARY cartridge: callable math surface + a landing card.
fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn mul(a: i32, b: i32) -> i32 {
    a * b
}

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

// Never ticked when mounted via spawn_lib; the direct-visit landing card.
fn frame(t: i32) {
    host::display::clear(0);
    host::display::present();
}
