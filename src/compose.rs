//! Compositor scheduling for `host::compose` (roadmap Track A / Phase 1a) —
//! the part that is pure control flow, so it lives here and is native-tested,
//! independent of the wasm `Instance`/`Memory` it will hold in `app::display`.
//!
//! The hazard the adversarial critique flagged as the most likely first crash:
//! a child module's `frame()` issues `spawn`/`close`/`move` on the table WHILE
//! the compositor is iterating it — a re-entrant mutation that double-borrows
//! the live `RefCell` (single-threaded wasm can't deadlock, but it *can* panic
//! the whole tab). The fix is structural: during a tick a child can only queue
//! ops into a separate [`Pending`](crate::compose::Pending) buffer; the table applies them AFTER every
//! module has ticked. The iteration never sees a mid-flight mutation.
//!
//! `H` is the opaque per-module runtime handle (a wasm instance + its memory in
//! `app::display`; a stand-in in tests). The table is generic over it so the
//! scheduling logic carries zero browser dependencies.

use crate::raster::Viewport;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Content-addressed cache for fetched module artifacts (compiled wasm /
/// instances in `app::display`; anything in tests). Keyed by a hash of the
/// WASM BYTES — never by tokenId or name. The critique flagged tokenId-keying
/// as a silent-staleness bug: an on-chain republish (new bytes, same name)
/// would hit a stale entry forever. Content-addressing makes the new bytes a
/// new key, so a republish is a cache miss → a fresh fetch. The on-chain TRUST
/// commitment is keccak256 (the registry capability seam); this LOCAL cache
/// only needs to distinguish different bytes, so a fast std hash suffices.
pub struct WasmCache<V> {
    map: HashMap<u64, V>,
}

impl<V> Default for WasmCache<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V> WasmCache<V> {
    pub fn new() -> Self {
        Self { map: HashMap::new() }
    }

    /// The content key for `bytes` — a hash of the bytes themselves, so
    /// identical bytes share a key and any change produces a different one.
    pub fn content_key(bytes: &[u8]) -> u64 {
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        h.finish()
    }

    pub fn get(&self, key: u64) -> Option<&V> {
        self.map.get(&key)
    }

    pub fn insert(&mut self, key: u64, value: V) {
        self.map.insert(key, value);
    }

    pub fn contains(&self, key: u64) -> bool {
        self.map.contains_key(&key)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// One composited child: its runtime handle and the sub-rectangle it draws to.
pub struct Module<H> {
    pub handle: H,
    pub viewport: Viewport,
}

/// Composite a CHILD framebuffer into a viewport `(x, y, view_w, view_h)` of a
/// PARENT framebuffer. Both buffers are packed `u32` per the worker's pixel
/// convention (`0xAABBGGRR` little-endian — byte order R,G,B,A; see
/// `web/cartridge-worker.js`). The child's native `child_w x child_h` surface is
/// scaled to fill the `view_w x view_h` viewport via **nearest-neighbour**
/// sampling: for each destination pixel the source pixel is
/// `src = (dx * child_w) / view_w`, an integer map that needs no float and works
/// for upscale (2x), downscale (0.5x — source pixels are dropped, never blended),
/// and identity alike. Pixels are copied verbatim (the alpha byte rides along,
/// no blending) so the child's channel order is preserved exactly.
///
/// **Clipping is total and bounds-safe — this function never panics and never
/// indexes out of bounds:**
/// - A negative `x`/`y` (the viewport hangs off the top/left) starts the copy at
///   the first on-screen destination column/row; the corresponding source pixels
///   are skipped, so the child is clipped, not shifted.
/// - A viewport overflowing the right/bottom edge is clamped to `dst_w`/`dst_h`.
/// - A fully off-screen viewport (entirely past any edge, or zero/negative
///   `view_w`/`view_h`, or an empty child) is a no-op.
/// - A `dst`/`child` slice shorter than `dst_w*dst_h` / `child_w*child_h` is
///   tolerated: any index that would fall outside the actual slice is skipped.
///
/// This is the pure pixel half of `host::compose` — after [`ComposeBudget::admit`]
/// gates a child and the host runs that child's `frame()` into its own buffer,
/// `blit_child` is what folds the result into the parent's framebuffer. It is
/// the iframe-free composition primitive: no DOM node, no second canvas, no
/// origin — just one buffer copied into a sub-rectangle of another.
#[allow(clippy::too_many_arguments)] // compositor primitive: parent fb + child fb + viewport rect
pub fn blit_child(
    dst: &mut [u32],
    dst_w: i32,
    dst_h: i32,
    child: &[u32],
    child_w: i32,
    child_h: i32,
    x: i32,
    y: i32,
    view_w: i32,
    view_h: i32,
) {
    // Degenerate inputs → nothing to composite.
    if view_w <= 0 || view_h <= 0 || child_w <= 0 || child_h <= 0 || dst_w <= 0 || dst_h <= 0 {
        return;
    }

    // Clip the destination viewport rect to the framebuffer. `dx0/dy0` are the
    // first on-screen destination columns/rows; `dx1/dy1` are exclusive ends.
    let dx0 = x.max(0);
    let dy0 = y.max(0);
    let dx1 = x.saturating_add(view_w).min(dst_w);
    let dy1 = y.saturating_add(view_h).min(dst_h);
    if dx0 >= dx1 || dy0 >= dy1 {
        return; // entirely off-screen
    }

    let dst_w_us = dst_w as usize;
    let child_w_us = child_w as usize;

    let mut dy = dy0;
    while dy < dy1 {
        // Viewport-local destination row, then nearest-neighbour source row.
        let vy = dy - y; // 0..view_h within the (unclipped) viewport
        let sy = ((vy as i64 * child_h as i64) / view_h as i64) as i32;
        // `sy` is in [0, child_h) for vy in [0, view_h); clamp defensively.
        if sy < 0 || sy >= child_h {
            dy += 1;
            continue;
        }
        let src_row = sy as usize * child_w_us;
        let dst_row = dy as usize * dst_w_us;

        let mut dx = dx0;
        while dx < dx1 {
            let vx = dx - x;
            let sx = ((vx as i64 * child_w as i64) / view_w as i64) as i32;
            if sx >= 0 && sx < child_w {
                let si = src_row + sx as usize;
                let di = dst_row + dx as usize;
                // Slice-length guards: never index past the actual buffers even
                // if they are shorter than w*h would imply.
                if si < child.len() && di < dst.len() {
                    dst[di] = child[si];
                }
            }
            dx += 1;
        }
        dy += 1;
    }
}

/// Map a PARENT-framebuffer pointer `(px, py)` into a composed CHILD's LOCAL
/// coordinate space, given the child's viewport `(x, y, view_w, view_h)` in
/// parent coords and the child's native `child_w x child_h` surface.
///
/// Returns `None` when the pointer is OUTSIDE the viewport (so a composed child
/// only ever "feels" the pointer over its own rect — a click on parent chrome or
/// a sibling never leaks in). Inside the viewport it returns the child-local
/// `(cx, cy)`, inverting the same nearest-neighbour scale [`blit_child`] uses:
/// `cx = ((px - x) * child_w) / view_w`, clamped to `[0, child_w)`. So a child
/// that drew at native resolution `child_w x child_h` and was scaled up/down into
/// the viewport receives pointer coordinates in its OWN space — exactly as if it
/// were running fullscreen at its native size.
///
/// This is what gives a composed child correct input. It is the pure inverse of
/// the blit's forward map and carries no browser dependency; the compositor in
/// `app::display` calls it each frame to fill the focused child's `InputSource::Local`
/// pointer cell (focus-gated so siblings stay isolated).
#[allow(clippy::too_many_arguments)] // compositor primitive: pointer + viewport rect + child dims
pub fn map_pointer_into_child(
    px: i32,
    py: i32,
    x: i32,
    y: i32,
    view_w: i32,
    view_h: i32,
    child_w: i32,
    child_h: i32,
) -> Option<(i32, i32)> {
    if view_w <= 0 || view_h <= 0 || child_w <= 0 || child_h <= 0 {
        return None;
    }
    // Outside the viewport rect → the child sees no pointer.
    if px < x || py < y || px >= x.saturating_add(view_w) || py >= y.saturating_add(view_h) {
        return None;
    }
    let vx = (px - x) as i64;
    let vy = (py - y) as i64;
    // Forward map (blit) is sx = vx*child_w/view_w; the pointer inverse is the
    // same division (each viewport pixel samples one source pixel).
    let cx = ((vx * child_w as i64) / view_w as i64) as i32;
    let cy = ((vy * child_h as i64) / view_h as i64) as i32;
    // Clamp into [0, child_w) x [0, child_h): the rightmost viewport column maps
    // to child_w-1, never child_w (an off-by-one a child could read OOB).
    Some((cx.clamp(0, child_w - 1), cy.clamp(0, child_h - 1)))
}

impl<H> Module<H> {
    /// Composite this child's framebuffer (`child_w x child_h`, packed `u32`)
    /// into this module's [`Viewport`] of a parent framebuffer. The viewport's
    /// `(ox, oy, w, h)` are the destination rect; the child is nearest-neighbour
    /// scaled to fill it (see [`blit_child`]). The single call the compositor
    /// makes per `Ready` child after its `frame()` runs.
    pub fn blit_into(&self, dst: &mut [u32], dst_w: i32, dst_h: i32, child: &[u32], child_w: i32, child_h: i32) {
        blit_child(
            dst, dst_w, dst_h, child, child_w, child_h,
            self.viewport.ox, self.viewport.oy, self.viewport.w, self.viewport.h,
        );
    }

    /// Map a parent-framebuffer pointer into this child's local space given the
    /// child's native dims, or `None` if the pointer is outside this module's
    /// viewport. See [`map_pointer_into_child`].
    pub fn pointer_into(&self, px: i32, py: i32, child_w: i32, child_h: i32) -> Option<(i32, i32)> {
        map_pointer_into_child(
            px, py,
            self.viewport.ox, self.viewport.oy, self.viewport.w, self.viewport.h,
            child_w, child_h,
        )
    }
}

/// Resource caps for a composition — the security gate that stops an
/// attacker-authored or runaway compose graph from exhausting the host (linear
/// memory) or the sponsor (per-mount fees). The adversarial critique flagged
/// ALL three frontier designs as leaving these uncapped (its #2 top risk:
/// "sponsor-key drain… uncapped in all three designs"). Checked when a spawn is
/// requested, BEFORE any fetch/instantiate/settle.
#[derive(Clone, Copy, Debug)]
pub struct ComposeBudget {
    /// Immediate children of ONE node.
    pub max_children: usize,
    pub max_bytes_per_child: usize,
    /// Wasm bytes across the WHOLE tree (every level), not per node.
    pub max_total_bytes: usize,
    /// Deepest spawnable node. Root = depth 0; a node at this depth gets an
    /// inert compose api (its `spawn_module` returns -1) — the recursion stop.
    pub max_depth: usize,
    /// Live nodes across the WHOLE tree — the fork-bomb backstop independent of
    /// the per-node child cap (a balanced tree could otherwise explode).
    pub max_total_nodes: usize,
}

impl ComposeBudget {
    /// v1 caps. Composition is RECURSIVE (the fractal): a child gets its own
    /// table and may spawn grandchildren, bounded by depth + global node/byte
    /// caps. 8 children/node, 16 KB each, 256 KB total, depth 5, 24 nodes total.
    pub fn v1() -> Self {
        Self {
            max_children: 8,
            max_bytes_per_child: 16 * 1024,
            max_total_bytes: 256 * 1024,
            max_depth: 5,
            max_total_nodes: 24,
        }
    }

    /// Whether a new child of `child_bytes` may be admitted given the `count`
    /// children and `total_bytes` already mounted. `Err` carries the reason so
    /// the host can log WHY a spawn was refused (silent caps read as "worked").
    pub fn admit(&self, count: usize, total_bytes: usize, child_bytes: usize) -> Result<(), String> {
        if count >= self.max_children {
            return Err(format!("compose: at the {}-child cap", self.max_children));
        }
        if child_bytes > self.max_bytes_per_child {
            return Err(format!(
                "compose: child is {child_bytes} bytes, over the {}-byte per-child cap",
                self.max_bytes_per_child
            ));
        }
        if total_bytes.saturating_add(child_bytes) > self.max_total_bytes {
            return Err(format!(
                "compose: mounting {child_bytes} more bytes would exceed the {}-byte total cap",
                self.max_total_bytes
            ));
        }
        Ok(())
    }

    /// Whether a node at `parent_depth` may spawn another child given
    /// `total_nodes` already live across the tree — the recursion-specific gate
    /// (depth + global node count) checked at spawn time, before the byte caps
    /// in [`admit`]. `Err` says which cap stopped the fractal.
    pub fn may_spawn(&self, parent_depth: usize, total_nodes: usize) -> Result<(), String> {
        if parent_depth >= self.max_depth {
            return Err(format!("compose: at the depth-{} cap", self.max_depth));
        }
        if total_nodes >= self.max_total_nodes {
            return Err(format!("compose: at the {}-node tree cap", self.max_total_nodes));
        }
        Ok(())
    }
}

/// Tile `n` module viewports across an `fb_w` x `fb_h` framebuffer in a near-
/// square grid (1 -> full screen, 2 -> side-by-side, 3-4 -> 2x2, 5-9 -> 3x3, …).
/// Cells fill left-to-right, top-to-bottom. Integer division can leave a thin
/// remainder strip on the right/bottom edge, which the compositor paints black.
/// Cells never overlap and stay within the framebuffer. Pure + native-tested so
/// the wasm-only `app::display` compositor carries no untested layout math.
pub fn grid_viewports(n: usize, fb_w: i32, fb_h: i32) -> Vec<Viewport> {
    if n == 0 {
        return Vec::new();
    }
    let cols = (n as f64).sqrt().ceil() as i32;
    let rows = (n as i32 + cols - 1) / cols; // ceil(n / cols); cols >= 1
    let (cw, ch) = (fb_w / cols, fb_h / rows);
    (0..n as i32)
        .map(|i| Viewport { ox: (i % cols) * cw, oy: (i / cols) * ch, w: cw, h: ch })
        .collect()
}

/// A deferred-op buffer handed to a module during a tick. A child issues
/// spawn/close/move here; nothing mutates the table until the tick completes.
pub struct Pending<H> {
    ops: Vec<Op<H>>,
}

enum Op<H> {
    Spawn(Module<H>),
    Close(usize),
    SetViewport(usize, Viewport),
}

impl<H> Pending<H> {
    fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Queue a new child module to be added after the tick.
    pub fn spawn(&mut self, handle: H, viewport: Viewport) {
        self.ops.push(Op::Spawn(Module { handle, viewport }));
    }

    /// Queue the removal of the module at `idx` (resolved against the table as
    /// it stands when ops are applied).
    pub fn close(&mut self, idx: usize) {
        self.ops.push(Op::Close(idx));
    }

    /// Queue a viewport change for the module at `idx`.
    pub fn set_viewport(&mut self, idx: usize, viewport: Viewport) {
        self.ops.push(Op::SetViewport(idx, viewport));
    }

    fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

/// The live set of composited child modules + the deferred-mutation discipline.
pub struct ModuleTable<H> {
    modules: Vec<Module<H>>,
}

impl<H> Default for ModuleTable<H> {
    fn default() -> Self {
        Self::new()
    }
}

impl<H> ModuleTable<H> {
    pub fn new() -> Self {
        Self { modules: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Add a module immediately (use outside a tick — e.g. the initial layout).
    pub fn push(&mut self, handle: H, viewport: Viewport) -> usize {
        self.modules.push(Module { handle, viewport });
        self.modules.len() - 1
    }

    /// Tick every module in order. `f` receives each module's handle + viewport
    /// and a [`Pending`] buffer to issue spawn/close/move on. Those mutations
    /// are applied only after the whole pass, so a child mutating the table
    /// during its own frame cannot invalidate the in-progress iteration.
    pub fn tick(&mut self, mut f: impl FnMut(usize, &H, &Viewport, &mut Pending<H>)) {
        let mut pending = Pending::new();
        for (i, m) in self.modules.iter().enumerate() {
            f(i, &m.handle, &m.viewport, &mut pending);
        }
        if !pending.is_empty() {
            self.apply(pending);
        }
    }

    /// The topmost module whose viewport contains global point `(x, y)`, with
    /// the pointer translated to that module's LOCAL coords. Last-pushed =
    /// topmost (z-order). Pointer events route only to the focused child
    /// (roadmap Phase 1c) so a click in one panel can't drive a sibling.
    pub fn focus_at(&self, x: i32, y: i32) -> Option<(usize, i32, i32)> {
        for i in (0..self.modules.len()).rev() {
            let vp = &self.modules[i].viewport;
            if x >= vp.ox && y >= vp.oy && x < vp.ox + vp.w && y < vp.oy + vp.h {
                return Some((i, x - vp.ox, y - vp.oy));
            }
        }
        None
    }

    fn apply(&mut self, pending: Pending<H>) {
        // Spawns and viewport sets first (stable indices), then closes in
        // DESCENDING index order so each removal can't shift a later one.
        let mut closes = Vec::new();
        for op in pending.ops {
            match op {
                Op::Spawn(m) => self.modules.push(m),
                Op::SetViewport(i, vp) => {
                    if let Some(m) = self.modules.get_mut(i) {
                        m.viewport = vp;
                    }
                }
                Op::Close(i) => closes.push(i),
            }
        }
        closes.sort_unstable();
        closes.dedup();
        for i in closes.into_iter().rev() {
            if i < self.modules.len() {
                self.modules.remove(i);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vp() -> Viewport {
        Viewport::full(256, 144)
    }

    #[test]
    fn push_adds_immediately() {
        let mut t: ModuleTable<&str> = ModuleTable::new();
        assert!(t.is_empty());
        let i = t.push("a", vp());
        assert_eq!(i, 0);
        assert_eq!(t.len(), 1);
    }

    #[test]
    fn tick_visits_every_module_with_its_index() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(10, vp());
        t.push(20, vp());
        let mut seen = Vec::new();
        t.tick(|i, h, _vp, _p| seen.push((i, *h)));
        assert_eq!(seen, vec![(0, 10), (1, 20)]);
    }

    #[test]
    fn spawn_during_tick_is_deferred_then_applied() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(1, vp());
        // Module 0's frame() spawns a child. The table must NOT grow mid-tick
        // (that's the double-borrow crash), and the child appears after.
        let mut len_seen_during = None;
        t.tick(|_i, _h, _vp, p| {
            // (we can't read t.len() here — that's the whole point — but the
            // iteration is over a snapshot of 1, so f runs exactly once)
            len_seen_during = Some(true);
            p.spawn(2, vp());
        });
        assert_eq!(len_seen_during, Some(true));
        assert_eq!(t.len(), 2, "spawned child applied after the tick");
    }

    #[test]
    fn tick_runs_once_per_preexisting_module_not_for_spawned() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(1, vp());
        let mut ticks = 0;
        t.tick(|_i, _h, _vp, p| {
            ticks += 1;
            p.spawn(99, vp()); // each spawn must NOT be ticked this pass
        });
        assert_eq!(ticks, 1, "only the pre-existing module ticked");
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn close_during_tick_applies_descending_so_indices_stay_valid() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(0, vp());
        t.push(1, vp());
        t.push(2, vp());
        // Close 0 and 2 during the tick; descending-order apply keeps it sound.
        t.tick(|i, _h, _vp, p| {
            if i == 0 || i == 2 {
                p.close(i);
            }
        });
        assert_eq!(t.len(), 1, "modules 0 and 2 removed, 1 remains");
        let mut left = None;
        t.tick(|_i, h, _vp, _p| left = Some(*h));
        assert_eq!(left, Some(1));
    }

    #[test]
    fn compose_budget_admits_within_caps_and_refuses_past_them() {
        let b = ComposeBudget::v1();
        // Within all caps.
        assert!(b.admit(0, 0, 1024).is_ok());
        assert!(b.admit(7, 1024, 1024).is_ok()); // last allowed child
        // Too many children.
        assert!(b.admit(8, 0, 1).is_err());
        // Child too big.
        assert!(b.admit(0, 0, 16 * 1024 + 1).is_err());
        // Total would overflow the aggregate cap (256 KB).
        assert!(b.admit(1, 250 * 1024, 8 * 1024).is_err());
        assert!(b.admit(1, 200 * 1024, 8 * 1024).is_ok()); // still room under 256 KB
        // saturating_add can't be tricked into wrapping past the cap.
        assert!(b.admit(0, usize::MAX, 1).is_err());
    }

    #[test]
    fn compose_budget_may_spawn_gates_depth_and_tree_node_count() {
        let b = ComposeBudget::v1();
        // A shallow node with room to grow may spawn.
        assert!(b.may_spawn(0, 0).is_ok());
        assert!(b.may_spawn(4, 23).is_ok()); // depth 4 child→5 (ok), 23 nodes (last slot)
        // A node AT the depth cap cannot spawn (its child would be depth 6).
        assert!(b.may_spawn(5, 0).is_err());
        // The global tree-node cap stops a wide fractal even when shallow.
        assert!(b.may_spawn(1, 24).is_err());
    }

    #[test]
    fn focus_at_routes_to_containing_module_in_local_coords() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(0, Viewport { ox: 0, oy: 0, w: 100, h: 100 });
        t.push(1, Viewport { ox: 100, oy: 50, w: 64, h: 32 });
        // Inside module 1 → its index + pointer translated to local coords.
        assert_eq!(t.focus_at(110, 60), Some((1, 10, 10)));
        // Inside module 0 only.
        assert_eq!(t.focus_at(5, 5), Some((0, 5, 5)));
        // Outside every viewport.
        assert_eq!(t.focus_at(200, 200), None);
    }

    #[test]
    fn focus_at_picks_topmost_on_overlap() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(0, Viewport { ox: 0, oy: 0, w: 100, h: 100 });
        t.push(1, Viewport { ox: 0, oy: 0, w: 100, h: 100 }); // same rect, on top
        // Last-pushed (index 1) wins the click.
        assert_eq!(t.focus_at(10, 10), Some((1, 10, 10)));
    }

    #[test]
    fn cache_content_key_is_deterministic_and_byte_sensitive() {
        let a = WasmCache::<()>::content_key(b"abc");
        assert_eq!(a, WasmCache::<()>::content_key(b"abc"));
        assert_ne!(a, WasmCache::<()>::content_key(b"abd"));
        assert_ne!(a, WasmCache::<()>::content_key(b""));
    }

    #[test]
    fn republish_changes_the_key_so_no_stale_hit() {
        // The whole point: same name/tokenId, new bytes (a republish) → a new
        // content key → cache MISS → fresh fetch. A tokenId-keyed cache would
        // have served the stale v1 forever.
        let mut cache: WasmCache<&str> = WasmCache::new();
        let k1 = WasmCache::<&str>::content_key(b"app-wasm-v1");
        cache.insert(k1, "compiled-v1");
        assert!(cache.contains(k1));

        let k2 = WasmCache::<&str>::content_key(b"app-wasm-v2");
        assert_ne!(k1, k2);
        assert!(cache.get(k2).is_none(), "republished bytes must not hit the v1 entry");
        assert_eq!(cache.get(k1), Some(&"compiled-v1"), "the v1 bytes still resolve to v1");
    }

    #[test]
    fn set_viewport_during_tick_is_deferred() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(7, Viewport::full(256, 144));
        t.tick(|i, _h, _vp, p| p.set_viewport(i, Viewport { ox: 10, oy: 20, w: 64, h: 32 }));
        let mut got = None;
        t.tick(|_i, _h, v, _p| got = Some(*v));
        assert_eq!(got, Some(Viewport { ox: 10, oy: 20, w: 64, h: 32 }));
    }

    // ── blit_child + map_pointer_into_child ────────────────────────────────

    /// A `w x h` parent framebuffer, all zero (transparent black).
    fn pfb(w: i32, h: i32) -> Vec<u32> {
        vec![0u32; (w * h) as usize]
    }

    /// A `w x h` child framebuffer filled with a single packed colour.
    fn cfb(w: i32, h: i32, color: u32) -> Vec<u32> {
        vec![color; (w * h) as usize]
    }

    fn at(buf: &[u32], w: i32, x: i32, y: i32) -> u32 {
        buf[(y * w + x) as usize]
    }

    #[test]
    fn blit_identity_copies_child_pixel_for_pixel() {
        let (w, h) = (16, 16);
        let mut dst = pfb(w, h);
        // 4x4 child with a distinct value per pixel so a mis-map is visible.
        let cw = 4;
        let ch = 4;
        let child: Vec<u32> = (0..(cw * ch) as u32).collect();
        // Identity scale (view == child dims) at offset (2,3).
        blit_child(&mut dst, w, h, &child, cw, ch, 2, 3, cw, ch);
        for cy in 0..ch {
            for cx in 0..cw {
                let want = (cy * cw + cx) as u32;
                assert_eq!(at(&dst, w, 2 + cx, 3 + cy), want, "child ({cx},{cy}) lands at parent ({},{})", 2 + cx, 3 + cy);
            }
        }
        // The pixel just outside the blit rect is untouched.
        assert_eq!(at(&dst, w, 1, 3), 0);
        assert_eq!(at(&dst, w, 2 + cw, 3), 0);
    }

    #[test]
    fn blit_preserves_rgba_channel_order() {
        // 0xAABBGGRR packed: a known colour must survive the copy byte-for-byte.
        let color = 0x11_22_33_44u32; // A=0x11 B=0x22 G=0x33 R=0x44
        let (w, h) = (8, 8);
        let mut dst = pfb(w, h);
        let child = cfb(2, 2, color);
        blit_child(&mut dst, w, h, &child, 2, 2, 0, 0, 2, 2);
        assert_eq!(at(&dst, w, 0, 0), color, "packed colour preserved exactly");
        assert_eq!(at(&dst, w, 1, 1), color);
    }

    #[test]
    fn blit_scales_2x_nearest_neighbour() {
        // 2x2 child → 4x4 viewport. Each source pixel becomes a 2x2 block.
        let cw = 2;
        let ch = 2;
        // child: [A B / C D]
        let child = vec![10u32, 20, 30, 40];
        let (w, h) = (8, 8);
        let mut dst = pfb(w, h);
        blit_child(&mut dst, w, h, &child, cw, ch, 0, 0, 4, 4);
        // Top-left 2x2 block == A(10), top-right == B(20), etc.
        assert_eq!(at(&dst, w, 0, 0), 10);
        assert_eq!(at(&dst, w, 1, 1), 10, "A occupies the whole top-left 2x2");
        assert_eq!(at(&dst, w, 2, 0), 20);
        assert_eq!(at(&dst, w, 3, 1), 20);
        assert_eq!(at(&dst, w, 0, 2), 30);
        assert_eq!(at(&dst, w, 2, 2), 40);
        assert_eq!(at(&dst, w, 3, 3), 40);
    }

    #[test]
    fn blit_scales_half_drops_source_pixels() {
        // 4x4 child → 2x2 viewport: nearest-neighbour picks source (0,0),(2,0),
        // (0,2),(2,2) — odd rows/cols are dropped, never blended.
        let cw = 4;
        let ch = 4;
        let child: Vec<u32> = (0..(cw * ch) as u32).collect(); // value == cy*4+cx
        let (w, h) = (8, 8);
        let mut dst = pfb(w, h);
        blit_child(&mut dst, w, h, &child, cw, ch, 0, 0, 2, 2);
        // dst(0,0)→src(0,0)=0; dst(1,0)→src(2,0)=2; dst(0,1)→src(0,2)=8; dst(1,1)→src(2,2)=10.
        assert_eq!(at(&dst, w, 0, 0), 0);
        assert_eq!(at(&dst, w, 1, 0), 2);
        assert_eq!(at(&dst, w, 0, 1), 8);
        assert_eq!(at(&dst, w, 1, 1), 10);
    }

    #[test]
    fn blit_clips_at_right_and_bottom_edges() {
        // Viewport runs off the right + bottom; only the on-screen part is drawn.
        let (w, h) = (4, 4);
        let mut dst = pfb(w, h);
        let child = cfb(4, 4, 7);
        // Place a 4x4 identity blit at (2,2): only the 2x2 bottom-right corner fits.
        blit_child(&mut dst, w, h, &child, 4, 4, 2, 2, 4, 4);
        assert_eq!(at(&dst, w, 2, 2), 7);
        assert_eq!(at(&dst, w, 3, 3), 7);
        assert_eq!(at(&dst, w, 0, 0), 0, "top-left untouched");
        assert_eq!(at(&dst, w, 1, 1), 0);
    }

    #[test]
    fn blit_clips_at_left_and_top_edges_without_shifting() {
        // Negative offset: the child is clipped (left/top columns dropped), NOT
        // shifted right. A 4x4 identity child at (-2,-2) shows its bottom-right.
        let (w, h) = (4, 4);
        let mut dst = pfb(w, h);
        let child: Vec<u32> = (0..16u32).collect(); // value == cy*4+cx
        blit_child(&mut dst, w, h, &child, 4, 4, -2, -2, 4, 4);
        // dst(0,0) shows child(2,2)=10 (the first on-screen source pixel).
        assert_eq!(at(&dst, w, 0, 0), 10);
        assert_eq!(at(&dst, w, 1, 1), 15);
        // Nothing wrote past the clipped region's natural extent.
        assert_eq!(at(&dst, w, 2, 2), 0);
    }

    #[test]
    fn blit_fully_offscreen_is_a_noop() {
        let (w, h) = (4, 4);
        let child = cfb(2, 2, 9);
        // Past the right edge.
        let mut dst = pfb(w, h);
        blit_child(&mut dst, w, h, &child, 2, 2, 4, 0, 2, 2);
        assert!(dst.iter().all(|&p| p == 0), "off the right edge writes nothing");
        // Past the bottom edge.
        let mut dst = pfb(w, h);
        blit_child(&mut dst, w, h, &child, 2, 2, 0, 4, 2, 2);
        assert!(dst.iter().all(|&p| p == 0));
        // Entirely off the left/top.
        let mut dst = pfb(w, h);
        blit_child(&mut dst, w, h, &child, 2, 2, -2, 0, 2, 2);
        assert!(dst.iter().all(|&p| p == 0), "viewport ends at x=0 → nothing on-screen");
        let mut dst = pfb(w, h);
        blit_child(&mut dst, w, h, &child, 2, 2, 0, -2, 2, 2);
        assert!(dst.iter().all(|&p| p == 0));
    }

    #[test]
    fn blit_degenerate_inputs_are_noops() {
        let (w, h) = (4, 4);
        let child = cfb(2, 2, 9);
        let mut dst = pfb(w, h);
        // Zero/negative viewport dims.
        blit_child(&mut dst, w, h, &child, 2, 2, 0, 0, 0, 4);
        blit_child(&mut dst, w, h, &child, 2, 2, 0, 0, 4, 0);
        blit_child(&mut dst, w, h, &child, 2, 2, 0, 0, -1, 4);
        // Empty child.
        blit_child(&mut dst, w, h, &[], 0, 0, 0, 0, 4, 4);
        assert!(dst.iter().all(|&p| p == 0), "no degenerate call writes anything");
    }

    #[test]
    fn blit_tolerates_short_slices_without_panicking() {
        // dst shorter than dst_w*dst_h, and child shorter than child_w*child_h:
        // any index past the real slice is skipped — must not panic.
        let mut dst = vec![0u32; 4]; // claims 8x8 but only 4 long
        let child = vec![5u32; 2]; // claims 4x4 but only 2 long
        blit_child(&mut dst, 8, 8, &child, 4, 4, 0, 0, 4, 4);
        // Whatever it wrote, it stayed inside the 4-element dst slice.
        assert_eq!(dst.len(), 4);
    }

    #[test]
    fn module_blit_into_uses_its_viewport() {
        let m = Module { handle: (), viewport: Viewport { ox: 3, oy: 1, w: 2, h: 2 } };
        let (w, h) = (8, 8);
        let mut dst = pfb(w, h);
        let child = cfb(2, 2, 0xABCD_1234);
        m.blit_into(&mut dst, w, h, &child, 2, 2);
        assert_eq!(at(&dst, w, 3, 1), 0xABCD_1234);
        assert_eq!(at(&dst, w, 4, 2), 0xABCD_1234);
        assert_eq!(at(&dst, w, 0, 0), 0, "outside the viewport untouched");
    }

    #[test]
    fn pointer_inside_viewport_maps_to_child_local() {
        // Viewport (10,20,64,32), child native 64x32 → identity scale. A pointer
        // at parent (60,45) is viewport-local (50,25) → child-local (50,25).
        let got = map_pointer_into_child(60, 45, 10, 20, 64, 32, 64, 32);
        assert_eq!(got, Some((50, 25)));
    }

    #[test]
    fn pointer_outside_viewport_is_none() {
        // Left, top, right, bottom of the viewport (10,20,64,32).
        assert_eq!(map_pointer_into_child(9, 30, 10, 20, 64, 32, 64, 32), None, "left of viewport");
        assert_eq!(map_pointer_into_child(30, 19, 10, 20, 64, 32, 64, 32), None, "above viewport");
        assert_eq!(map_pointer_into_child(74, 30, 10, 20, 64, 32, 64, 32), None, "ox+w is exclusive");
        assert_eq!(map_pointer_into_child(30, 52, 10, 20, 64, 32, 64, 32), None, "oy+h is exclusive");
    }

    #[test]
    fn pointer_scale_2x_halves_into_child_space() {
        // Child native 32x16 scaled into a 64x32 viewport at origin (0,0).
        // A pointer at viewport (40,20) maps to child (40*32/64, 20*16/32)=(20,10).
        assert_eq!(map_pointer_into_child(40, 20, 0, 0, 64, 32, 32, 16), Some((20, 10)));
        // Top-left corner maps to (0,0).
        assert_eq!(map_pointer_into_child(0, 0, 0, 0, 64, 32, 32, 16), Some((0, 0)));
    }

    #[test]
    fn pointer_rightmost_column_clamps_inside_child() {
        // The last viewport column/row must map to child_w-1 / child_h-1, never
        // child_w/child_h (which a child would read OOB).
        let got = map_pointer_into_child(63, 31, 0, 0, 64, 32, 64, 32);
        assert_eq!(got, Some((63, 31)));
        // Downscale edge: 4x view of a 1-wide child still clamps to 0.
        let got = map_pointer_into_child(3, 0, 0, 0, 4, 4, 1, 1);
        assert_eq!(got, Some((0, 0)));
    }

    #[test]
    fn pointer_forward_blit_roundtrip_agrees() {
        // The pointer map is the inverse of the blit's forward sample: a pointer
        // over destination pixel (dx,dy) must select the SAME source pixel the
        // blit copied there. Check across a 3x scale.
        let (cw, ch) = (5, 3);
        let (vw, vh) = (15, 9);
        let (ox, oy) = (7, 11);
        for dy in 0..vh {
            for dx in 0..vw {
                let mapped = map_pointer_into_child(ox + dx, oy + dy, ox, oy, vw, vh, cw, ch).unwrap();
                let blit_sx = ((dx as i64 * cw as i64) / vw as i64) as i32;
                let blit_sy = ((dy as i64 * ch as i64) / vh as i64) as i32;
                assert_eq!(mapped, (blit_sx, blit_sy), "pointer at dst ({dx},{dy}) must select blit's source pixel");
            }
        }
    }

    #[test]
    fn module_pointer_into_uses_its_viewport() {
        let m = Module { handle: (), viewport: Viewport { ox: 100, oy: 50, w: 64, h: 32 } };
        assert_eq!(m.pointer_into(110, 60, 64, 32), Some((10, 10)));
        assert_eq!(m.pointer_into(10, 10, 64, 32), None, "pointer over parent chrome → child sees nothing");
    }

    #[test]
    fn grid_one_module_is_the_full_framebuffer() {
        assert_eq!(grid_viewports(1, 256, 144), vec![Viewport { ox: 0, oy: 0, w: 256, h: 144 }]);
    }

    #[test]
    fn grid_two_modules_split_side_by_side_without_overlap() {
        let v = grid_viewports(2, 256, 144);
        assert_eq!(v, vec![
            Viewport { ox: 0, oy: 0, w: 128, h: 144 },
            Viewport { ox: 128, oy: 0, w: 128, h: 144 },
        ]);
        assert!(v[0].ox + v[0].w <= v[1].ox, "left cell ends before the right begins");
    }

    #[test]
    fn grid_four_modules_are_a_2x2() {
        let v = grid_viewports(4, 256, 144); // cols=2, rows=2, cw=128, ch=72
        assert_eq!(v.len(), 4);
        assert_eq!(v[0], Viewport { ox: 0, oy: 0, w: 128, h: 72 });
        assert_eq!(v[3], Viewport { ox: 128, oy: 72, w: 128, h: 72 });
    }

    #[test]
    fn grid_cells_stay_in_bounds_and_zero_is_empty() {
        assert!(grid_viewports(0, 256, 144).is_empty());
        for n in 1..=9 {
            for vp in grid_viewports(n, 256, 144) {
                assert!(vp.ox >= 0 && vp.oy >= 0);
                assert!(vp.ox + vp.w <= 256 && vp.oy + vp.h <= 144, "cell {vp:?} escapes the framebuffer for n={n}");
            }
        }
    }
}
