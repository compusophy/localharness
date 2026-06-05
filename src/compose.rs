//! Compositor scheduling for `host::compose` (roadmap Track A / Phase 1a) —
//! the part that is pure control flow, so it lives here and is native-tested,
//! independent of the wasm `Instance`/`Memory` it will hold in `app::display`.
//!
//! The hazard the adversarial critique flagged as the most likely first crash:
//! a child module's `frame()` issues `spawn`/`close`/`move` on the table WHILE
//! the compositor is iterating it — a re-entrant mutation that double-borrows
//! the live `RefCell` (single-threaded wasm can't deadlock, but it *can* panic
//! the whole tab). The fix is structural: during a tick a child can only queue
//! ops into a separate [`Pending`] buffer; the table applies them AFTER every
//! module has ticked. The iteration never sees a mid-flight mutation.
//!
//! `H` is the opaque per-module runtime handle (a wasm instance + its memory in
//! `app::display`; a stand-in in tests). The table is generic over it so the
//! scheduling logic carries zero browser dependencies.

use crate::raster::Viewport;

/// One composited child: its runtime handle and the sub-rectangle it draws to.
pub struct Module<H> {
    pub handle: H,
    pub viewport: Viewport,
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
    fn set_viewport_during_tick_is_deferred() {
        let mut t: ModuleTable<i32> = ModuleTable::new();
        t.push(7, Viewport::full(256, 144));
        t.tick(|i, _h, _vp, p| p.set_viewport(i, Viewport { ox: 10, oy: 20, w: 64, h: 32 }));
        let mut got = None;
        t.tick(|_i, _h, v, _p| got = Some(*v));
        assert_eq!(got, Some(Viewport { ox: 10, oy: 20, w: 64, h: 32 }));
    }
}
