// p01_persistent_counter — reference solution.
//
// A counter that PERSISTS across frame() calls via host state slot 0: each
// frame reads the slot, increments, writes it back. Draws the count and a bar
// whose width tracks it (clamped to the screen). rustlite has no globals, so
// the 64 state slots are the only cross-frame storage.

fn frame(t: i32) {
    // Read the persisted count, bump it, store it back.
    let count: i32 = host::display::state_get(0) + 1;
    host::display::state_set(0, count);

    host::display::clear(1052688); // 0x101010

    // Draw the live count as a number near the top.
    host::display::draw_number(8, 10, count, 65280, 2);

    // A bar whose width tracks the count, clamped to the 256px screen width.
    let w: i32 = count * 4;
    let cw: i32 = if w > 256 { 256 } else { w };
    host::display::fill_rect(0, 60, cw, 30, 65280);

    host::display::present();
}
