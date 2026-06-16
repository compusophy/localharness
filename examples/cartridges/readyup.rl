// Ready Up — a portrait phone app built fresh in the new system.
//
// Variable resolution: dims() declares a 270x480 (9:16) framebuffer instead of
// the old 256x144 landscape, so it fills a phone in portrait. Layout, top→bottom:
//   • a SUB / UNSUB toggle button (green when you're subscribed)
//   • the live member count (how many are subscribed to this feed)
//   • a full-width READY UP button that opens the host's text input (prefilled
//     with the default message) so the presser can type a CUSTOM message, then
//     pushes it to everyone subbed
// Identity-gated: a viewer with no wallet taps once to mint one, then can sub.

fn dims() -> i32 {
    // (width << 16) | height  →  270 x 480, a clean 9:16 portrait.
    (270 * 65536) + 480
}

fn ch(x: i32, y: i32, c: i32, col: i32, sc: i32) {
    host::display::draw_char(x, y, c, col, sc);
}

// "SUB" centered in the top toggle (scale 5: glyph 25w, advance 30).
fn label_sub(col: i32) {
    ch(90, 70, 83, col, 5);    // S
    ch(120, 70, 85, col, 5);   // U
    ch(150, 70, 66, col, 5);   // B
}

// "UNSUB" centered in the top toggle (scale 4: advance 24, 5 chars = 120w).
fn label_unsub(col: i32) {
    ch(75, 78, 85, col, 4);    // U
    ch(99, 78, 78, col, 4);    // N
    ch(123, 78, 83, col, 4);   // S
    ch(147, 78, 85, col, 4);   // U
    ch(171, 78, 66, col, 4);   // B
}

// "READY UP" centered on the bottom button (scale 4: advance 24).
fn label_readyup(col: i32) {
    ch(27, 410, 82, col, 4);    // R
    ch(51, 410, 69, col, 4);    // E
    ch(75, 410, 65, col, 4);    // A
    ch(99, 410, 68, col, 4);    // D
    ch(123, 410, 89, col, 4);   // Y
    ch(159, 410, 85, col, 4);   // U
    ch(183, 410, 80, col, 4);   // P
}

fn frame(t: i32) {
    host::display::clear(0x000000);

    let has: i32 = host::agent::viewer_has_identity();
    let subbed: i32 = host::agent::is_subscribed();
    let count: i32 = host::agent::subscriber_count();

    // --- SUB / UNSUB toggle (top) ---
    let mut subcol: i32 = 0x222222;
    if subbed == 1 {
        subcol = 0x119911;
    }
    host::display::fill_rect(35, 50, 200, 80, subcol);
    if subbed == 1 {
        label_unsub(0x000000);
    } else {
        label_sub(0xffffff);
    }

    // --- member count (middle), big ---
    ch(60, 215, 77, 0x666666, 3);   // 'M'
    ch(78, 215, 58, 0x666666, 3);   // ':'
    host::display::draw_number(120, 205, count, 0xffffff, 6);

    // --- READY UP button (bottom, full width) ---
    host::display::fill_rect(0, 400, 270, 80, 0xffffff);
    label_readyup(0x000000);

    // identity hint strip if no wallet yet (a thin amber line above the button)
    if has == 0 {
        host::display::fill_rect(0, 394, 270, 4, 0xaa5500);
    }

    // --- edge-triggered tap (act once per press, not every frame held) ---
    let down: i32 = host::display::pointer_down();
    let prev: i32 = host::display::state_get(0);
    host::display::state_set(0, down);

    if down == 1 {
        if prev == 0 {
            let py: i32 = host::display::pointer_y();
            if has == 0 {
                host::agent::request_identity();
            } else {
                if py < 200 {
                    if subbed == 1 {
                        host::agent::unsubscribe();
                    } else {
                        host::agent::subscribe();
                    }
                }
                if py >= 400 {
                    // Opens the host's composer (text input over the canvas)
                    // prefilled with the default; [send] broadcasts the typed
                    // message to every subscriber.
                    host::agent::broadcast_compose("Ready Up!", "Tap in — it's go time.");
                }
            }
        }
    }

    host::display::present();
}
