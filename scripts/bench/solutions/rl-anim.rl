// rl-anim — a 32x32 square sweeping right, wrapped to the screen.
fn frame(t: i32) {
    host::display::clear(0x101010);
    let x: i32 = (t * 3) % 480;
    host::display::fill_rect(x, 240, 32, 32, 0xffffff);
    host::display::present();
}
