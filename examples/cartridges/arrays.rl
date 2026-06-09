// CORPUS: array literals + indexed READS (the lookup-table pattern).
//
// Exercises: an array literal stored in linear memory, a constant-index read,
// and a VARIABLE-index read (the index computed at runtime). Arrays are
// READ-ONLY in current rustlite — this cartridge never writes an element, so it
// stays valid while the sibling adds write support.
//
// A 4-colour palette is indexed by `t % 4`, and a second array of bar widths is
// summed via a loop reading `widths[i]`. The harness pins:
//   - palette[0] at t=0 -> clear colour 0xFF0000 (red)
//   - the four bars are drawn in their palette colours (non-blank)
//   - the summed width (10+20+30+40 = 100) lands as a marker pixel

fn frame(t: i32) {
    let palette = [16711680, 65280, 255, 16776960]; // red, green, blue, yellow
    let widths = [10, 20, 30, 40];

    // Background = the palette colour selected by the frame counter.
    host::display::clear(palette[t % 4]);

    // Lay the four bars left-to-right, each its palette colour, widths from the
    // array — a constant-index read per iteration via the loop variable.
    let mut x: i32 = 0;
    let mut i: i32 = 0;
    let mut total: i32 = 0;
    while i < 4 {
        let w: i32 = widths[i];
        host::display::fill_rect(x, 40, w, 40, palette[i]);
        x = x + w;
        total = total + w;
        i = i + 1;
    }

    // Marker rect whose height encodes the summed width / 5 = 20px tall,
    // proving the variable-index reads accumulated correctly (total = 100).
    host::display::fill_rect(0, 100, 8, total / 5, 16777215);
    host::display::present();
}
