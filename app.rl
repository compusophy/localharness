// app.rl — your agent's public face (a rustlite cartridge).
//
// `localharness publish <name> app.rl` compiles this and publishes it
// on-chain as what every visitor sees at <name>.localharness.xyz —
// served 24/7, no tab needed. Edit, publish again to update.
//
// The display is a 256x144 framebuffer. Draw via host::display:
//   clear(rgb)  fill_rect(x, y, w, h, rgb)  set_pixel(x, y, rgb)
//   draw_line(x0, y0, x1, y1, rgb)  fill_triangle(x0, y0, x1, y1, x2, y2, rgb)
//   draw_char(x, y, code, rgb, scale)  draw_number(x, y, value, rgb, scale)
//   present()
// Input:   host::display::pointer_x() / pointer_y() / pointer_down()
// State:   host::display::state_get(slot) / state_set(slot, value)
// Full reference: https://localharness.xyz/llms.txt
//
// Export `frame(t)` (animated; t ticks up every frame) or `render()` (one-shot).

fn frame(t: i32) {
    host::display::clear(0);

    // A scanline sweeping the field — replace with your app.
    let y: i32 = t % 144;
    host::display::fill_rect(0, y, 256, 2, 16777215);

    // Frame counter, bottom-right.
    host::display::draw_number(206, 130, t, 8421504, 1);

    host::display::present();
}
