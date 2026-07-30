// rl-pointer — a cursor square that grows while the pointer is down.
fn frame(t: i32) {
    host::display::clear(0);
    let x: i32 = host::display::pointer_x();
    let y: i32 = host::display::pointer_y();
    if host::display::pointer_down() > 0 {
        host::display::fill_rect(x, y, 16, 16, 0xff0000);
    } else {
        host::display::fill_rect(x, y, 8, 8, 0xffffff);
    }
    host::display::present();
}
