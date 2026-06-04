//! Pure, native-testable framebuffer rasterization with a viewport — the
//! geometry foundation for `host::compose` (roadmap Phase 0a, `design/host-
//! compose.md`).
//!
//! A [`Viewport`] offsets and clips a cartridge's draw calls into a sub-
//! rectangle of the shared RGBA framebuffer. The single-cartridge path uses
//! [`Viewport::full`] — an identity transform that reproduces the pre-refactor
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
    let gx = vp.ox + x;
    let gy = vp.oy + y;
    if gx < 0 || gy < 0 || gx >= fb_w {
        return;
    }
    let idx = ((gy as usize) * (fb_w as usize) + gx as usize) * 4;
    if idx + 3 >= buf.len() {
        return;
    }
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
}
