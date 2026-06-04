// claude.localharness.xyz — BITMASK COMPOSER.
//
// A real dev tool, served on-chain to every visitor with no browser tab.
// Click the 16 bit-cells to toggle them; read the live DEC + HEX value;
// the buttons shift left/right, clear, and invert the 16-bit register.
// Monochrome framebuffer, no bitwise ops in rustlite — bits are arithmetic.

fn pow2(n: i32) -> i32 {
    let mut r: i32 = 1;
    let mut i: i32 = 0;
    while i < n {
        r = r * 2;
        i = i + 1;
    }
    r
}

fn bit_at(value: i32, b: i32) -> i32 {
    (value / pow2(b)) % 2
}

fn nibble_char(n: i32) -> i32 {
    if n < 10 {
        48 + n
    } else {
        55 + n
    }
}

fn draw_cell(i: i32, on: i32) {
    let x: i32 = i * 16;
    if on == 1 {
        host::display::fill_rect(x + 1, 36, 14, 28, 16777215);
    } else {
        host::display::fill_rect(x + 1, 36, 14, 28, 4210752);
        host::display::fill_rect(x + 2, 37, 12, 26, 0);
    }
}

fn btn_box(x: i32) {
    host::display::fill_rect(x + 2, 116, 60, 22, 4210752);
    host::display::fill_rect(x + 3, 117, 58, 20, 0);
}

fn frame(t: i32) {
    host::display::clear(0);

    let mut value: i32 = host::display::state_get(0);
    let prev: i32 = host::display::state_get(1);
    let down: i32 = host::display::pointer_down();

    // Edge-triggered: act once on the down-stroke, not every held frame.
    if down == 1 {
        if prev == 0 {
            let px: i32 = host::display::pointer_x();
            let py: i32 = host::display::pointer_y();
            if py >= 36 {
                if py < 64 {
                    let idx: i32 = px / 16;
                    if idx >= 0 {
                        if idx < 16 {
                            let b: i32 = 15 - idx;
                            if bit_at(value, b) == 1 {
                                value = value - pow2(b);
                            } else {
                                value = value + pow2(b);
                            }
                        }
                    }
                }
            }
            if py >= 116 {
                if py < 140 {
                    if px < 64 {
                        value = (value * 2) % 65536;
                    } else {
                        if px < 128 {
                            value = value / 2;
                        } else {
                            if px < 192 {
                                value = 0;
                            } else {
                                value = 65535 - value;
                            }
                        }
                    }
                }
            }
        }
    }
    host::display::state_set(0, value);
    host::display::state_set(1, down);

    // Title: BITMASK
    host::display::draw_char(4, 6, 66, 16777215, 2);
    host::display::draw_char(16, 6, 73, 16777215, 2);
    host::display::draw_char(28, 6, 84, 16777215, 2);
    host::display::draw_char(40, 6, 77, 16777215, 2);
    host::display::draw_char(52, 6, 65, 16777215, 2);
    host::display::draw_char(64, 6, 83, 16777215, 2);
    host::display::draw_char(76, 6, 75, 16777215, 2);

    // 16 bit-cells, MSB on the left.
    let mut i: i32 = 0;
    while i < 16 {
        let b: i32 = 15 - i;
        draw_cell(i, bit_at(value, b));
        i = i + 1;
    }

    // DEC
    host::display::draw_char(4, 76, 68, 8421504, 1);
    host::display::draw_char(10, 76, 69, 8421504, 1);
    host::display::draw_char(16, 76, 67, 8421504, 1);
    host::display::draw_number(30, 74, value, 16777215, 2);

    // HEX (4 nibbles, MSnibble left)
    host::display::draw_char(4, 96, 72, 8421504, 1);
    host::display::draw_char(10, 96, 69, 8421504, 1);
    host::display::draw_char(16, 96, 88, 8421504, 1);
    let mut k: i32 = 0;
    while k < 4 {
        let shift: i32 = (3 - k) * 4;
        let nib: i32 = (value / pow2(shift)) % 16;
        host::display::draw_char(30 + k * 12, 94, nibble_char(nib), 16777215, 2);
        k = k + 1;
    }

    // Buttons: <<  >>  CLR  INV
    btn_box(0);
    btn_box(64);
    btn_box(128);
    btn_box(192);
    host::display::draw_char(20, 120, 60, 16777215, 2);
    host::display::draw_char(32, 120, 60, 16777215, 2);
    host::display::draw_char(84, 120, 62, 16777215, 2);
    host::display::draw_char(96, 120, 62, 16777215, 2);
    host::display::draw_char(140, 120, 67, 16777215, 2);
    host::display::draw_char(152, 120, 76, 16777215, 2);
    host::display::draw_char(164, 120, 82, 16777215, 2);
    host::display::draw_char(204, 120, 73, 16777215, 2);
    host::display::draw_char(216, 120, 78, 16777215, 2);
    host::display::draw_char(228, 120, 86, 16777215, 2);

    host::display::present();
}
