// CORPUS: the known-good animated TIMER — a frame(t) that visibly changes.
//
// Exercises: a real animation driven by the frame counter `t` (clear +
// fill_rect + draw_line + draw_char), arithmetic on `t` for a moving element,
// and a sweep that depends on `t` so DIFFERENT `t` values produce DIFFERENT
// framebuffers. The harness renders at several `t` and asserts:
//   - the framebuffer is non-blank at every t
//   - the framebuffer at t=0 DIFFERS from t=30 (it actually animates)

fn frame(t: i32) {
    host::display::clear(1052688); // dark grey 0x101010

    // A progress bar whose width sweeps with t (mod the screen width).
    let w: i32 = (t * 4) % 256;
    host::display::fill_rect(0, 60, w, 24, 65280); // green bar

    // A horizontal rule under the bar.
    host::display::draw_line(0, 90, 255, 90, 8421504);

    // A marker square that walks across the screen with t.
    let mx: i32 = (t * 3) % 248;
    host::display::fill_rect(mx, 20, 8, 8, 16776960); // yellow box

    // A label: "T" then the current tick as a number.
    host::display::draw_char(8, 8, 84, 16777215, 2);       // 'T'
    host::display::draw_number(24, 8, t, 16777215, 2);

    host::display::present();
}
