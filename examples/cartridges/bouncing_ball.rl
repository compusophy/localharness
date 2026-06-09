// CORPUS: a bouncing ball — a small realistic app driven by frame(t).
//
// Exercises: triangle-wave position math (so the ball reverses at the walls),
// integer division/modulo to fold `t` into a back-and-forth path, two
// independent axes, and a drawn ball (fill_rect) over a cleared field with a
// border. The position is a PURE function of `t` (no state needed) so it is
// fully deterministic and the harness can pin the ball's location.
//
// triangle(t, span): folds t into 0..span and back, period 2*span.
//   phase = t % (2*span); if phase < span -> phase else -> 2*span - phase
//
// The harness asserts: non-blank every frame, the field animates between t
// values, and at t=0 the ball sits at the start corner (a known pixel is lit).

fn triangle(t: i32, span: i32) -> i32 {
    let period: i32 = span * 2;
    let phase: i32 = t % period;
    // `phase < span { … }` parses fine: the if-condition position forbids a
    // bare struct literal, so `span` is the variable and `{` opens the block.
    if phase < span {
        phase
    } else {
        period - phase
    }
}

fn frame(t: i32) {
    host::display::clear(0);

    // Border frame so the playfield is visible even on a still frame.
    host::display::fill_rect(0, 0, 256, 2, 4210752);
    host::display::fill_rect(0, 142, 256, 2, 4210752);
    host::display::fill_rect(0, 0, 2, 144, 4210752);
    host::display::fill_rect(254, 0, 2, 144, 4210752);

    // Ball position bounces within the inner field (8px radius margins).
    let bx: i32 = 8 + triangle(t * 2, 232);
    let by: i32 = 8 + triangle(t, 120);

    // The ball: a 12x12 white square (a circle isn't needed to prove motion).
    host::display::fill_rect(bx, by, 12, 12, 16777215);

    host::display::present();
}
