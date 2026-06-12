# host::compose — in-framebuffer subdomain composition

> **STATUS: pure core LANDED; wiring open.** The single-cartridge framebuffer
> runtime (Web Worker + watchdog) shipped, the iframe-based `?compose=` it
> replaces is gone, and the **pure compositor core** now lives native-tested in
> `src/compose.rs`: the cache (`WasmCache`), the admission budget
> (`ComposeBudget`), the deferred-mutation module table (`ModuleTable` /
> `Pending`), the grid layout (`grid_viewports`), and — new this cut — the
> **framebuffer blit** (`blit_child`) + **pointer map** (`map_pointer_into_child`)
> that composite a child surface into a parent rect and route input into it. What
> remains is the wasm-only wiring: the rustlite `host::compose` import and the
> `app::display` compositor calling these pure functions per child. This is also
> where the open **cartridge host-import / rich-context-cartridge** frontier
> lives.
>
> **What THIS cut delivers (the pure core, `src/compose.rs`):**
> - `blit_child(dst, dst_w, dst_h, child, child_w, child_h, x, y, view_w, view_h)`
>   — composite a packed-`u32` child framebuffer into a viewport of a packed-`u32`
>   parent framebuffer; nearest-neighbour integer scale; total edge clipping;
>   bounds-safe (never panics / never indexes OOB). Mirrored as
>   `Module::blit_into`.
> - `map_pointer_into_child(px, py, x, y, view_w, view_h, child_w, child_h) ->
>   Option<(cx, cy)>` — map a parent pointer into a child's local space, `None`
>   when outside the viewport; the exact inverse of the blit's forward sample.
>   Mirrored as `Module::pointer_into`.
> - 31 native unit tests covering scale (identity / 2x / 0.5x), four-edge + fully-
>   offscreen clipping, RGBA channel-order preservation, short-slice safety, the
>   pointer in/out-of-viewport gate, scale mapping, rightmost-column clamp, and a
>   blit↔pointer roundtrip agreement.
>
> **What's DEFERRED to the wiring follow-up (wasm-only, `src/app/` + rustlite):**
> the `host::compose` rustlite import (spawn/status/move/focus/close), the
> `app::display` compositor pass that fetches a child's `app.wasm`, runs its
> `frame()` into its own buffer, calls `blit_child` to fold it into the shared
> framebuffer, and fills the focused child's `InputSource::Local` pointer cell via
> `map_pointer_into_child`. None of that is in this cut.

Status: design. Replaces the iframe-based `?compose=` (`src/app/compose.rs` +
`src/app/embed.rs`). No iframes, no DOM, one canvas.

## Why

The current `?compose=foo,bar` path renders a grid of `<iframe
src="foo.localharness.xyz/?embed=1">` elements. That violates two hard project
rules — **no iframes** and **no imperative DOM / framework composition** — and it
is dead on mobile (the embedded origin's OPFS is partitioned, same failure class
as the signer iframe). It also means each "module" is a whole second browsing
context with its own wallet/signer, not a lightweight UI widget the host app can
place anywhere on its surface.

The DISPLAY framebuffer (`src/app/display.rs`) already proves the alternative: a
wasm cartridge draws *pixels* into a host-owned RGBA buffer through the
`host_display` import ABI; it never touches the canvas or the DOM. `host_net`
proves a second precedent — a poll-model host service handed to the cartridge as
integer-only imports.

`host::compose` extends that model **one level up**: a *compositor* cartridge can
spawn other subdomains' `app.wasm` as **child cartridges**, each bound to a
sub-rectangle of the same framebuffer, each its own isolated wasm `Instance` +
`Memory`. A module is now a wasm program drawing pixels into a clipped rect — not
an origin, not an iframe, not a DOM node. Composition happens entirely inside the
one canvas the host already owns.

This is the Orbital analogy completed: DISPLAY is the display server, a cartridge
is a client app, and `host::compose` is the **window manager** — it places client
surfaces on the screen and routes input to the focused one.

## Model overview

```
                 ┌──────────────────────────────────────────┐
                 │ #display-canvas  (256 x 144 framebuffer)  │
                 │                                           │
   parent  ───►  │   parent draws chrome / background        │
   cartridge     │   ┌────────────┐      ┌────────────┐      │
   (claude)      │   │ child rect │      │ child rect │      │
                 │   │ bitmask    │      │ pong        │      │
                 │   │ (own Inst, │      │ (own Inst,  │      │
                 │   │  own Mem)  │      │  own Mem)   │      │
                 │   └────────────┘      └────────────┘      │
                 └──────────────────────────────────────────┘
```

- The **parent** cartridge runs exactly as today: `frame(t)` driven by the rAF
  loop in `display.rs`. It owns the whole framebuffer.
- The parent calls `compose::spawn_module(name, x, y, w, h)` to mount a child.
  The host fetches that subdomain's on-chain `app.wasm`, instantiates it in its
  **own** `WebAssembly.Instance` with its **own** `Memory`, and binds it to the
  rect `(x, y, w, h)`.
- Each frame, after the parent's `frame(t)` returns, the host ticks every live
  child's `frame(t)` (or one-shot `render()`), giving each child a **virtual
  display** whose origin and bounds are its rect: a child that calls
  `display::set_pixel(0,0,...)` writes the top-left pixel **of its rect**, clipped
  to the rect, never able to scribble outside it or over a sibling.
- Pointer events inside a child's rect are translated to rect-local coordinates
  and exposed to that child via the same `pointer_x/pointer_y/pointer_down`
  imports it already knows; events outside its rect read as "no pointer".

The parent is the window manager: it decides the layout (rects), z-order (spawn
order), and which child is focused. There is one shared host framebuffer; the
parent's draws and every child's draws all land in it, then a single `present()`
flips it to the canvas.

## 0. The pure compositor core (`src/compose.rs`) — LANDED

Everything below the ABI that is *pure control flow + pixel math* lives in
`src/compose.rs`, native-tested, carrying zero browser dependency. The wasm-only
`app::display` compositor is a thin shell over it.

### Framebuffer model

Both parent and child framebuffers are **packed `u32`** in the worker's
convention: `0xAABBGGRR` little-endian (byte order R, G, B, A — see
`web/cartridge-worker.js`, which fills a `Uint32Array`). A child renders into its
own `child_w x child_h` buffer (its native resolution); the host folds that buffer
into the parent's framebuffer at the child's viewport.

### `blit_child` — the composite primitive

```text
blit_child(dst, dst_w, dst_h, child, child_w, child_h, x, y, view_w, view_h)
```

Composites the `child_w x child_h` child surface into the rect `(x, y, view_w,
view_h)` of the parent `dst`.

- **Sampling.** Nearest-neighbour, integer-only. For each destination pixel the
  source is `src = (dst_local * child_dim) / view_dim` per axis. This handles
  upscale (2x → each source pixel becomes a block), downscale (0.5x → odd
  rows/cols are dropped, never blended), and identity uniformly. Pixels are
  copied verbatim — the alpha byte rides along, channel order preserved, no
  alpha-blending in v1 (a composited child is opaque within its rect).
- **Clipping.** Total and bounds-safe. Negative `x`/`y` clip the child (the
  off-screen source columns/rows are skipped — the child is clipped, not shifted);
  right/bottom overflow is clamped to `dst_w`/`dst_h`; a fully off-screen or
  degenerate (`view_w<=0`, empty child, …) call is a no-op. A `dst`/`child` slice
  shorter than its declared `w*h` is tolerated — any index past the real slice is
  skipped. **The function never panics and never indexes out of bounds.**
- `Module::blit_into(dst, dst_w, dst_h, child, child_w, child_h)` is the
  convenience that passes the module's own `Viewport` as the rect.

### `map_pointer_into_child` — the input primitive

```text
map_pointer_into_child(px, py, x, y, view_w, view_h, child_w, child_h)
        -> Option<(cx, cy)>
```

Maps a parent-framebuffer pointer into a child's LOCAL space. Returns `None` when
the pointer is outside the viewport rect (`ox+w` / `oy+h` exclusive) — so a
composed child only ever feels the pointer over its own rect; a click on parent
chrome or a sibling never leaks in. Inside, it inverts the blit's forward sample
(`cx = ((px-x)*child_w)/view_w`) and clamps into `[0, child_w) x [0, child_h)` so
the rightmost column maps to `child_w-1`, never `child_w` (an OOB a child could
read). It is the exact inverse of `blit_child`'s scale, proven by the
`pointer_forward_blit_roundtrip_agrees` test. `Module::pointer_into(px, py,
child_w, child_h)` uses the module's viewport.

These two are what give a composed child a correct, isolated **virtual display
clipped to its rect** and **local, focus-gated pointer** — the pixel + input core
of the ABI specified next.

## 1. The `host::compose` ABI

A new import module `host_compose`, wired alongside `host_display` / `host_net` in
`display.rs::build_host_display` (and in `rustlite/loader.rs::build_host_imports`).
Integer-only, poll-model, strings passed as length-prefixed pointers into the
**parent's** linear memory — identical conventions to `host_net`.

```text
host::compose                           (rustlite `use host::compose;`)
──────────────────────────────────────────────────────────────────────
spawn_module(name_ptr, x, y, w, h) -> handle
        Fetch <name>.app.wasm on-chain, instantiate a child bound to the
        rect (x,y,w,h) in PARENT framebuffer coords. `name_ptr` is a
        length-prefixed string in the parent's memory. Returns a handle
        >= 0, or a negative error/pending code (see status()). Async fetch:
        the call returns immediately with a handle in the LOADING state;
        the child starts ticking once its bytes arrive.

status(handle) -> i32
        -1 bad handle
         0 LOADING   (fetch / instantiate in flight)
         1 READY     (ticking each frame)
         2 FAILED    (no app.wasm, bad bytes, or trap budget exceeded)

move_module(handle, x, y, w, h)
        Re-bind the child's rect (the window manager moves/resizes a
        window). No-op on a bad handle.

focus_module(handle)
        Make `handle` the focused child — the one that receives pointer
        input and (later) keyboard. Pass -1 to focus the parent itself.

focused() -> i32
        The currently focused handle, or -1 for the parent.

close_module(handle)
        Tear down the child: drop its Instance + Memory + import closures,
        remove its rect. Frees the handle slot (slots never alias).

module_count() -> i32
        Number of live (non-closed) children. Lets the parent enumerate.
```

### How the child sees a display offset to its rect

The child is **unmodified**: it imports the ordinary `host_display` module and
calls `clear / set_pixel / fill_rect / draw_char / draw_number / present /
width / height / pointer_x / pointer_y / pointer_down / state_get / state_set`
exactly as a fullscreen cartridge does. The host gives each child its **own**
`host_display` import object whose closures are bound to a `ModuleViewport`:

```
viewport = { ox, oy, w, h }   // rect in parent framebuffer coords
```

- `width()`  → returns `viewport.w`  (the child believes the screen IS its rect)
- `height()` → returns `viewport.h`
- `clear(rgb)` → fills only `[ox..ox+w] x [oy..oy+h]` of the shared framebuffer
- `set_pixel(x,y,rgb)` → writes `(ox+x, oy+y)`, dropped if `x,y` fall outside
  `[0,w) x [0,h)` (rect-relative clipping)
- `fill_rect`, `draw_char`, `draw_number` → translated by `(ox,oy)` and clipped to
  the rect (the existing routines already clip to `FB_W/FB_H`; the compose
  variants additionally clamp to the rect)
- `present()` → **no-op for children.** Only the host presents, once per frame,
  after every child has drawn. (A child calling `present()` mid-frame must not
  flip a half-composited buffer to the canvas.)
- `pointer_x/y` → global pointer translated into rect-local coords, or a sentinel
  (`-1`) when the pointer is outside the rect / the child is not focused
- `pointer_down` → 1 only when the primary button is down **and** this child is
  focused **and** the pointer is inside its rect
- `state_get/state_set` → the child gets its **own** 64-slot register file
  (per-instance, not the global one), so two children can't clobber each other's
  state

The key invariant: **a child draws in its own coordinate space, starting at
(0,0), sized `w x h`, and the host translates+clips into the shared buffer.** The
child has no knowledge it is composited. The same `app.wasm` runs fullscreen on
its own subdomain and as a module here — byte-for-byte identical, no "embed
build".

Children themselves get a `host_compose` import object whose `spawn_module`
returns `FAILED`/`-1` in v1: recursion (a module spawning sub-modules) is the
parent's job, mirroring the iframe-era decision that "sub-composition is the
host's job, all at depth 1". This caps nesting and keeps the instance graph flat.

## 2. Per-module instance + memory isolation

Each child is a **separate `WebAssembly.Instance` over its own `WebAssembly.Memory`.**
There is no shared linear memory between parent and children, or between
siblings. This is the strong isolation the iframe gave us (separate address
space) without the iframe (no second origin, no second OPFS, no partitioning).

Host-side, the runtime grows from a single `RUNTIME: Option<CartridgeRuntime>` to
a parent runtime plus a child table:

```rust
struct ModuleViewport { ox: i32, oy: i32, w: i32, h: i32 }

enum ModuleState { Loading, Ready, Failed }

struct ChildModule {
    name: String,
    viewport: ModuleViewport,
    state: ModuleState,
    // Filled once the fetched bytes instantiate. None while Loading/Failed.
    instance: Option<JsValue>,     // WebAssembly.Instance
    memory:   Option<JsValue>,     // this child's exports.memory
    frame:    Option<js_sys::Function>,  // exports.frame, if animated
    render:   Option<js_sys::Function>,  // exports.render, if one-shot
    // This child's OWN host-import closures (its own host_display bound to
    // `viewport`, its own host_net socket table, its own 64-slot state).
    // Held here so wasm's JS references into them stay alive for the
    // child's lifetime; dropped on close_module.
    runtime: ChildRuntime,
    state_regs: [i32; 64],
}

thread_local! {
    static MODULES: RefCell<Vec<Option<ChildModule>>> = ...; // handle = index
    static FOCUS:   Cell<i32> = const { Cell::new(-1) };     // -1 = parent
}
```

Isolation properties:

- **Address space:** each child has its own `Memory`; a pointer in child A's
  memory is meaningless to child B or the parent. The host's `host_display` /
  `host_net` string reads for a given child use *that child's* `memory` handle, so
  a child can only ever read/write its own bytes.
- **Framebuffer:** the one resource children share is the host-owned RGBA buffer,
  and they touch it only through their viewport-clipped `host_display` closures.
  A child physically cannot address a pixel outside its rect — the closure clamps
  before indexing. No child can read pixels back (the API is write-only), so a
  module can't snoop a sibling's rendered output.
- **State:** each child carries its own `state_regs`; `state_get/set` close over
  the child's slot of the `MODULES` table, not the global `STATE`.
- **Net:** each child gets its own `host_net` socket table (the existing
  `NetRuntime` is already per-build; we build one per child). One module's sockets
  are invisible to another.
- **Lifetime / teardown:** `close_module(h)` takes the slot, dropping the
  `Instance`, `Memory`, and every closure; its sockets close. The handle slot
  becomes `None` and is never reused-aliased (new spawns push or fill a `None`
  but always mint a fresh logical identity — same discipline as the `host_net`
  socket table). A new parent load (`run_with_ctx`) clears the whole table.
- **Fault containment:** a child whose `frame(t)` throws (trap) is caught at the
  host call site, marked `Failed`, and skipped on subsequent frames — it cannot
  take down the parent or siblings. A budget (e.g. N consecutive traps) latches
  `Failed` permanently.

### The per-frame composite loop

`start_frame_loop` (the rAF tick) changes from "call the parent's `frame(t)`" to
a **compositor pass**:

```
on each rAF tick (generation still current):
    t = now - start
    parent.frame(t)                 // parent draws chrome + background into FB
    for child in MODULES (in spawn order = z-order):
        match child.state:
            Ready:
                child.pointer = map_pointer_into_child(            // focus-gated
                    POINTER.x, POINTER.y, child.viewport, child.native_w/h)
                                       // .. or "no pointer" when None / not focused
                try child.frame(t)  // renders into child's OWN child_w×child_h buf
                                    // (trap => mark Failed, skip blit)
                blit_child(FB, FB_W, FB_H, child.buf, child.native_w/h,
                           child.viewport.ox/oy/w/h)              // fold into FB
            Loading: draw a placeholder into child.viewport        // see §4
            Failed:  draw an error tile into child.viewport
    host.present()                  // single flip of the composited buffer
```

Two model refinements the pure core makes concrete vs the original sketch:

1. **A child renders into its OWN buffer, then the host blits.** The original
   sketch bound the child's `host_display` viewport so its draws landed directly
   in the shared FB. With `blit_child` the cleaner model is: the child draws into
   a private `child_w x child_h` buffer at its native resolution, and the host
   composites that buffer into the viewport — which is what makes **scaling** free
   (a 256×144 child app shown in a 160×144 panel) and keeps the child's pixels
   physically unable to touch a sibling (it writes its own buffer, never the FB).
   The viewport-bound-draw path still works for a child authored at exactly the
   viewport size (identity scale); `blit_child` subsumes both.
2. **Pointer routing is one `map_pointer_into_child` call per focused child.** The
   host fills the child's `InputSource::Local` pointer cell from the result (or a
   "no pointer" sentinel when `None` / the child isn't focused), so the child's
   `pointer_x/y/down` polls read its own rect-local, focus-gated coordinates.

The parent's own `host_display` closures write the *full* framebuffer
(viewport = whole screen); children's write their rects. Z-order is spawn order:
later children composite on top, matching how a window manager stacks. Only the
host's terminal `present()` flips to the canvas; child `present()` calls are
no-ops, so the user only ever sees fully-composited frames.

## 3. Pointer-event routing

The host already tracks the global pointer in framebuffer coordinates
(`POINTER` cell, updated by the delegated `mousemove` in `events.rs` via
`display::set_pointer`) and the button state (`POINTER_DOWN`). No new DOM
listeners — compose reuses the exact same single delegated listeners (respecting
the *no new `Closure::wrap` outside the four delegated listeners* rule).

Routing happens entirely in the `host_display` closures the host hands each
child:

1. **Hit-testing / focus.** On `mousedown` (already captured globally), the host
   picks the focused child = the **topmost** (last in spawn order) child whose
   rect contains the global pointer. If none contains it, focus → parent (`-1`).
   This is the standard "click to focus" window-manager rule. The parent can also
   set focus explicitly via `focus_module(handle)` (e.g. tabbed UIs).
2. **Coordinate translation.** A focused child's `pointer_x()` returns
   `global_x - viewport.ox`, `pointer_y()` returns `global_y - viewport.oy`. So a
   child sees the cursor in its own (0,0)-origin space, exactly as if it were
   fullscreen at its rect's resolution.
3. **Gating.** For a child that is **not** focused, or whose rect does not contain
   the pointer this instant, `pointer_x/pointer_y` return `-1` and
   `pointer_down` returns `0`. A child therefore only "feels" the pointer when it
   is both focused and hovered — a sibling never sees clicks meant for the
   focused module. (Returning `-1` rather than a stale coordinate matters: poll-
   model cartridges read every frame and must be able to tell "no pointer here".)
4. **The parent** reads the *untranslated* global pointer through its own
   `host_display` (viewport = full screen) and is responsible for any chrome-level
   hit-testing (drag a module's title bar, resize handles, etc.) by calling
   `move_module` / `focus_module`.

Because `pointer_x/pointer_y` already existed as poll imports, **no child needs
modification** to participate in routing — the host simply answers those polls
with rect-local, focus-gated values. Keyboard routing is out of scope for v1
(the framebuffer cartridges are pointer-driven today); when added it follows the
same "deliver to focused handle only" rule.

## 4. Fetching + caching child `app.wasm`

`spawn_module(name, ...)` resolves the child's bytes from the **on-chain
registry**, never from another origin's OPFS (that was the iframe's job and the
source of the mobile partition bug):

```
spawn_module(name):
    handle = allocate MODULES slot, state = Loading
    spawn_local(async {
        id  = registry::id_of_name(name)            // name -> tokenId
        if id == 0: mark handle Failed; return
        wasm = registry::app_wasm_of(id)            // on-chain bytes (cached)
        match wasm:
            Some(bytes): instantiate child (own Instance+Memory+imports),
                         wire viewport, mark Ready
            None:        mark handle Failed          // subdomain has no app
    })
    return handle                                   // immediately, state Loading
```

- **Source of truth:** `registry::app_wasm_of(token_id)` reads
  `metadata(tokenId, keccak256("localharness.app.wasm"))` — the same published
  bytes a visitor to `name.localharness.xyz` would run as its public face. So a
  composed module is *exactly* the module subdomain's live app, with zero extra
  publishing step. (A future refinement can honor `public_face_of`: if the module
  published `html` rather than `app`, render its HTML snapshot into the rect via
  the existing `paint_html_fb`; v1 handles the `app`/cartridge face.)
- **Cache:** a `thread_local! WASM_CACHE: RefCell<HashMap<u64, Rc<Vec<u8>>>>`
  keyed by `tokenId`. The first `spawn_module` for a name hits the chain; repeat
  spawns (or a re-layout that re-spawns) reuse the cached bytes. Instantiation
  still produces a fresh `Instance`+`Memory` per spawn even on a cache hit —
  caching is for the *bytes*, isolation is per *instance*. Cache lifetime = the
  page session; a parent reload clears it (bytes are immutable per published
  version anyway, so staleness only matters across an on-chain republish, which
  is a page-reload-scale event).
- **Before it loads (the `Loading` window).** `app_wasm_of` is an async
  `eth_call` round-trip (hundreds of ms). The handle is returned synchronously in
  `Loading`, and each frame the host paints a **placeholder** into the child's
  rect — a dim fill plus the module name centered via `draw_char` (reusing the
  bitmap font), so the user sees a labeled "loading bitmask…" tile, not a black
  hole. On failure the tile becomes a monochrome error glyph + name. The parent
  can poll `status(handle)` to drive its own chrome (spinner, retry button) and
  decide whether to `close_module` + re-`spawn_module` to retry.
- **Trust:** the bytes are public on-chain data; the host runs them in an
  isolated instance with only the imports it grants (display clipped to the rect,
  net, its own state). A malicious module can at worst draw garbage inside its own
  rect or open its own sockets — it cannot read the parent, a sibling, the
  wallet/seed, OPFS, or the chain-signing path (none of those are in the
  `host_compose`/child `host_display`/`host_net` surface). This is strictly
  *more* contained than the iframe (which carried a full wallet + signer per
  module).

## 5. Worked example — `claude` running `bitmask` as a live panel

Goal: `claude.localharness.xyz` (the agent's own subdomain, tokenId 8) shows
`bitmask.localharness.xyz`'s app as a live, interactive panel occupying the right
half of its framebuffer, with a label strip down the left that the agent draws
itself.

**Parent cartridge (`claude`'s `app.rl`), rustlite:**

```rust
use host::display;
use host::compose;

// Layout: 256x144 framebuffer. Left 96px = claude's own chrome,
// right 160px = the bitmask module.
//   module rect = (96, 0, 160, 144)

fn render() {
    // Mount bitmask once. state_set/get persists the handle across frames.
    // slot 0 = "have we spawned yet", slot 1 = the handle.
    if display::state_get(0) == 0 {
        let h: i32 = compose::spawn_module(name_bitmask(), 96, 0, 160, 144);
        display::state_set(0, 1);
        display::state_set(1, h);
        compose::focus_module(h);   // hand pointer input to the panel
    }
}

fn frame(t: i32) {
    render();                       // ensure the module is mounted

    // claude draws its own chrome in the left strip (its host_display
    // viewport is the WHOLE screen, so it draws at absolute coords).
    display::fill_rect(0, 0, 96, 144, 1118481);   // 0x111111 panel bg
    display::draw_char(8, 8, 99, 16777215, 2);     // 'c'
    display::draw_char(8, 24, 108, 16777215, 2);   // 'l' ...

    let h: i32 = display::state_get(1);
    // Reflect load state in the chrome.
    let st: i32 = compose::status(h);
    if st == 0 { display::draw_char(8, 120, 46, 8421504, 1); }      // '.' loading
    if st == 2 { display::draw_char(8, 120, 88, 16711680, 1); }     // 'X' failed

    // NOTE: claude does NOT call present(). The host composites claude's
    // chrome + the bitmask module into one buffer and presents once.
}

// `name_bitmask` returns a pointer to the length-prefixed string "bitmask"
// in claude's data segment (rustlite string literal → data segment ptr).
fn name_bitmask() -> i32 { ptr_of("bitmask") }
```

**What the host does, frame by frame:**

1. First `frame(t)`: `render()` sees slot 0 == 0, calls
   `compose::spawn_module("bitmask", 96,0,160,144)`. Host resolves
   `id_of_name("bitmask")` → tokenId, kicks an async `app_wasm_of(id)`, returns
   handle `0` in `Loading`, and `focus_module(0)` makes it the focus target.
   Parent draws its left strip; host paints a "bitmask…" placeholder into
   `(96,0,160,144)`; host `present()`s.
2. A few frames later the on-chain bytes arrive. Host instantiates bitmask in its
   **own** `Instance`+`Memory`, builds a `host_display` bound to viewport
   `{96,0,160,144}`, a fresh `host_net` table, and its own 64-slot state.
   `status(0)` flips to `Ready`.
3. Steady state each rAF tick:
   - `claude.frame(t)` paints the left chrome (absolute coords).
   - Host sets the active viewport to bitmask's rect, sets active-focus =
     (`FOCUS == 0`), calls `bitmask.frame(t)`. bitmask calls `display::width()`
     → `160`, `display::height()` → `144`, `display::clear(...)`,
     `set_pixel`/`fill_rect` at its own (0,0)-origin coords — all translated by
     `(+96,+0)` and clipped to the 160×144 rect.
   - Host `present()`s the single composited buffer.
4. The user moves the mouse over the right half and clicks. The global pointer is
   `(180, 70)`. It lies in bitmask's rect, and bitmask is focused, so bitmask's
   `pointer_x()` returns `180-96 = 84`, `pointer_y()` returns `70`,
   `pointer_down()` returns `1`. bitmask toggles the bit under the cursor — fully
   interactive, in its own coordinate space, oblivious to being a panel. The
   pointer over the left strip (`x < 96`) makes bitmask's `pointer_x/y` read `-1`
   and `pointer_down` read `0`, so a click on claude's chrome never leaks into the
   module.

bitmask's `app.wasm` is **the identical file** served at
`bitmask.localharness.xyz`. No embed build, no iframe, no second origin, no
second wallet. Two isolated wasm instances draw into one 256×144 framebuffer that
flips to one `#display-canvas` — and the agent composed it from its own subdomain
by writing ~30 lines of rustlite.

## Wiring summary (where the code lands)

- `src/app/display.rs`
  - new `mod compose` submodule (sibling of `mod net`): the `MODULES` table,
    `ChildModule`/`ModuleViewport`/`ChildRuntime`, the `host_compose` import
    builder, and viewport-aware variants of the draw closures.
  - `build_host_display` gains a `viewport` parameter (the parent passes the full
    screen; children pass their rect) so one builder serves both.
  - `start_frame_loop` becomes the compositor pass (§2): parent frame → children
    frames (viewport+focus-bound) → single host `present()`.
  - `set_pointer`/`POINTER` unchanged; the focus hit-test on mousedown is added in
    the existing delegated listener path (no new listeners).
- `src/rustlite/codegen.rs` / `typecheck` — `host::compose::*` resolves through
  the existing `HostCall` path (`register_import("compose", "spawn_module", …)`
  emits an import from module `host_compose`). No emitter change beyond the new
  builtin signatures in the host-import allowlist.
- `src/rustlite/loader.rs::build_host_imports` — add a `host_compose` stub so a
  standalone-loaded cartridge that references compose still instantiates (its
  `spawn_module` returns `FAILED` outside the DISPLAY compositor context).
- `src/registry.rs` — reuse `id_of_name` + `app_wasm_of` as-is; add the
  `WASM_CACHE` in `display.rs` (caching is a host concern, not a registry one).
- Delete `src/app/compose.rs` + `src/app/embed.rs` and the `?embed=1` / iframe
  `?compose=` routing in `mod.rs`; repoint `?compose=` to mount a synthetic parent
  that `spawn_module`s each named subdomain in a grid (so the URL entrypoint
  survives, now iframe-free), or drop the URL form entirely in favor of an
  agent-authored compositor cartridge.

## Deferred (named honestly)

- **HTML-faced modules**: v1 composes `app`/cartridge faces; a module whose
  `public_face` is `html` should render its HTML snapshot into the rect via
  `paint_html_fb`. Straightforward, not in the first cut.
- **Keyboard routing** to the focused module (cartridges are pointer-only today).
- **Recursive composition** (a module spawning sub-modules). v1 caps nesting at
  depth 1 — a child's `host_compose.spawn_module` returns `FAILED`. When recursion
  is opened, the bound is `ComposeBudget` (`src/compose.rs`): `max_children` (the
  count cap), `max_bytes_per_child`, and `max_total_bytes` (the aggregate cap that
  makes a deep tree converge — every grandchild's bytes count against the same
  total). v1 caps: 8 children, 16 KB each, 64 KB total (`ComposeBudget::v1()`).
  `admit(count, total_bytes, child_bytes)` is checked BEFORE any fetch/instantiate,
  returning the refusal reason so a hit cap logs instead of silently "working". A
  per-tree **depth** cap is the one budget axis not yet a field — add it to
  `ComposeBudget` alongside the byte caps when depth>1 lands.
- **Per-module fuel/time budget** beyond the trap-latch — a runaway child `frame`
  can still burn its slice of the rAF tick; a real scheduler would time-box each
  child.
- **Pixel read-back isolation is already total** (write-only API); if a future
  blit/read API is added it must stay rect-scoped.
```
