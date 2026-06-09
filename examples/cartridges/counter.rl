// CORPUS: a stateful COUNTER — state_get / state_set persisting across frames.
//
// Exercises: the host state slots (read slot 0, increment, write it back) so a
// value PERSISTS between frame() calls — the cartridge has no memory of `t`
// itself driving the count, it accumulates its own. Also draws the count as a
// number and a bar whose width tracks it.
//
// The harness drives several frames in sequence with a SHARED state map (the
// real host model) and asserts the stored slot increments per frame:
//   after frame() called N times, state[0] == N
// and that the drawn bar grows (different framebuffer as the count rises).

fn frame(t: i32) {
    // Read the persisted count, bump it, store it back.
    let count: i32 = host::display::state_get(0) + 1;
    host::display::state_set(0, count);

    host::display::clear(1052688); // 0x101010

    // "N=" label then the live count.
    host::display::draw_char(8, 10, 78, 16777215, 2);  // 'N'
    host::display::draw_char(20, 10, 61, 16777215, 2); // '='
    host::display::draw_number(34, 10, count, 65280, 2);

    // A bar whose width tracks the count (clamped to the screen).
    let w: i32 = count * 4;
    let cw: i32 = if w > 256 { 256 } else { w };
    host::display::fill_rect(0, 60, cw, 30, 65280);

    host::display::present();
}
