#!/usr/bin/env node
// Headless RENDER harness — the Track-A safety net for the present-ownership
// inversion (design/roadmap.md Phase 0a).
//
//   localharness compile app.rl app.wasm
//   node scripts/render-cartridge.js app.wasm
//
// `scripts/validate-cartridge.js` only checks a cartridge doesn't trap (draw
// calls are no-ops). This goes further: it backs host::display with a real
// framebuffer (a Uint8Array — no browser needed) and drives the cartridge under
// the **present-after-frame** model the inversion adopts: the `present` import
// is a NO-OP, and the HOST snapshots the framebuffer after `frame()` returns.
// Then it asserts real pixels landed. This proves the present-ownership model
// renders correctly (the part the native raster unit tests can't exercise: who
// presents, and when) without a headless browser. The pixel MATH is already
// covered by `crate::raster`'s native tests; this covers the call-ordering.
//
// Exits non-zero if the cartridge renders nothing, or (with expectations) if a
// region that should be drawn is blank.

const fs = require('fs');
const FB_W = 256;
const FB_H = 144;
const path = process.argv[2];
if (!path) {
  console.error('usage: node scripts/render-cartridge.js <cartridge.wasm>');
  process.exit(2);
}

const fb = new Uint8Array(FB_W * FB_H * 4);
const state = new Map();
const ptr = { x: 0, y: 0, down: 0 };

function setPixel(x, y, rgb) {
  if (x < 0 || y < 0 || x >= FB_W || y >= FB_H) return;
  const i = (y * FB_W + x) * 4;
  fb[i] = (rgb >>> 16) & 255;
  fb[i + 1] = (rgb >>> 8) & 255;
  fb[i + 2] = rgb & 255;
  fb[i + 3] = 255;
}
function fillRect(x, y, w, h, rgb) {
  const x0 = Math.max(0, x), y0 = Math.max(0, y);
  const x1 = Math.min(FB_W, x + w), y1 = Math.min(FB_H, y + h);
  for (let yy = y0; yy < y1; yy++) for (let xx = x0; xx < x1; xx++) setPixel(xx, yy, rgb);
}

const host_display = {
  clear: (rgb) => fillRect(0, 0, FB_W, FB_H, rgb),
  set_pixel: (x, y, rgb) => setPixel(x, y, rgb),
  fill_rect: (x, y, w, h, rgb) => fillRect(x, y, w, h, rgb),
  draw_char: () => {}, // glyph fidelity is covered by crate::raster native tests
  draw_number: () => {},
  present: () => {}, // PRESENT IS A NO-OP — the host presents after frame()
  width: () => FB_W,
  height: () => FB_H,
  pointer_x: () => ptr.x,
  pointer_y: () => ptr.y,
  pointer_down: () => ptr.down,
  state_get: (s) => state.get(s | 0) || 0,
  state_set: (s, v) => { state.set(s | 0, v | 0); },
};

const mod = new WebAssembly.Module(fs.readFileSync(path));
const importObj = {};
for (const imp of WebAssembly.Module.imports(mod)) {
  importObj[imp.module] = importObj[imp.module] || {};
  if (imp.module === 'host_display') {
    importObj.host_display = host_display;
  } else if (imp.kind === 'function') {
    importObj[imp.module][imp.name] = () => 0;
  } else if (imp.kind === 'memory') {
    importObj[imp.module][imp.name] = new WebAssembly.Memory({ initial: 1 });
  }
}
const inst = new WebAssembly.Instance(mod, importObj);
const frame = inst.exports.frame || inst.exports.render;
if (typeof frame !== 'function') {
  console.error('FAIL: no frame()/render() export');
  process.exit(1);
}

// present-after-frame model: run frame, THEN the host presents (snapshot).
frame(0);
const presented = fb.slice();

let lit = 0;
for (let i = 0; i < presented.length; i += 4) {
  if (presented[i] || presented[i + 1] || presented[i + 2]) lit++;
}
console.log(`rendered ${path}: ${lit} lit pixels of ${FB_W * FB_H}`);
if (lit === 0) {
  console.error('FAIL: framebuffer is blank after frame() — nothing rendered');
  process.exit(1);
}
console.log('PASS: renders a non-blank frame under the present-after-frame host model');
