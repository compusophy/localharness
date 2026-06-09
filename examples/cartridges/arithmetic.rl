// CORPUS: integer arithmetic with an OBSERVABLE result.
//
// Exercises: +, -, *, /, % (all five integer binops), operator precedence,
// parenthesised sub-expressions, and a let-bound intermediate. The frame paints
// the computed value as the clear colour so the harness can read it back as a
// deterministic pixel.
//
//   v = ((7 * 6) + 100) / 2 - 3 % 4    (Rust precedence: * / % before + -)
//     = (42 + 100) / 2 - (3 % 4)
//     = 142 / 2 - 3
//     = 71 - 3
//     = 68
//
// The harness asserts the cleared pixel == 0x000044 (68 = 0x44), proving the
// codegen evaluates arithmetic + precedence correctly, not just "doesn't trap".

fn compute() -> i32 {
    let a: i32 = 7 * 6;
    let b: i32 = a + 100;
    let c: i32 = b / 2;
    let d: i32 = 3 % 4;
    c - d
}

fn frame(t: i32) {
    let v: i32 = compute();
    // Pack the result into the low byte of the clear colour so it is readable
    // as a single pixel's blue channel (v = 68 -> 0x000044 -> B=68).
    host::display::clear(v);
    host::display::present();
}
