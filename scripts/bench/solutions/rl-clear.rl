// rl-clear — minimal frame: clear + present.
fn frame(t: i32) {
    host::display::clear(0);
    host::display::present();
}
