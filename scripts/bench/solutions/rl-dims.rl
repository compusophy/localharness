// rl-dims — custom 256x128 canvas via the packed dims() export.
fn dims() -> i32 {
    (256 << 16) | 128
}

fn frame(t: i32) {
    host::display::clear(0x202020);
    host::display::present();
}
