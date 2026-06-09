// CORPUS: recursion (factorial + Fibonacci).
//
// Exercises: self-recursive function calls, the call/return ABI under deep-ish
// nesting, and conditional base cases. Both classic recursive forms are run and
// their results painted so the harness can pin exact values.
//
//   fact(5) = 120  -> 0x000078 (120 = 0x78)
//   fib(10) = 55   -> 0x003700 (55 = 0x37, in the green channel)
//
// A non-trapping deep call chain proves the codegen's locals/stack handling for
// recursion is sound (a common place bad codegen blows the wasm stack or traps).

fn fact(n: i32) -> i32 {
    if n <= 1 {
        1
    } else {
        n * fact(n - 1)
    }
}

fn fib(n: i32) -> i32 {
    if n < 2 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn frame(t: i32) {
    let f: i32 = fact(5); // 120
    let g: i32 = fib(10); // 55
    // Pack fact into the blue channel and fib into the green channel so the
    // harness reads both back from one pixel: 0x00<fib><fact> = 0x003778.
    host::display::clear(g * 256 + f);
    host::display::present();
}
