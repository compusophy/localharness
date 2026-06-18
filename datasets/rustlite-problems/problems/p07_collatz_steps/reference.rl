// p07_collatz_steps — reference solution.
//
// Counts the Collatz stopping time: from `start`, halve if even else 3n+1,
// counting steps to reach 1. collatz_steps(27) == 111 (a famously long orbit).
// 111 == 0x6F, painted into the GREEN channel (0x006F00) so it reads back as a
// deterministic pixel, and also drawn as a number. Exercises a data-dependent
// while loop with an even/odd branch via the modulo operator.

fn collatz_steps(start: i32) -> i32 {
    let mut n: i32 = start;
    let mut steps: i32 = 0;
    while n != 1 {
        if n % 2 == 0 {
            n = n / 2;
        } else {
            n = 3 * n + 1;
        }
        steps = steps + 1;
    }
    steps
}

fn frame(t: i32) {
    let s: i32 = collatz_steps(27); // 111
    host::display::clear(s * 256);  // 0x006F00 -> green channel 111
    host::display::draw_number(8, 8, s, 16777215, 2);
    host::display::present();
}
