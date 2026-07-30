// rl-counter — persistent frame count via host state slot 0.
fn frame(t: i32) {
    let count: i32 = host::display::state_get(0) + 1;
    host::display::state_set(0, count);
    host::display::clear(0x101010);
    host::display::draw_number(10, 10, count, 0x00ff00, 2);
    host::display::present();
}
