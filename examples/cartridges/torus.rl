// torus.rl — a spinning 3D torus, round the way it should be.
// Built on host::math::sin/cos (telemetry #83): angle = 1/256 turn,
// values scaled by 256. Weak-perspective projection, painter-free
// point cloud with depth-shaded 2px dots.

fn shade(z: i32) -> i32 {
    // z roughly -110..110 -> brightness 60..255 (green-white).
    let mut b: i32 = 158 + z;
    if b < 60 { b = 60; }
    if b > 255 { b = 255; }
    (b << 16) | (255 << 8) | b
}

fn frame(t: i32) {
    host::display::clear(0);
    let cx: i32 = host::display::width() / 2;
    let cy: i32 = host::display::height() / 2;
    let big: i32 = 78;   // major radius
    let small: i32 = 34; // minor radius
    let rot_a: i32 = t;      // spin around y
    let rot_b: i32 = t / 2;  // tilt around x

    let mut u: i32 = 0;
    while u < 256 {
        let mut v: i32 = 0;
        while v < 256 {
            // Tube-center circle + tube offset.
            let ring: i32 = big + (small * host::math::cos(v)) / 256;
            let x0: i32 = (ring * host::math::cos(u)) / 256;
            let z0: i32 = (ring * host::math::sin(u)) / 256;
            let y0: i32 = (small * host::math::sin(v)) / 256;

            // Rotate around y by rot_a.
            let ca: i32 = host::math::cos(rot_a);
            let sa: i32 = host::math::sin(rot_a);
            let x1: i32 = (x0 * ca - z0 * sa) / 256;
            let z1: i32 = (x0 * sa + z0 * ca) / 256;

            // Tilt around x by rot_b.
            let cb: i32 = host::math::cos(rot_b);
            let sb: i32 = host::math::sin(rot_b);
            let y2: i32 = (y0 * cb - z1 * sb) / 256;
            let z2: i32 = (y0 * sb + z1 * cb) / 256;

            // Weak perspective: z2 in ~[-115,115]; camera at 300.
            let depth: i32 = z2 + 300;
            let px: i32 = cx + (x1 * 340) / depth;
            let py: i32 = cy + (y2 * 340) / depth;
            host::display::fill_rect(px, py, 2, 2, shade(z2));
            v = v + 12;
        }
        u = u + 5;
    }

    host::display::draw_string(10, 10, "host math torus", 4210752, 1);
    host::display::present();
}
