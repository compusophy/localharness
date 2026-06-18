// p06_array_max — reference solution.
//
// Scans a fixed array for its maximum, with the array passed to a helper as a
// PARAMETER (rustlite passes arrays by base pointer, C-style — the callee reads
// the caller's backing region directly). max_of([12,200,47,99,5], 5) == 200,
// painted as the clear colour 0x0000C8 so the blue channel reads 200.

fn max_of(xs: [i32; 5], n: i32) -> i32 {
    let mut best: i32 = xs[0];
    let mut i: i32 = 1;
    while i < n {
        let v: i32 = xs[i];
        if v > best {
            best = v;
        }
        i = i + 1;
    }
    best
}

fn frame(t: i32) {
    let arr = [12, 200, 47, 99, 5];
    let m: i32 = max_of(arr, 5); // 200
    host::display::clear(m);
    host::display::present();
}
