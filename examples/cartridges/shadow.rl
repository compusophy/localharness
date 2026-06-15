// CORPUS: variable SHADOWING — the regression guard for the local-allocation bug
// where a `let` that shadowed an outer-scope `let` of the same name reused (and
// then collided on) a single wasm local slot, silently returning wrong values
// (it surfaced as the `console` Game Boy loading the WRONG cartridge — two
// compose handles read back as one). See src/rustlite/codegen.rs::alloc_local.
//
// Two shapes are exercised, both deterministic:
//   1. A name declared INSIDE an `if` block AND AGAIN in the function body
//      (the exact console shape). Each `let` must get its OWN slot, so the two
//      body reads return their own distinct values (11 and 22, not 11 and 11).
//   2. A SAME-SCOPE shadow with self-reference: `let r = 3; let r = r + 1;` must
//      read the OLD r (3) for the RHS before binding the new r, giving 4 (a 1
//      would mean the RHS read the new, still-uninitialised slot).
//
// The result is packed into the clear colour R=p G=q B=r, so the harness pins a
// single pixel to [11, 22, 4]. A collision/mis-order regresses that pixel.

fn frame(t: i32) {
    // slot 63 = "did we seed yet" (NOT keyed on t==0 — robust if composed).
    if host::display::state_get(63) == 0 {
        host::display::state_set(63, 1);
        let p: i32 = 11; // INNER-scope p
        let q: i32 = 22; // INNER-scope q
        host::display::state_set(0, p);
        host::display::state_set(1, q);
    }
    let p: i32 = host::display::state_get(0); // BODY-scope p, must be its own slot
    let q: i32 = host::display::state_get(1); // BODY-scope q, must be its own slot
    let r: i32 = 3;
    let r: i32 = r + 1; // same-scope self-ref shadow -> 4
    host::display::clear(p * 65536 + q * 256 + r);
    host::display::present();
}
