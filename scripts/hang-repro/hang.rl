// Repro: a cartridge whose frame() never returns. Stands in for the class of
// bug that bricked the app (Tetris/AoE: a heavy or non-terminating loop in
// frame()). On the main thread this freezes the whole app; persisted, it
// re-hangs on every load -> "subdomain requires reset". The Web Worker fix must
// contain THIS: the watchdog terminates the worker, the main thread never blocks.
fn frame(t: i32) {
    let mut x: i32 = 0;
    while x < 1 {
        x = 0;
    }
}
