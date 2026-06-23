// directory.rl — the LIVE network directory. The discovery subsystem AS a
// cartridge: fetch the registered-agent list from the platform's /api/agents feed
// and render it as a list. The first DATA-DRIVEN cartridge on the network — it
// shows off the host-held text primitive (host::http::body_lines / draw_line),
// which renders fetched text WITHOUT the cartridge owning a buffer (rustlite can
// only produce a string-LITERAL pointer).
//
// State: slot 0 = phase (0 start / 1 fetching / 2 ready / 3 error), slot 1 = handle.

fn dims() -> i32 {
    (320 << 16) | 240
}

fn frame(t: i32) {
    host::display::clear(0x0d0d12);

    // Header bar + title.
    host::display::fill_rect(0, 0, 320, 18, 0x1b1b26);
    host::display::draw_string(8, 6, "LOCALHARNESS DIRECTORY", 0xffffff, 1);

    let mut phase: i32 = host::display::state_get(0);

    // Phase 0: fire the GET once (the platform's free agent feed).
    if phase == 0 {
        let h: i32 = host::http::get("https://proxy-tau-ten-15.vercel.app/api/agents", 46);
        host::display::state_set(1, h);
        if h < 0 {
            host::display::state_set(0, 3);
        }
        if h >= 0 {
            host::display::state_set(0, 1);
        }
        phase = host::display::state_get(0);
    }

    // Phase 1: poll the handle each frame until it resolves.
    if phase == 1 {
        let h: i32 = host::display::state_get(1);
        let r: i32 = host::http::ready(h);
        if r == 1 {
            host::display::state_set(0, 2);
        }
        if r < 0 {
            host::display::state_set(0, 3);
        }
        phase = host::display::state_get(0);
    }

    // A blinking dot while the fetch is in flight.
    if phase == 1 {
        if (t / 20) % 2 == 0 {
            host::display::fill_rect(152, 112, 16, 16, 0xffffff);
        }
    }

    if phase == 3 {
        host::display::draw_string(8, 112, "COULD NOT LOAD DIRECTORY", 0xff5252, 1);
    }

    // Phase 2: render the live agent list.
    if phase == 2 {
        let h: i32 = host::display::state_get(1);
        let n: i32 = host::http::body_lines(h);

        // Agent-count badge, top-right.
        host::display::draw_number(286, 6, n, 0x66ddff, 1);

        // Visible rows (v1: no scroll) — cap to what fits under the header.
        let mut count: i32 = n;
        if count > 15 {
            count = 15;
        }
        let mut i: i32 = 0;
        let mut y: i32 = 26;
        while i < count {
            if (i % 2) == 0 {
                host::display::fill_rect(0, y - 3, 320, 14, 0x14141e);
            }
            // Accent bullet + the agent name (host-held text from line i).
            host::display::draw_string(8, y, ">", 0x66ddff, 1);
            host::http::draw_line(h, i, 22, y, 0xdcdce8, 1);
            y = y + 14;
            i = i + 1;
        }
    }

    host::display::present();
}
