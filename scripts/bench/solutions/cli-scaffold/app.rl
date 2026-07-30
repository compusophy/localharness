// cli-scaffold — the scaffolded starter cartridge.
fn frame(t: i32) {
    host::display::clear(0x101010);
    host::display::fill_rect(96, 96, 64, 64, 0xffffff);
    host::display::present();
}
