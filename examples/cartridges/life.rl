// CORPUS: Conway's GAME OF LIFE on an 8x8 grid — the capstone "stateful grid
// game" (Tetris / Game-of-Life class). It is a real cellular automaton:
//
//   - The 64 cells live in the 64 host STATE SLOTS (slot = y*8 + x), so the
//     grid PERSISTS across frame() calls — each frame steps one generation.
//   - On the FIRST frame (t == 0) it SEEDS a horizontal BLINKER: the three
//     live cells (2,4), (3,4), (4,4) (a 3-in-a-row oscillator).
//   - Every frame reads the 64 slots into a local array `cur`, computes the
//     next generation into a local array `next` via INDEXED WRITES
//     (`next[i] = ...`) applying Conway's rules, writes `next` back to the
//     slots, and draws the grid (one filled block per cell, live vs dead).
//
// Conway's rules (bounded grid, off-grid = dead):
//   - a LIVE cell with 2 or 3 live neighbours survives; otherwise it dies.
//   - a DEAD cell with EXACTLY 3 live neighbours is born.
//
// The blinker is a period-2 oscillator:  horizontal -> vertical -> horizontal.
// After frame(0) the grid is one step past the seed (VERTICAL); after frame(1)
// it is HORIZONTAL again; the live-cell count stays 3 forever. The harness
// (scripts/test-cartridges.mjs) drives several frames against a SHARED state
// map and asserts exactly this oscillation — a deterministic correctness check.
//
// LANGUAGE PROOF: rustlite now has an ARRAY TYPE (`[i32; N]`), so the per-cell
// neighbour count is FACTORED INTO A REAL HELPER `live_neighbours(cur, cx, cy)`
// taking the grid as an array PARAMETER (passed by its base pointer — the
// callee reads the caller's backing region directly, C-style). The `cur`/`next`
// grids use the SIZED REPEAT INIT `[0; 64]` instead of a 64-element literal.
// This is the end-to-end proof: a game factored into helpers compiles + runs.

// Count the live neighbours of cell (cx, cy) in the 3x3 block around it,
// skipping the centre. Off-grid neighbours count as dead (bounded grid).
// `cur` is the grid as an array PARAMETER — read through its base pointer.
fn live_neighbours(cur: [i32; 64], cx: i32, cy: i32) -> i32 {
    let mut nbrs: i32 = 0;
    let mut dy: i32 = -1;
    while dy <= 1 {
        let mut dx: i32 = -1;
        while dx <= 1 {
            if dx != 0 || dy != 0 {
                let nx: i32 = cx + dx;
                let ny: i32 = cy + dy;
                if nx >= 0 && nx < 8 && ny >= 0 && ny < 8 {
                    let nidx: i32 = ny * 8 + nx;
                    nbrs = nbrs + cur[nidx];
                }
            }
            dx = dx + 1;
        }
        dy = dy + 1;
    }
    nbrs
}

fn frame(t: i32) {
    // --- 1. Read the persisted grid out of the 64 state slots into `cur`. ---
    // `[0; 64]` reserves a fresh 64-i32 region (sized repeat init); we overwrite
    // every slot below, so the initial zeros are immaterial.
    let mut cur = [0; 64];

    // On the very first frame, SEED the horizontal blinker instead of reading
    // the (still-empty) slots: cells (2,4), (3,4), (4,4) -> y*8 + x.
    if t == 0 {
        cur[34] = 1; // (2,4)
        cur[35] = 1; // (3,4)
        cur[36] = 1; // (4,4)
    } else {
        let mut i: i32 = 0;
        while i < 64 {
            cur[i] = host::display::state_get(i);
            i = i + 1;
        }
    }

    // --- 2. Compute the next generation into `next` via array WRITES. ---
    let mut next = [0; 64];

    let mut y: i32 = 0;
    while y < 8 {
        let mut x: i32 = 0;
        while x < 8 {
            let idx: i32 = y * 8 + x;
            let alive: i32 = cur[idx];

            // Neighbour count is now a real HELPER call over the array param.
            let nbrs: i32 = live_neighbours(cur, x, y);

            // Conway's rules, branchless-ish via explicit cases.
            let mut born: i32 = 0;
            if alive == 1 {
                // survives on 2 or 3 neighbours.
                if nbrs == 2 || nbrs == 3 {
                    born = 1;
                }
            } else {
                // dead cell becomes alive on exactly 3 neighbours.
                if nbrs == 3 {
                    born = 1;
                }
            }
            next[idx] = born;

            x = x + 1;
        }
        y = y + 1;
    }

    // --- 3. Write `next` back to the slots (persist for the next frame). ---
    let mut w: i32 = 0;
    while w < 64 {
        host::display::state_set(w, next[w]);
        w = w + 1;
    }

    // --- 4. Draw the grid: one block per cell, live = green, dead = dark. ---
    host::display::clear(1052688); // 0x101010 background
    let cell: i32 = 16; // 16px per cell -> 128x128 grid
    let mut gy: i32 = 0;
    while gy < 8 {
        let mut gx: i32 = 0;
        while gx < 8 {
            let gi: i32 = gy * 8 + gx;
            let px: i32 = gx * cell;
            let py: i32 = gy * cell;
            if next[gi] == 1 {
                // live cell: bright green block with a 1px gap.
                host::display::fill_rect(px, py, 15, 15, 65280);
            } else {
                // dead cell: faint grey block.
                host::display::fill_rect(px, py, 15, 15, 3158064); // 0x303030
            }
            gx = gx + 1;
        }
        gy = gy + 1;
    }

    host::display::present();
}
