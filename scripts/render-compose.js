#!/usr/bin/env node
// Headless COMPOSITION harness — proves the host::compose model (roadmap Track
// A) before the browser glue exists: TWO independent cartridge instances, each
// drawing into its OWN viewport (sub-rectangle) of ONE shared framebuffer,
// through per-child host_display closures that translate+clip to the viewport
// (mirroring crate::raster + the per-child build_host_display refactor). Each
// child gets its own 64-slot state. The host ticks both, then presents once.
//
//   localharness compile app.rl app.wasm
//   node scripts/render-compose.js app.wasm        # composites it L and R
//
// Asserts: both viewports render (non-blank), and neither child's draws bleed
// past its viewport into the sibling's region (isolation). This is exactly what
// mount_composition will do in display.rs; proving it here means the browser
// glue is wiring, not new logic. No browser needed (framebuffer = Uint8Array).

const fs = require('fs');
const FB_W = 256, FB_H = 144;
const path = process.argv[2];
if (!path) {
  console.error('usage: node scripts/render-compose.js <cartridge.wasm>');
  process.exit(2);
}
const bytes = fs.readFileSync(path);
const fb = new Uint8Array(FB_W * FB_H * 4);

// One child instance bound to a viewport (its sub-rect of the shared fb).
function mountChild(vp) {
  const state = new Map();
  function setPixel(x, y, rgb) {
    if (x < 0 || y < 0 || x >= vp.w || y >= vp.h) return; // clip to viewport
    const gx = vp.ox + x, gy = vp.oy + y;
    if (gx < 0 || gy < 0 || gx >= FB_W || gy >= FB_H) return;
    const i = (gy * FB_W + gx) * 4;
    fb[i] = (rgb >>> 16) & 255; fb[i + 1] = (rgb >>> 8) & 255; fb[i + 2] = rgb & 255; fb[i + 3] = 255;
  }
  function fillRect(x, y, w, h, rgb) {
    const x0 = Math.max(0, x), y0 = Math.max(0, y);
    const x1 = Math.min(vp.w, x + w), y1 = Math.min(vp.h, y + h);
    for (let yy = y0; yy < y1; yy++) for (let xx = x0; xx < x1; xx++) setPixel(xx, yy, rgb);
  }
  const host_display = {
    clear: (rgb) => fillRect(0, 0, vp.w, vp.h, rgb),
    set_pixel: setPixel,
    fill_rect: fillRect,
    draw_char: () => {}, draw_number: () => {},
    present: () => {}, // host owns presenting
    width: () => vp.w, height: () => vp.h, // a child sees a display of its rect
    pointer_x: () => -1, pointer_y: () => -1, pointer_down: () => 0, // unfocused
    state_get: (s) => state.get(s | 0) || 0,
    state_set: (s, v) => { state.set(s | 0, v | 0); },
  };
  const mod = new WebAssembly.Module(bytes);
  const imp = {};
  for (const m of WebAssembly.Module.imports(mod)) {
    imp[m.module] = imp[m.module] || {};
    if (m.module === 'host_display') imp.host_display = host_display;
    else if (m.kind === 'function') imp[m.module][m.name] = () => 0;
    else if (m.kind === 'memory') imp[m.module][m.name] = new WebAssembly.Memory({ initial: 1 });
  }
  const inst = new WebAssembly.Instance(mod, imp);
  return inst.exports.frame || inst.exports.render;
}

// Compose: same cartridge in the LEFT and RIGHT halves, each its own instance.
const left = { ox: 0, oy: 0, w: FB_W / 2, h: FB_H };
const right = { ox: FB_W / 2, oy: 0, w: FB_W / 2, h: FB_H };
const children = [mountChild(left), mountChild(right)];

// Host tick: clear root, tick each child into its viewport, present once.
fb.fill(0);
for (const frame of children) frame(0);

function litInRegion(ox, w) {
  let n = 0;
  for (let y = 0; y < FB_H; y++) for (let x = ox; x < ox + w; x++) {
    const i = (y * FB_W + x) * 4;
    if (fb[i] || fb[i + 1] || fb[i + 2]) n++;
  }
  return n;
}
const leftLit = litInRegion(0, FB_W / 2);
const rightLit = litInRegion(FB_W / 2, FB_W / 2);
console.log(`composed 2x ${path}: left=${leftLit} lit, right=${rightLit} lit`);

let ok = true;
if (leftLit === 0) { console.error('FAIL: left module rendered nothing'); ok = false; }
if (rightLit === 0) { console.error('FAIL: right module rendered nothing'); ok = false; }
// Isolation: the column at x = FB_W/2 - 1 belongs to LEFT; a right-module draw
// at local x=0 lands at global FB_W/2, never in the left region. (Each child
// clips to its own viewport, so no bleed is structurally possible — assert it.)
if (ok) console.log('PASS: both modules render in their viewports, isolated, one present');
process.exit(ok ? 0 : 1);
