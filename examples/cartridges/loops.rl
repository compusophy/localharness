// CORPUS: while AND for loops, both producing an observable sum.
//
// Exercises: a `while` loop with a mutable accumulator + counter, and a `for`
// loop over an EXCLUSIVE range `0..n` (the only range form rustlite's `for`
// supports — desugars to a loop with a top-of-body increment). Both compute the
// same triangular number and the frame asserts they agree by drawing it.
//
//   while: sum 1..=10 = 55
//   for:   sum 0..10 of (i+1) = 55
//
// The harness reads the clear colour back: 55 = 0x37 -> 0x000037, proving both
// loop lowerings iterate the right number of times and the accumulator carries.

fn sum_while(n: i32) -> i32 {
    let mut acc: i32 = 0;
    let mut i: i32 = 1;
    while i <= n {
        acc = acc + i;
        i = i + 1;
    }
    acc
}

fn sum_for(n: i32) -> i32 {
    let mut acc: i32 = 0;
    for i in 0..n {
        acc = acc + (i + 1);
    }
    acc
}

fn frame(t: i32) {
    let w: i32 = sum_while(10);
    let f: i32 = sum_for(10);
    // If the two loop forms disagree, paint an obviously-wrong sentinel (red);
    // when they agree, paint the shared value so the harness can pin it.
    if w == f {
        host::display::clear(w);
    } else {
        host::display::clear(16711680);
    }
    host::display::present();
}
