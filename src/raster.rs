//! Pure, native-testable framebuffer rasterization with a viewport — the
//! geometry foundation for `host::compose` (roadmap Phase 0a, `design/host-
//! compose.md`).
//!
//! A [`Viewport`](crate::raster::Viewport) offsets and clips a cartridge's draw calls into a sub-
//! rectangle of the shared RGBA framebuffer. The single-cartridge path uses
//! [`Viewport::full`](crate::raster::Viewport::full) — an identity transform that reproduces the pre-refactor
//! behavior byte-for-byte. Because these functions operate on a plain `&mut
//! [u8]` (not a web-sys canvas), they are unit-tested natively here — closing
//! the gap the design flagged, where the wasm-only display closures can't be
//! exercised by `cargo test`. `src/app/display.rs` calls into these from its
//! host-import closures.
//!
//! Scope note (Phase 0a): `clear` / `set_pixel` / `fill_rect` are wired here;
//! glyph blitting (`draw_char`/`draw_number`) and the present-ownership
//! inversion are deferred to the follow-up, which lands them under a real
//! wasm-instantiation render test (the part a pure unit test can't prove).

/// Upper bound on an integer glyph scale (`blit_glyph` / `draw_number`). A
/// glyph scaled past this already exceeds any framebuffer; the cap stops a
/// hostile/garbage scale from overflowing `col*scale` or turning the
/// `scale x scale` fill into a multi-billion-iteration hang.
const MAX_GLYPH_SCALE: i32 = 256;

/// Max endpoint span (in either axis) for which [`draw_line`] takes its exact
/// Bresenham path. Far beyond any framebuffer (2^20 = ~1M px) yet small enough
/// that the walk can't hang and the i32 deltas can't overflow; a longer span is
/// clipped to the viewport first. Lines within this (every reachable line) are
/// unchanged.
const MAX_LINE_SPAN: i64 = 1 << 20;

/// A sub-rectangle of the shared framebuffer a cartridge draws into. Child-
/// local coordinates are translated by `(ox, oy)` and clipped to
/// `[0, w) x [0, h)` (the viewport) and then to the framebuffer bounds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Viewport {
    pub ox: i32,
    pub oy: i32,
    pub w: i32,
    pub h: i32,
}

impl Viewport {
    /// The whole framebuffer — the identity transform (single-cartridge path).
    pub fn full(fb_w: i32, fb_h: i32) -> Self {
        Self { ox: 0, oy: 0, w: fb_w, h: fb_h }
    }
}

/// Write one opaque RGBA pixel at child-local `(x, y)`, translated and clipped
/// by `vp` and the framebuffer width. No-op if out of the viewport or buffer.
#[inline]
pub fn set_pixel(buf: &mut [u8], fb_w: i32, vp: &Viewport, x: i32, y: i32, rgb: (u8, u8, u8)) {
    if x < 0 || y < 0 || x >= vp.w || y >= vp.h {
        return;
    }
    // Global coord + byte index in i64 (with saturating mul) so a pathological
    // viewport — huge `ox`/`oy`, or a height past 2^20 — can't overflow the
    // offset past the bounds guard into an OOB write. The previous `usize` math
    // was sound on native (64-bit) but DEFEATED on wasm32 (the live target,
    // 32-bit usize): `gy*fb_w` could wrap to a small in-bounds index. i64 is
    // 64-bit on every target, so the guard now holds everywhere.
    let gx = vp.ox as i64 + x as i64;
    let gy = vp.oy as i64 + y as i64;
    if gx < 0 || gy < 0 || gx >= fb_w as i64 {
        return;
    }
    let idx = gy
        .saturating_mul(fb_w as i64)
        .saturating_add(gx)
        .saturating_mul(4);
    if idx < 0 || idx.saturating_add(3) >= buf.len() as i64 {
        return;
    }
    let idx = idx as usize;
    buf[idx] = rgb.0;
    buf[idx + 1] = rgb.1;
    buf[idx + 2] = rgb.2;
    buf[idx + 3] = 255;
}

/// Fill a child-local rectangle, clipped to the viewport. Routes every pixel
/// through [`set_pixel`] so translation/clipping stay consistent.
#[allow(clippy::too_many_arguments)] // low-level raster primitive: fb + viewport + rect + color
pub fn fill_rect(
    buf: &mut [u8],
    fb_w: i32,
    vp: &Viewport,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    rgb: (u8, u8, u8),
) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = x.saturating_add(w).min(vp.w);
    let y1 = y.saturating_add(h).min(vp.h);
    let mut yy = y0;
    while yy < y1 {
        let mut xx = x0;
        while xx < x1 {
            set_pixel(buf, fb_w, vp, xx, yy, rgb);
            xx += 1;
        }
        yy += 1;
    }
}

/// Clear the viewport (not the whole framebuffer — that distinction is what
/// lets multiple modules share one canvas). For [`Viewport::full`] this fills
/// the entire framebuffer, exactly as the old whole-buffer clear did.
pub fn clear(buf: &mut [u8], fb_w: i32, vp: &Viewport, rgb: (u8, u8, u8)) {
    fill_rect(buf, fb_w, vp, 0, 0, vp.w, vp.h, rgb);
}

/// Blit a single 5x7 glyph at child-local `(x, y)`, integer-scaled, every lit
/// pixel routed through [`set_pixel`] (so it translates+clips by the viewport).
#[allow(clippy::too_many_arguments)] // raster primitive: fb + viewport + pos + glyph + color + scale
pub fn blit_glyph(
    buf: &mut [u8],
    fb_w: i32,
    vp: &Viewport,
    x: i32,
    y: i32,
    code: u32,
    color: (u8, u8, u8),
    scale: i32,
) {
    let glyph = glyph_5x7(code);
    // Clamp the scale: a glyph past 256x already dwarfs any framebuffer, and an
    // unbounded scale both overflows `col*scale` AND makes the `scale x scale`
    // fill loop a hang (a brick). 256 keeps `col*scale`/`6*scale` well within i32.
    let scale = scale.clamp(1, MAX_GLYPH_SCALE);
    for (row, bits) in glyph.iter().enumerate() {
        for col in 0..5i32 {
            if (bits >> (4 - col)) & 1 == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    set_pixel(buf, fb_w, vp, x + col * scale + dx, y + row as i32 * scale + dy, color);
                }
            }
        }
    }
}

/// Draw a base-10 signed integer at child-local `(x, y)`, integer-scaled, via
/// [`blit_glyph`] (so it honors the viewport). Advance is 6px per glyph scaled.
#[allow(clippy::too_many_arguments)] // raster primitive: fb + viewport + pos + value + color + scale
pub fn draw_number(
    buf: &mut [u8],
    fb_w: i32,
    vp: &Viewport,
    x: i32,
    y: i32,
    value: i32,
    color: (u8, u8, u8),
    scale: i32,
) {
    let s = scale.clamp(1, MAX_GLYPH_SCALE); // see blit_glyph: bounds 6*s + the fill loop
    let advance = 6 * s; // 5px glyph + 1px gap, scaled
    let mut cx = x;
    let mut n = (value as i64).unsigned_abs();
    if value < 0 {
        blit_glyph(buf, fb_w, vp, cx, y, '-' as u32, color, s);
        cx += advance;
    }
    // Collect digits (least-significant first), then draw reversed.
    let mut digits = [0u8; 20];
    let mut count = 0;
    if n == 0 {
        digits[0] = b'0';
        count = 1;
    } else {
        while n > 0 {
            digits[count] = b'0' + (n % 10) as u8;
            n /= 10;
            count += 1;
        }
    }
    for i in (0..count).rev() {
        blit_glyph(buf, fb_w, vp, cx, y, digits[i] as u32, color, s);
        cx += advance;
    }
}

/// Clip segment `(x0,y0)-(x1,y1)` to the viewport rect `[0, w-1] x [0, h-1]`
/// (Liang-Barsky in f64 — exact for i32 coords since |coord| < 2^31 < 2^52, and
/// overflow-free, the same f64 approach `fill_triangle_z` already uses). Returns
/// the clipped integer endpoints, or `None` if the segment is entirely outside.
/// Only used by [`draw_line`] for spans past `MAX_LINE_SPAN`, to bound the walk.
fn clip_line(w: i32, h: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> Option<(i32, i32, i32, i32)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let (fx0, fy0) = (x0 as f64, y0 as f64);
    let dx = x1 as f64 - x0 as f64; // cast BEFORE subtracting (i32 sub would overflow)
    let dy = y1 as f64 - y0 as f64;
    let (xmax, ymax) = ((w - 1) as f64, (h - 1) as f64);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    // Each clip boundary as (p, q): the segment is inside where p*t <= q.
    for (p, q) in [(-dx, fx0), (dx, xmax - fx0), (-dy, fy0), (dy, ymax - fy0)] {
        if p == 0.0 {
            if q < 0.0 {
                return None; // parallel to this edge and outside it
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                if t > t1 {
                    return None;
                }
                if t > t0 {
                    t0 = t;
                }
            } else {
                if t < t0 {
                    return None;
                }
                if t < t1 {
                    t1 = t;
                }
            }
        }
    }
    Some((
        (fx0 + t0 * dx).round() as i32,
        (fy0 + t0 * dy).round() as i32,
        (fx0 + t1 * dx).round() as i32,
        (fy0 + t1 * dy).round() as i32,
    ))
}

/// Draw a 1px line from child-local `(x0,y0)` to `(x1,y1)` (integer
/// Bresenham), every pixel routed through [`set_pixel`] so it translates+
/// clips by the viewport. The line counterpart of the triangle fill —
/// wireframe edges, axes, vectors.
#[allow(clippy::too_many_arguments)] // raster primitive: fb + viewport + 2 points + color
pub fn draw_line(
    buf: &mut [u8],
    fb_w: i32,
    vp: &Viewport,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    rgb: (u8, u8, u8),
) {
    // Bound the Bresenham walk. A line whose span fits MAX_LINE_SPAN (any line a
    // framebuffer could hold, and then some) takes the original exact path —
    // byte-identical to before. Only an EXTREME endpoint (which would overflow
    // `(x1-x0).abs()` and make the walk |dx| ≈ 2^32 steps — a hang) is first
    // CLIPPED to the viewport, so the walk is always bounded and overflow-free.
    let span = (x1 as i64 - x0 as i64).abs().max((y1 as i64 - y0 as i64).abs());
    let (x0, y0, x1, y1) = if span > MAX_LINE_SPAN {
        match clip_line(vp.w, vp.h, x0, y0, x1, y1) {
            Some(c) => c,
            None => return, // segment entirely outside the viewport
        }
    } else {
        (x0, y0, x1, y1)
    };
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let (mut x, mut y) = (x0, y0);
    loop {
        set_pixel(buf, fb_w, vp, x, y, rgb);
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Twice the signed area of triangle `(a,b,c)` — the edge function for
/// barycentric coverage. `> 0` is CCW; widened to `i64` so framebuffer coords
/// (up to the 1024×1024 cartridge-declared max) can't overflow the cross product.
#[inline]
fn edge(ax: i32, ay: i32, bx: i32, by: i32, cx: i32, cy: i32) -> i128 {
    // i128 so NO i32 input can overflow: each delta widens to i128 before the
    // subtract (an i32 `bx-ax` overflowed on e.g. i32::MAX-i32::MIN), and the
    // product of two full-i32-range deltas (~2^32 each → ~2^64) fits i128 with
    // room to spare. Coverage sign tests + the i128→f64 barycentric cast are
    // unaffected; for framebuffer-scale coords the value is identical to before.
    (bx as i128 - ax as i128) * (cy as i128 - ay as i128)
        - (by as i128 - ay as i128) * (cx as i128 - ax as i128)
}

/// Fill the triangle `(x0,y0),(x1,y1),(x2,y2)` with a flat colour, scanline-
/// rasterized via integer barycentric edge functions. Only the triangle's
/// bounding box is scanned, clipped to the viewport; every covered pixel is
/// routed through [`set_pixel`] (so it translates+clips like every other
/// primitive). Winding-agnostic. The flat-shaded software-3D primitive —
/// overlap order is the cartridge's responsibility (painter's algorithm);
/// use [`fill_triangle_z`] for correct occlusion.
#[allow(clippy::too_many_arguments)] // raster primitive: fb + viewport + 3 points + color
pub fn fill_triangle(
    buf: &mut [u8],
    fb_w: i32,
    vp: &Viewport,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    rgb: (u8, u8, u8),
) {
    let min_x = x0.min(x1).min(x2).max(0);
    let min_y = y0.min(y1).min(y2).max(0);
    let max_x = x0.max(x1).max(x2).min(vp.w.saturating_sub(1));
    let max_y = y0.max(y1).max(y2).min(vp.h.saturating_sub(1));
    if min_x > max_x || min_y > max_y {
        return;
    }
    let area = edge(x0, y0, x1, y1, x2, y2);
    if area == 0 {
        return; // degenerate
    }
    let positive = area > 0;
    let mut py = min_y;
    while py <= max_y {
        let mut px = min_x;
        while px <= max_x {
            let w0 = edge(x1, y1, x2, y2, px, py);
            let w1 = edge(x2, y2, x0, y0, px, py);
            let w2 = edge(x0, y0, x1, y1, px, py);
            // Inside test independent of winding (all weights share `area`'s sign).
            let inside = if positive {
                w0 >= 0 && w1 >= 0 && w2 >= 0
            } else {
                w0 <= 0 && w1 <= 0 && w2 <= 0
            };
            if inside {
                set_pixel(buf, fb_w, vp, px, py, rgb);
            }
            px += 1;
        }
        py += 1;
    }
}

/// Fill a triangle with per-vertex depth `(z0,z1,z2)` (caller-provided i32
/// fixed-point — any monotonic "nearer = smaller"), z-testing against
/// `depth` (one i32 per framebuffer pixel, indexed by the GLOBAL pixel like
/// `buf`). A pixel is drawn (and its depth updated) only when its
/// barycentric-interpolated z is `<` the stored z, giving correct occlusion
/// of interpenetrating triangles. `depth` MUST be `fb_w * fb_h` long;
/// out-of-viewport / out-of-buffer pixels are skipped exactly as in
/// [`set_pixel`]. Reset `depth` per frame with [`clear_depth`].
#[allow(clippy::too_many_arguments)] // raster primitive: fb + depth + viewport + 3 points+z + color
pub fn fill_triangle_z(
    buf: &mut [u8],
    depth: &mut [i32],
    fb_w: i32,
    vp: &Viewport,
    x0: i32,
    y0: i32,
    z0: i32,
    x1: i32,
    y1: i32,
    z1: i32,
    x2: i32,
    y2: i32,
    z2: i32,
    rgb: (u8, u8, u8),
) {
    let min_x = x0.min(x1).min(x2).max(0);
    let min_y = y0.min(y1).min(y2).max(0);
    let max_x = x0.max(x1).max(x2).min(vp.w.saturating_sub(1));
    let max_y = y0.max(y1).max(y2).min(vp.h.saturating_sub(1));
    if min_x > max_x || min_y > max_y {
        return;
    }
    let area = edge(x0, y0, x1, y1, x2, y2);
    if area == 0 {
        return;
    }
    let positive = area > 0;
    let area_f = area as f64;
    let mut py = min_y;
    while py <= max_y {
        let mut px = min_x;
        while px <= max_x {
            let w0 = edge(x1, y1, x2, y2, px, py);
            let w1 = edge(x2, y2, x0, y0, px, py);
            let w2 = edge(x0, y0, x1, y1, px, py);
            let inside = if positive {
                w0 >= 0 && w1 >= 0 && w2 >= 0
            } else {
                w0 <= 0 && w1 <= 0 && w2 <= 0
            };
            if inside {
                // Barycentric z interpolation. f64 keeps it readable + exact
                // enough at 512x512; the whole ABI stays integer (z is i32).
                let l0 = w0 as f64 / area_f;
                let l1 = w1 as f64 / area_f;
                let l2 = w2 as f64 / area_f;
                let z = (l0 * z0 as f64 + l1 * z1 as f64 + l2 * z2 as f64) as i32;
                // Depth index in i64/saturating — this path computes `di` itself
                // (it doesn't route through set_pixel), so it needs the SAME
                // overflow safety: `vp.ox + px` and `gy*fb_w` would otherwise
                // panic (debug) or wrap on wasm32 (32-bit usize) into the wrong
                // depth slot. Mirror set_pixel.
                let gx = vp.ox as i64 + px as i64;
                let gy = vp.oy as i64 + py as i64;
                if gx >= 0 && gy >= 0 && gx < fb_w as i64 {
                    let di = gy.saturating_mul(fb_w as i64).saturating_add(gx);
                    if di < depth.len() as i64 && z < depth[di as usize] {
                        depth[di as usize] = z;
                        set_pixel(buf, fb_w, vp, px, py, rgb);
                    }
                }
            }
            px += 1;
        }
        py += 1;
    }
}

/// Reset a depth buffer to `far` (the cartridge calls this once per frame
/// before drawing z-tested triangles — it has no arrays/globals of its own).
pub fn clear_depth(depth: &mut [i32], far: i32) {
    for d in depth.iter_mut() {
        *d = far;
    }
}

/// 5x7 bitmap font. Each row's low 5 bits are pixels (bit 4 = leftmost).
/// Covers digits, A-Z, a-z, space, and common punctuation; unknown codes
/// render as a hollow box. Hand-encoded (no font dep).
pub fn glyph_5x7(c: u32) -> [u8; 7] {
    match c {
        0x20 => [0, 0, 0, 0, 0, 0, 0],                       // space
        0x30 => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],  // 0
        0x31 => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],  // 1
        0x32 => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],  // 2
        0x33 => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],  // 3
        0x34 => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],  // 4
        0x35 => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],  // 5
        0x36 => [0x0E, 0x10, 0x10, 0x1E, 0x11, 0x11, 0x0E],  // 6
        0x37 => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],  // 7
        0x38 => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],  // 8
        0x39 => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x01, 0x0E],  // 9
        0x21 => [0x04, 0x04, 0x04, 0x04, 0x04, 0x00, 0x04],  // !
        0x22 => [0x0A, 0x0A, 0x0A, 0x00, 0x00, 0x00, 0x00],  // "
        0x23 => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],  // #
        0x25 => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],  // %
        0x26 => [0x0C, 0x12, 0x14, 0x08, 0x15, 0x12, 0x0D],  // &
        0x27 => [0x04, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00],  // '
        0x28 => [0x04, 0x08, 0x10, 0x10, 0x10, 0x08, 0x04],  // (
        0x29 => [0x04, 0x02, 0x01, 0x01, 0x01, 0x02, 0x04],  // )
        0x2A => [0x00, 0x04, 0x15, 0x0E, 0x15, 0x04, 0x00],  // *
        0x2B => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],  // +
        0x2C => [0x00, 0x00, 0x00, 0x00, 0x06, 0x04, 0x08],  // ,
        0x2D => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],  // -
        0x2E => [0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x06],  // .
        0x2F => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],  // /
        0x3A => [0x00, 0x06, 0x06, 0x00, 0x06, 0x06, 0x00],  // :
        0x3B => [0x00, 0x06, 0x06, 0x00, 0x06, 0x04, 0x08],  // ;
        0x3C => [0x02, 0x04, 0x08, 0x10, 0x08, 0x04, 0x02],  // <
        0x3D => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],  // =
        0x3E => [0x08, 0x04, 0x02, 0x01, 0x02, 0x04, 0x08],  // >
        0x3F => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],  // ?
        0x40 => [0x0E, 0x11, 0x17, 0x15, 0x17, 0x10, 0x0E],  // @
        0x5B => [0x0E, 0x08, 0x08, 0x08, 0x08, 0x08, 0x0E],  // [
        0x5D => [0x0E, 0x02, 0x02, 0x02, 0x02, 0x02, 0x0E],  // ]
        0x5F => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],  // _
        0x41 => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],  // A
        0x42 => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],  // B
        0x43 => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],  // C
        0x44 => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],  // D
        0x45 => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],  // E
        0x46 => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],  // F
        0x47 => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],  // G
        0x48 => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],  // H
        0x49 => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],  // I
        0x4A => [0x07, 0x02, 0x02, 0x02, 0x12, 0x12, 0x0C],  // J
        0x4B => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],  // K
        0x4C => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],  // L
        0x4D => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],  // M
        0x4E => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],  // N
        0x4F => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],  // O
        0x50 => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],  // P
        0x51 => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],  // Q
        0x52 => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],  // R
        0x53 => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],  // S
        0x54 => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],  // T
        0x55 => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],  // U
        0x56 => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],  // V
        0x57 => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],  // W
        0x58 => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],  // X
        0x59 => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],  // Y
        0x5A => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],  // Z
        0x61 => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],  // a
        0x62 => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x1E],  // b
        0x63 => [0x00, 0x00, 0x0E, 0x10, 0x10, 0x11, 0x0E],  // c
        0x64 => [0x01, 0x01, 0x0D, 0x13, 0x11, 0x11, 0x0F],  // d
        0x65 => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],  // e
        0x66 => [0x06, 0x09, 0x08, 0x1C, 0x08, 0x08, 0x08],  // f
        0x67 => [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x0E],  // g
        0x68 => [0x10, 0x10, 0x16, 0x19, 0x11, 0x11, 0x11],  // h
        0x69 => [0x04, 0x00, 0x0C, 0x04, 0x04, 0x04, 0x0E],  // i
        0x6A => [0x02, 0x00, 0x06, 0x02, 0x02, 0x12, 0x0C],  // j
        0x6B => [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],  // k
        0x6C => [0x0C, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],  // l
        0x6D => [0x00, 0x00, 0x1A, 0x15, 0x15, 0x11, 0x11],  // m
        0x6E => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],  // n
        0x6F => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],  // o
        0x70 => [0x00, 0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10],  // p
        0x71 => [0x00, 0x0F, 0x11, 0x11, 0x0F, 0x01, 0x01],  // q
        0x72 => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],  // r
        0x73 => [0x00, 0x00, 0x0F, 0x10, 0x0E, 0x01, 0x1E],  // s
        0x74 => [0x08, 0x08, 0x1C, 0x08, 0x08, 0x09, 0x06],  // t
        0x75 => [0x00, 0x00, 0x11, 0x11, 0x11, 0x13, 0x0D],  // u
        0x76 => [0x00, 0x00, 0x11, 0x11, 0x11, 0x0A, 0x04],  // v
        0x77 => [0x00, 0x00, 0x11, 0x11, 0x15, 0x15, 0x0A],  // w
        0x78 => [0x00, 0x00, 0x11, 0x0A, 0x04, 0x0A, 0x11],  // x
        0x79 => [0x00, 0x11, 0x11, 0x11, 0x0F, 0x01, 0x0E],  // y
        0x7A => [0x00, 0x00, 0x1F, 0x02, 0x04, 0x08, 0x1F],  // z
        _ => [0x1F, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1F],     // unknown -> box
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(w: i32, h: i32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    #[test]
    fn identity_viewport_set_pixel_matches_direct_index() {
        let (w, h) = (256, 144);
        let mut buf = fb(w, h);
        let vp = Viewport::full(w, h);
        set_pixel(&mut buf, w, &vp, 10, 20, (1, 2, 3));
        let idx = ((20 * w + 10) * 4) as usize;
        assert_eq!(&buf[idx..idx + 4], &[1, 2, 3, 255]);
    }

    #[test]
    fn offset_viewport_translates_into_subrect() {
        let (w, h) = (256, 144);
        let mut buf = fb(w, h);
        let vp = Viewport { ox: 100, oy: 50, w: 64, h: 32 };
        set_pixel(&mut buf, w, &vp, 5, 5, (9, 9, 9));
        let idx = (((50 + 5) * w + (100 + 5)) * 4) as usize;
        assert_eq!(&buf[idx..idx + 4], &[9, 9, 9, 255]);
    }

    #[test]
    fn viewport_clips_child_coords_outside_its_bounds() {
        let (w, h) = (256, 144);
        let mut buf = fb(w, h);
        let vp = Viewport { ox: 100, oy: 50, w: 64, h: 32 };
        set_pixel(&mut buf, w, &vp, 64, 0, (9, 9, 9)); // x == vp.w → clipped
        set_pixel(&mut buf, w, &vp, -1, 0, (9, 9, 9)); // x < 0 → clipped
        set_pixel(&mut buf, w, &vp, 0, 32, (9, 9, 9)); // y == vp.h → clipped
        assert!(buf.iter().all(|&b| b == 0), "no pixel should have been written");
    }

    #[test]
    fn full_viewport_clear_fills_entire_framebuffer() {
        let (w, h) = (8, 4);
        let mut buf = fb(w, h);
        clear(&mut buf, w, &Viewport::full(w, h), (5, 5, 5));
        for px in buf.chunks(4) {
            assert_eq!(px, &[5, 5, 5, 255]);
        }
    }

    #[test]
    fn blit_glyph_identity_renders_lit_pixels_in_place() {
        let (w, h) = (32, 16);
        let mut buf = fb(w, h);
        // '-' (0x2D) lights all 5 pixels of row 3 only.
        blit_glyph(&mut buf, w, &Viewport::full(w, h), 0, 0, 0x2D, (9, 9, 9), 1);
        for col in 0..5 {
            let idx = (((3 * w) + col) * 4) as usize;
            assert_eq!(&buf[idx..idx + 4], &[9, 9, 9, 255], "row 3 col {col} lit");
        }
        assert_eq!(&buf[0..4], &[0, 0, 0, 0], "row 0 is blank for '-'");
    }

    #[test]
    fn blit_glyph_offset_viewport_translates_glyph() {
        let (w, h) = (64, 64);
        let mut buf = fb(w, h);
        let vp = Viewport { ox: 10, oy: 20, w: 16, h: 16 };
        blit_glyph(&mut buf, w, &vp, 0, 0, 0x2D, (1, 2, 3), 1);
        let idx = (((23 * w) + 10) * 4) as usize; // glyph row 3 → global y=23, x=10
        assert_eq!(&buf[idx..idx + 4], &[1, 2, 3, 255]);
    }

    #[test]
    fn draw_number_negative_blits_minus_then_digit() {
        let (w, h) = (64, 16);
        let mut buf = fb(w, h);
        draw_number(&mut buf, w, &Viewport::full(w, h), 0, 0, -5, (5, 5, 5), 1);
        let minus = ((3 * w) * 4) as usize; // '-' row 3, col 0
        assert_eq!(&buf[minus..minus + 4], &[5, 5, 5, 255]);
        let five_top = (6 * 4) as usize; // '5' starts at advance=6, row 0 col 6 lit
        assert_eq!(&buf[five_top..five_top + 4], &[5, 5, 5, 255]);
    }

    #[test]
    fn offset_clear_stays_inside_viewport() {
        let (w, h) = (16, 16);
        let mut buf = fb(w, h);
        let vp = Viewport { ox: 4, oy: 4, w: 4, h: 4 };
        clear(&mut buf, w, &vp, (7, 7, 7)); // child clears its whole surface
        let inside = (((4 * w) + 4) * 4) as usize;
        assert_eq!(&buf[inside..inside + 4], &[7, 7, 7, 255]);
        assert_eq!(&buf[0..4], &[0, 0, 0, 0], "origin is outside the viewport");
        let past = (((8 * w) + 8) * 4) as usize;
        assert_eq!(&buf[past..past + 4], &[0, 0, 0, 0], "just past the viewport");
    }

    fn px(buf: &[u8], w: i32, x: i32, y: i32) -> [u8; 4] {
        let idx = ((y * w + x) * 4) as usize;
        [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]]
    }

    #[test]
    fn draw_line_lights_both_endpoints() {
        let (w, h) = (32, 16);
        let mut buf = fb(w, h);
        let vp = Viewport::full(w, h);
        draw_line(&mut buf, w, &vp, 2, 3, 20, 11, (4, 5, 6));
        assert_eq!(px(&buf, w, 2, 3), [4, 5, 6, 255], "first endpoint lit");
        assert_eq!(px(&buf, w, 20, 11), [4, 5, 6, 255], "last endpoint lit");
    }

    #[test]
    fn fill_triangle_fills_interior_and_clips_to_viewport() {
        let (w, h) = (32, 32);
        let mut buf = fb(w, h);
        let vp = Viewport::full(w, h);
        // A right triangle covering the top-left; (4,4) is well inside.
        fill_triangle(&mut buf, w, &vp, 0, 0, 20, 0, 0, 20, (9, 9, 9));
        assert_eq!(px(&buf, w, 4, 4), [9, 9, 9, 255], "interior pixel filled");
        // A point past the hypotenuse stays blank.
        assert_eq!(px(&buf, w, 18, 18), [0, 0, 0, 0], "outside the triangle blank");
    }

    #[test]
    fn extreme_coords_never_panic_or_write_oob() {
        // Cartridge-controlled coords/scales at the i32 extremes must not
        // overflow into a panic (debug) or an OOB write / hang (release/wasm32).
        // Every primitive routes through the now-i64/saturating-bounded set_pixel.
        let (w, h) = (8, 8);
        let mut buf = fb(w, h);
        let full = Viewport::full(w, h);
        // Pathological viewport offsets + a huge fb_w — set_pixel must no-op.
        let evil = Viewport { ox: i32::MAX, oy: i32::MAX, w: i32::MAX, h: i32::MAX };
        set_pixel(&mut buf, w, &evil, 0, 0, (1, 2, 3));
        set_pixel(&mut buf, i32::MAX, &full, 0, 0, (1, 2, 3));
        // A triangle with FULL i32::MIN..i32::MAX vertices on BOTH axes must not
        // overflow edge() (now i128) — its bbox is viewport-clipped so the scan
        // stays bounded.
        fill_triangle(&mut buf, w, &full, i32::MIN, i32::MIN, i32::MAX, i32::MIN, 0, i32::MAX, (7, 8, 9));
        // fill_triangle_z computes its OWN depth index (bypassing set_pixel) — the
        // same extreme coords must not overflow that path either.
        let mut depth = vec![i32::MAX; (w * h) as usize];
        fill_triangle_z(
            &mut buf, &mut depth, w, &full,
            i32::MIN, i32::MIN, 0, i32::MAX, i32::MIN, 1, 0, i32::MAX, 2, (7, 8, 9),
        );
        // A pathological viewport width must not panic the `vp.w - 1` bbox clamp.
        let neg = Viewport { ox: 0, oy: 0, w: i32::MIN, h: i32::MIN };
        fill_triangle(&mut buf, w, &neg, 0, 0, 5, 0, 0, 5, (1, 1, 1));
        // A garbage scale must neither overflow `col*scale` nor hang the
        // `scale x scale` fill loop (clamped to MAX_GLYPH_SCALE).
        blit_glyph(&mut buf, w, &full, 0, 0, 'A' as u32, (1, 1, 1), i32::MAX);
        draw_number(&mut buf, w, &full, 0, 0, -5, (1, 1, 1), i32::MAX);
        // draw_line with i32::MIN..i32::MAX endpoints: clipped to the viewport, so
        // the Bresenham walk is bounded (would otherwise be ~2^32 steps — a hang).
        draw_line(&mut buf, w, &full, i32::MIN, 3, i32::MAX, 4, (4, 5, 6));
        draw_line(&mut buf, w, &full, 3, i32::MIN, 4, i32::MAX, (4, 5, 6));
        // The buffer is exactly w*h*4 bytes — surviving the above (no panic, no
        // OOB) is the assertion; a normal in-bounds draw still works afterwards.
        set_pixel(&mut buf, w, &full, 3, 3, (10, 20, 30));
        assert_eq!(px(&buf, w, 3, 3), [10, 20, 30, 255], "normal draw still works");
    }

    #[test]
    fn clip_line_preserves_inside_clips_outside_rejects_disjoint() {
        // Fully inside → returned unchanged (so draw_line's fast/clip paths agree
        // on in-viewport segments).
        assert_eq!(clip_line(10, 10, 1, 2, 8, 7), Some((1, 2, 8, 7)));
        // Crosses the viewport far-left → far-right: clips to the x bounds, y kept.
        assert_eq!(clip_line(10, 10, -1000, 5, 1000, 5), Some((0, 5, 9, 5)));
        // Entirely above the viewport → None.
        assert_eq!(clip_line(10, 10, 0, -5, 9, -1), None);
        // Extreme endpoints must clip without overflowing (the draw_line DoS case).
        assert!(clip_line(8, 8, i32::MIN, 4, i32::MAX, 4).is_some());
        // Degenerate viewport → None (never a panic).
        assert_eq!(clip_line(0, 0, 1, 1, 2, 2), None);
    }

    #[test]
    fn fill_triangle_winding_agnostic() {
        let (w, h) = (32, 32);
        let mut a = fb(w, h);
        let mut b = fb(w, h);
        let vp = Viewport::full(w, h);
        // Same triangle, opposite vertex orders → identical coverage.
        fill_triangle(&mut a, w, &vp, 0, 0, 20, 0, 0, 20, (1, 2, 3));
        fill_triangle(&mut b, w, &vp, 0, 20, 20, 0, 0, 0, (1, 2, 3));
        assert_eq!(a, b, "CW and CCW windings fill the same pixels");
    }

    #[test]
    fn fill_triangle_clipped_by_offset_viewport() {
        let (w, h) = (64, 64);
        let mut buf = fb(w, h);
        let vp = Viewport { ox: 10, oy: 10, w: 8, h: 8 };
        // Child-local triangle larger than the viewport — must not escape it.
        fill_triangle(&mut buf, w, &vp, 0, 0, 100, 0, 0, 100, (5, 5, 5));
        // A pixel that would be inside the triangle but OUTSIDE the viewport.
        assert_eq!(px(&buf, w, 30, 11), [0, 0, 0, 0], "draw clipped to viewport rect");
        // A pixel inside both the triangle and the viewport is filled.
        assert_eq!(px(&buf, w, 11, 11), [5, 5, 5, 255], "in-viewport interior filled");
    }

    #[test]
    fn fill_triangle_z_nearer_overdraws_farther() {
        let (w, h) = (32, 32);
        let mut buf = fb(w, h);
        let mut depth = vec![i32::MAX; (w * h) as usize];
        let vp = Viewport::full(w, h);
        // Far triangle (z=100) drawn first, then a nearer one (z=10) over the
        // same area: the nearer colour must win at the overlap.
        fill_triangle_z(&mut buf, &mut depth, w, &vp, 0, 0, 100, 20, 0, 100, 0, 20, 100, (1, 1, 1));
        fill_triangle_z(&mut buf, &mut depth, w, &vp, 0, 0, 10, 20, 0, 10, 0, 20, 10, (2, 2, 2));
        assert_eq!(px(&buf, w, 4, 4), [2, 2, 2, 255], "nearer triangle wins the z-test");
    }

    #[test]
    fn fill_triangle_z_farther_does_not_overdraw_nearer() {
        let (w, h) = (32, 32);
        let mut buf = fb(w, h);
        let mut depth = vec![i32::MAX; (w * h) as usize];
        let vp = Viewport::full(w, h);
        // Nearer first (z=10), then farther (z=100): farther must be occluded.
        fill_triangle_z(&mut buf, &mut depth, w, &vp, 0, 0, 10, 20, 0, 10, 0, 20, 10, (2, 2, 2));
        fill_triangle_z(&mut buf, &mut depth, w, &vp, 0, 0, 100, 20, 0, 100, 0, 20, 100, (1, 1, 1));
        assert_eq!(px(&buf, w, 4, 4), [2, 2, 2, 255], "farther triangle is occluded");
    }

    #[test]
    fn clear_depth_resets_every_slot() {
        let mut depth = vec![5i32; 16];
        clear_depth(&mut depth, i32::MAX);
        assert!(depth.iter().all(|&d| d == i32::MAX), "all depth slots reset to far");
    }
}
