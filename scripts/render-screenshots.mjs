#!/usr/bin/env node
// scripts/render-screenshots.mjs — REAL mobile screenshots for the README.
//
// These are NOT mockups. They are the genuine pixel output of the published
// cartridges (readyup / fractal) rendered through the SAME worker host the
// browser runs (web/cartridge-worker.js) — the exact bytes a visitor to
// <name>.localharness.xyz sees fullscreen on a phone. We frame each in a
// minimal monochrome bezel (decorative; the screen content is real) and write a
// PNG. The fractal is rendered through the real recursive compose path: we
// resolve each spawned child with the same wasm (simulating the on-chain
// compose_bytes round-trip) so the nested Droste depth is the true output.
//
// Run:  node scripts/render-screenshots.mjs   (compiles the .rl files first)
// Out:  web/screenshots/{readyup,fractal}.png

import { execFileSync } from 'node:child_process';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { createRequire } from 'node:module';
import { deflateSync } from 'node:zlib';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const require = createRequire(import.meta.url);
const worker = require(join(ROOT, 'web', 'cartridge-worker.js'));

// ---- compile a .rl cartridge to wasm via the CLI ----------------------------
function compile(rl) {
  const out = join(ROOT, 'target', rl.split('/').pop().replace('.rl', '.wasm'));
  execFileSync('cargo', ['run', '--quiet', '--features', 'wallet', '--bin', 'localharness',
    '--', 'compile', join(ROOT, rl), out], { cwd: ROOT, stdio: ['ignore', 'ignore', 'pipe'] });
  const b = readFileSync(out);
  return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength); // ArrayBuffer
}

// ---- PNG (truecolor + alpha, 8-bit) — no deps beyond zlib -------------------
const CRCT = (() => { const t = []; for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; t[n] = c >>> 0; } return t; })();
const crc32 = (buf) => { let c = 0xffffffff; for (let i = 0; i < buf.length; i++) c = CRCT[(c ^ buf[i]) & 0xff] ^ (c >>> 8); return (c ^ 0xffffffff) >>> 0; };
const u32 = (n) => { const b = Buffer.alloc(4); b.writeUInt32BE(n >>> 0, 0); return b; };
const chunk = (type, data) => { const body = Buffer.concat([Buffer.from(type, 'ascii'), data]); return Buffer.concat([u32(data.length), body, u32(crc32(body))]); };
function encodePNG(w, h, rgba) {
  const sig = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  const ihdr = Buffer.concat([u32(w), u32(h), Buffer.from([8, 6, 0, 0, 0])]); // 8-bit RGBA
  const stride = w * 4;
  const raw = Buffer.alloc((stride + 1) * h);
  for (let y = 0; y < h; y++) { raw[y * (stride + 1)] = 0; rgba.copy(raw, y * (stride + 1) + 1, y * stride, y * stride + stride); }
  return Buffer.concat([sig, chunk('IHDR', ihdr), chunk('IDAT', deflateSync(raw, { level: 9 })), chunk('IEND', Buffer.alloc(0))]);
}

// ---- tiny pixel ops on an RGBA Buffer ---------------------------------------
const mkimg = (w, h, c) => { const b = Buffer.alloc(w * h * 4); for (let i = 0; i < w * h; i++) { b[i * 4] = c[0]; b[i * 4 + 1] = c[1]; b[i * 4 + 2] = c[2]; b[i * 4 + 3] = 255; } return b; };
const setpx = (b, w, x, y, c) => { if (x < 0 || y < 0 || x >= w) return; const i = (y * w + x) * 4; if (i < 0 || i + 3 >= b.length) return; b[i] = c[0]; b[i + 1] = c[1]; b[i + 2] = c[2]; b[i + 3] = 255; };
const rect = (b, w, h, x, y, rw, rh, c) => { for (let yy = Math.max(0, y); yy < Math.min(h, y + rh); yy++) for (let xx = Math.max(0, x); xx < Math.min(w, x + rw); xx++) setpx(b, w, xx, yy, c); };
function blitScaled(dst, dw, dh, dx, dy, vw, vh, src, sw, sh) { // nearest-neighbour
  for (let y = 0; y < vh; y++) { const sy = Math.floor(y * sh / vh); for (let x = 0; x < vw; x++) { const sx = Math.floor(x * sw / vw); const si = (sy * sw + sx) * 4; setpx(dst, dw, dx + x, dy + y, [src[si], src[si + 1], src[si + 2]]); } }
}
function text(dst, dw, dh, x, y, str, scale, c) { // 5x7 bitmap font, advance 6
  let cx = x;
  for (const chr of str) { const g = worker.glyph5x7(chr.codePointAt(0)); for (let row = 0; row < 7; row++) { const bits = g[row]; for (let col = 0; col < 5; col++) if ((bits >> (4 - col)) & 1) rect(dst, dw, dh, cx + col * scale, y + row * scale, scale, scale, c); } cx += 6 * scale; }
}

// ---- the phone frame --------------------------------------------------------
// Fixed 9:16 portrait screen (matches readyup; landscape cartridges letterbox
// exactly as the app does with object-fit: contain).
const SCREEN_W = 540, SCREEN_H = 960; // 2x of 270x480
function phone(screenRGBA, sw, sh, url) {
  const M = 20, TB = 52, BB = 44, BORDER = 2;
  const W = SCREEN_W + 2 * M, H = SCREEN_H + TB + BB;
  const body = [14, 14, 14], edge = [40, 40, 40], black = [0, 0, 0], muted = [130, 130, 130];
  const im = mkimg(W, H, body);
  rect(im, W, H, 0, 0, W, BORDER, edge); rect(im, W, H, 0, H - BORDER, W, BORDER, edge);
  rect(im, W, H, 0, 0, BORDER, H, edge); rect(im, W, H, W - BORDER, 0, BORDER, H, edge);
  text(im, W, H, M, Math.floor((TB - 14) / 2), url, 2, muted);            // status bar = the real URL
  rect(im, W, H, M, TB, SCREEN_W, SCREEN_H, black);                       // screen
  const scale = Math.min(SCREEN_W / sw, SCREEN_H / sh);                   // contain-fit (app's letterbox)
  const vw = Math.round(sw * scale), vh = Math.round(sh * scale);
  blitScaled(im, W, H, M + Math.floor((SCREEN_W - vw) / 2), TB + Math.floor((SCREEN_H - vh) / 2), vw, vh, screenRGBA, sw, sh);
  rect(im, W, H, Math.floor(W / 2) - 46, H - Math.floor(BB / 2) - 3, 92, 6, [80, 80, 80]); // home bar
  return { rgba: im, w: W, h: H };
}

const u8 = (u32arr) => Buffer.from(new Uint8Array(u32arr.buffer, u32arr.byteOffset, u32arr.byteLength * 1).buffer
  ? new Uint8Array(u32arr.buffer) : new Uint8Array(u32arr.buffer));

// ---- 1. readyup: a single static frame (the portrait Ready Up UI) -----------
function renderReadyup(bytes) {
  worker.composeReset();
  const fb = worker.renderOnce(bytes, 0);            // Uint8ClampedArray, RGBA
  const [w, h] = worker.liveDims();
  return { rgba: Buffer.from(fb.buffer, fb.byteOffset, fb.byteLength), w, h };
}

// ---- 2. fractal: render the REAL recursive nesting --------------------------
// renderOnce draws the root frame and (via its frame()) spawns child 0 LOADING.
// We then resolve every LOADING node with the same wasm (simulating the
// compose_bytes reply) and re-run the composite pass; each pass resolves the
// next level until the depth cap stops the recursion. Root frame is re-seeded
// each pass so children composite on top of it — exactly like the worker tick.
function resolveLoading(children, parent, bytes) {
  for (let h = 0; h < children.length; h++) {
    const c = children[h];
    if (!c) continue;
    if (c.state === 0) worker.composeInstantiateForTest(h, bytes, parent); // 0 = LOADING
    else if (c.state === 1) resolveLoading(c.children, c, bytes);          // 1 = READY → recurse
  }
}
function renderFractal(bytes) {
  const t = 4200;
  worker.composeReset();
  const root = worker.renderOnce(bytes, t);
  const [w, h] = worker.liveDims();
  const root32 = new Uint32Array(root.buffer.slice(0));
  const parent32 = new Uint32Array(w * h);
  for (let pass = 0; pass < 7; pass++) {            // depth cap is 5; a couple extra to settle
    resolveLoading(worker.composeChildren(), null, bytes);
    parent32.set(root32);                           // re-seed the root frame
    worker.composeRunPass(parent32, w, h, t, { x: -1, y: -1, down: 0 });
  }
  return { rgba: Buffer.from(new Uint8Array(parent32.buffer)), w, h };
}

// ---- run --------------------------------------------------------------------
mkdirSync(join(ROOT, 'web', 'screenshots'), { recursive: true });
const readyupBytes = compile('examples/cartridges/readyup.rl');
const fractalBytes = compile('examples/cartridges/fractal.rl');

const shots = [
  { name: 'readyup', url: 'readyup.localharness.xyz', ...renderReadyup(readyupBytes) },
  { name: 'fractal', url: 'fractal.localharness.xyz', ...renderFractal(fractalBytes) },
];
for (const s of shots) {
  const p = phone(s.rgba, s.w, s.h, s.url);
  const png = encodePNG(p.w, p.h, p.rgba);
  const out = join(ROOT, 'web', 'screenshots', `${s.name}.png`);
  writeFileSync(out, png);
  // sanity: report non-black pixel ratio in the screen so a blank render is caught
  let lit = 0; for (let i = 0; i < s.rgba.length; i += 4) if (s.rgba[i] | s.rgba[i + 1] | s.rgba[i + 2]) lit++;
  const ratio = (100 * lit / (s.w * s.h)).toFixed(1);
  console.log(`wrote ${out}  (${p.w}x${p.h}, screen ${s.w}x${s.h}, ${ratio}% lit, ${png.length} bytes)`);
}
