// p02_fill_rect_quadrants — reference solution.
//
// Splits the host-owned framebuffer into four equal quadrants, each a distinct
// solid colour. Dimensions are QUERIED from the host (width()/height()) rather
// than hard-coded, so the cartridge tracks a resized framebuffer.

fn frame(t: i32) {
    host::display::clear(0);

    let w: i32 = host::display::width();
    let h: i32 = host::display::height();
    let hw: i32 = w / 2;
    let hh: i32 = h / 2;

    host::display::fill_rect(0, 0, hw, hh, 16711680);        // top-left  red
    host::display::fill_rect(hw, 0, w - hw, hh, 65280);       // top-right green
    host::display::fill_rect(0, hh, hw, h - hh, 255);         // bottom-left blue
    host::display::fill_rect(hw, hh, w - hw, h - hh, 16777215); // bottom-right white

    host::display::present();
}
