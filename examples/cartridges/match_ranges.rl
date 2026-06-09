// CORPUS: match with literal arms, an INCLUSIVE range arm (0..=5), an exclusive
// range arm (6..10), and a wildcard fallback.
//
// Exercises: the match lowering for IntRange patterns (both inclusive `lo..=hi`
// and exclusive `lo..hi`), literal patterns, and the `_` catch-all. `classify`
// maps an input bucket to a distinct colour; the frame runs it across several
// inputs and draws horizontal bands so the harness can pin each arm's pixel.
//
//   bucket(3)  -> 0..=5   -> 0x0000FF (blue)
//   bucket(7)  -> 6..10   -> 0x00FF00 (green)
//   bucket(42) -> _       -> 0xFF0000 (red)
//   bucket(100)-> literal -> 0xFFFFFF (white, the explicit `100 =>` arm)

fn classify(n: i32) -> i32 {
    match n {
        100 => 16777215,
        0..=5 => 255,
        6..10 => 65280,
        _ => 16711680,
    }
}

fn frame(t: i32) {
    host::display::clear(0);
    // Four bands, each filled with the colour its bucket maps to.
    host::display::fill_rect(0, 0, 256, 36, classify(3));   // blue band
    host::display::fill_rect(0, 36, 256, 36, classify(7));  // green band
    host::display::fill_rect(0, 72, 256, 36, classify(42)); // red band
    host::display::fill_rect(0, 108, 256, 36, classify(100)); // white band
    host::display::present();
}
