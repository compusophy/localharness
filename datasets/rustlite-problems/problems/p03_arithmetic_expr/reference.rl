// p03_arithmetic_expr — reference solution.
//
// Evaluates ((7 * 6) + 100) / 2 - 3 % 4 with Rust precedence:
//   (42 + 100) / 2 - (3 % 4) = 142 / 2 - 3 = 71 - 3 = 68
// then paints 68 as the clear colour (0x000044) so the result is readable as
// the blue channel of any pixel. Proves codegen honours operator precedence.

fn compute() -> i32 {
    ((7 * 6) + 100) / 2 - 3 % 4
}

fn frame(t: i32) {
    let v: i32 = compute();
    host::display::clear(v);
    host::display::present();
}
