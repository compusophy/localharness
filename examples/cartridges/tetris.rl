// tetris.rl — a self-playing TETRIS cartridge (160x144), built so the logic is
// cleanly separated into small functions instead of one monolithic frame():
//   board model : cell_get / cell_set (10 cells x 3 bits packed per row slot)
//   pieces      : piece_cell (7 tetrominoes, rotated on the fly) / color_of
//   rules       : collides / lock_piece / clear_lines / lowest_col / spawn
//   step        : step_down (gravity + lock + clear + respawn)
//   render      : draw_block / draw (board, falling piece, score)
//
// It runs as its own face at tetris.localharness.xyz AND as a cartridge inside
// the `console` Game Boy (host::compose). Because a composed child does NOT get
// t==0 on its first tick (it inherits the host clock), all state is guarded by an
// init flag (slot 23) and gravity is driven by an INTERNAL tick counter (slot 21),
// never by the raw t — so it behaves identically standalone and embedded.
//
// State slots: 0..15 board rows · 16 type · 17 col · 18 row · 19 rotation
//              20 rng seed · 21 tick · 22 lines · 23 init flag

fn dims() -> i32 {
    (160 * 65536) + 144
}

// Read cell x (0..9) of a packed row (3 bits per cell, color 0..7).
fn cell_get(rowbits: i32, x: i32) -> i32 {
    (rowbits >> (x * 3)) & 7
}

// Return `rowbits` with cell x set to `color` (XOR out the old 3 bits, OR in new
// — avoids a bitwise NOT, which rustlite doesn't have).
fn cell_set(rowbits: i32, x: i32, color: i32) -> i32 {
    let sh: i32 = x * 3;
    let old: i32 = (rowbits >> sh) & 7;
    let cleared: i32 = rowbits ^ (old << sh);
    cleared | (color << sh)
}

// The i-th cell (0..3) of tetromino `ptype` (0..6) at rotation `rot`, packed as
// y*8 + x within a 4x4 box. Only the 7 BASE shapes are stored; rotations are
// computed by applying (x,y) -> (n-1-y, x) `rot` times in an n-sized box.
fn piece_cell(ptype: i32, rot: i32, i: i32) -> i32 {
    // base cells, packed y*4 + x:  I, O, T, S, Z, J, L
    let base = [4, 5, 6, 7, 0, 1, 4, 5, 1, 4, 5, 6, 1, 2, 4, 5, 0, 1, 5, 6, 0, 4, 5, 6, 2, 4, 5, 6];
    let boxes = [4, 2, 3, 3, 3, 3, 3];
    let n: i32 = boxes[ptype];
    let c: i32 = base[ptype * 4 + i];
    let mut x: i32 = c % 4;
    let mut y: i32 = c / 4;
    let mut r: i32 = rot % 4;
    while r > 0 {
        let nx: i32 = n - 1 - y;
        let ny: i32 = x;
        x = nx;
        y = ny;
        r = r - 1;
    }
    y * 8 + x
}

// Classic Tetris colours, indexed by piece type (0..6).
fn color_of(ptype: i32) -> i32 {
    let pal = [0x00f0f0, 0xf0f000, 0xa000f0, 0x00f000, 0xf00000, 0x2030f0, 0xf0a000];
    pal[ptype]
}

// 1 if placing `ptype`/`rot` at (col,row) hits a wall, the floor, or a filled
// cell; 0 if it fits. Cells above the top (cy<0) are allowed (spawn overhang).
fn collides(ptype: i32, rot: i32, col: i32, row: i32) -> i32 {
    let mut hit: i32 = 0;
    let mut i: i32 = 0;
    while i < 4 {
        let v: i32 = piece_cell(ptype, rot, i);
        let cx: i32 = col + (v % 8);
        let cy: i32 = row + (v / 8);
        if cx < 0 || cx >= 10 || cy >= 16 {
            hit = 1;
        } else {
            if cy >= 0 {
                if cell_get(host::display::state_get(cy), cx) != 0 {
                    hit = 1;
                }
            }
        }
        i = i + 1;
    }
    hit
}

// Write the piece into the board (colour = type+1, so 0 stays "empty").
fn lock_piece(ptype: i32, rot: i32, col: i32, row: i32) {
    let mut i: i32 = 0;
    while i < 4 {
        let v: i32 = piece_cell(ptype, rot, i);
        let cx: i32 = col + (v % 8);
        let cy: i32 = row + (v / 8);
        if cy >= 0 && cy < 16 {
            let nb: i32 = cell_set(host::display::state_get(cy), cx, ptype + 1);
            host::display::state_set(cy, nb);
        }
        i = i + 1;
    }
}

// Remove every full row (all 10 cells non-empty), shifting the rows above down,
// and bump the line counter (slot 22). Re-checks the same row after a shift.
fn clear_lines() {
    let mut r: i32 = 15;
    while r >= 0 {
        let rowbits: i32 = host::display::state_get(r);
        let mut full: i32 = 1;
        let mut x: i32 = 0;
        while x < 10 {
            if cell_get(rowbits, x) == 0 {
                full = 0;
            }
            x = x + 1;
        }
        if full == 1 {
            let mut rr: i32 = r;
            while rr > 0 {
                host::display::state_set(rr, host::display::state_get(rr - 1));
                rr = rr - 1;
            }
            host::display::state_set(0, 0);
            host::display::state_set(22, host::display::state_get(22) + 1);
        } else {
            r = r - 1;
        }
    }
}

// How far `ptype`/`rot` falls in `col` before it locks: the landing row, or -1
// if the column is invalid (the piece can't even sit at the top there).
fn drop_row(ptype: i32, rot: i32, col: i32) -> i32 {
    let mut r: i32 = 0 - 1;
    if collides(ptype, rot, col, 0) == 0 {
        r = 0;
        while collides(ptype, rot, col, r + 1) == 0 {
            r = r + 1;
        }
    }
    r
}

// A pseudo-random value in 0..n (LCG in slot 20; i32 multiply wraps, which is
// fine for piece variety). Folded positive.
fn rng_mod(n: i32) -> i32 {
    let s1: i32 = host::display::state_get(20) * 1103515245 + 12345;
    host::display::state_set(20, s1);
    let v: i32 = (s1 / 65536) % 32768;
    let mut r: i32 = v % n;
    if r < 0 {
        r = r + n;
    }
    r
}

// Pick the next piece (random type) and, like a player would, choose the
// rotation + column where it lands DEEPEST — this fills valleys evenly across
// all 10 columns, so rows actually complete and clear (instead of one tower
// topping out). If nothing can be placed, the well is full → wipe for a new game.
fn spawn() {
    let ptype: i32 = rng_mod(7);
    let mut best_rot: i32 = 0;
    let mut best_col: i32 = 0;
    let mut best_row: i32 = 0 - 1;
    let mut rot: i32 = 0;
    while rot < 4 {
        let mut col: i32 = 0;
        while col < 10 {
            let lr: i32 = drop_row(ptype, rot, col);
            if lr > best_row {
                best_row = lr;
                best_rot = rot;
                best_col = col;
            }
            col = col + 1;
        }
        rot = rot + 1;
    }
    if best_row < 0 {
        let mut i: i32 = 0;
        while i < 16 {
            host::display::state_set(i, 0);
            i = i + 1;
        }
        host::display::state_set(22, 0);
        best_rot = 0;
        best_col = 0;
    }
    host::display::state_set(16, ptype);
    host::display::state_set(19, best_rot);
    host::display::state_set(17, best_col);
    host::display::state_set(18, 0);
}

// One gravity step: fall a row if it fits, otherwise lock + clear + respawn.
fn step_down() {
    let ptype: i32 = host::display::state_get(16);
    let rot: i32 = host::display::state_get(19);
    let col: i32 = host::display::state_get(17);
    let row: i32 = host::display::state_get(18);
    if collides(ptype, rot, col, row + 1) == 0 {
        host::display::state_set(18, row + 1);
    } else {
        lock_piece(ptype, rot, col, row);
        clear_lines();
        spawn();
    }
}

// A single beveled board cell (sz px, 1px gap so the grid reads).
fn draw_block(px: i32, py: i32, sz: i32, color: i32) {
    host::display::fill_rect(px, py, sz - 1, sz - 1, color);
    host::display::fill_rect(px, py, sz - 1, 1, 0xc8c8c8);
    host::display::fill_rect(px, py, 1, sz - 1, 0xc8c8c8);
    host::display::fill_rect(px, py + sz - 2, sz - 1, 1, 0x303030);
    host::display::fill_rect(px + sz - 2, py, 1, sz - 1, 0x303030);
}

// Draw the whole scene: well, board, falling piece, and the score sidebar.
fn draw() {
    let sz: i32 = 7;
    let bx: i32 = 8;
    let by: i32 = 10;
    host::display::clear(0x101018);
    // well border + black playfield
    host::display::fill_rect(bx - 2, by - 2, 10 * sz + 4, 16 * sz + 4, 0x383850);
    host::display::fill_rect(bx, by, 10 * sz, 16 * sz, 0x000000);

    // settled blocks
    let mut r: i32 = 0;
    while r < 16 {
        let rowbits: i32 = host::display::state_get(r);
        let mut x: i32 = 0;
        while x < 10 {
            let cell: i32 = cell_get(rowbits, x);
            if cell != 0 {
                draw_block(bx + x * sz, by + r * sz, sz, color_of(cell - 1));
            }
            x = x + 1;
        }
        r = r + 1;
    }

    // the falling piece
    let ptype: i32 = host::display::state_get(16);
    let rot: i32 = host::display::state_get(19);
    let col: i32 = host::display::state_get(17);
    let row: i32 = host::display::state_get(18);
    let mut i: i32 = 0;
    while i < 4 {
        let v: i32 = piece_cell(ptype, rot, i);
        let cx: i32 = col + (v % 8);
        let cy: i32 = row + (v / 8);
        if cy >= 0 {
            draw_block(bx + cx * sz, by + cy * sz, sz, color_of(ptype));
        }
        i = i + 1;
    }

    // sidebar: title + line count
    let title = [84, 69, 84, 82, 73, 83]; // "TETRIS"
    let mut ti: i32 = 0;
    while ti < 6 {
        host::display::draw_char(86 + ti * 6, 14, title[ti], 0xf0f0f0, 1);
        ti = ti + 1;
    }
    let lab = [76, 73, 78, 69, 83]; // "LINES"
    let mut li: i32 = 0;
    while li < 5 {
        host::display::draw_char(90 + li * 6, 40, lab[li], 0x8890a0, 1);
        li = li + 1;
    }
    host::display::draw_number(96, 52, host::display::state_get(22), 0xf0f000, 2);

    host::display::present();
}

fn frame(t: i32) {
    // one-time init (NOT keyed on t==0 — a composed child never sees t==0)
    if host::display::state_get(23) == 0 {
        let mut i: i32 = 0;
        while i < 16 {
            host::display::state_set(i, 0);
            i = i + 1;
        }
        host::display::state_set(20, t + 1); // rng seed
        host::display::state_set(21, 0); // tick
        host::display::state_set(22, 0); // lines
        host::display::state_set(23, 1); // mark initialised
        spawn();
    }

    // gravity: advance an internal tick each frame, drop one row every 5 ticks
    let tick: i32 = host::display::state_get(21) + 1;
    host::display::state_set(21, tick);
    if tick % 5 == 0 {
        step_down();
    }

    draw();
}
