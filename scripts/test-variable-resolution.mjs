#!/usr/bin/env node
// scripts/test-variable-resolution.mjs — VARIABLE FRAMEBUFFER RESOLUTION gate.
//
// Proves the dims() convention end-to-end through the REAL worker host
// (web/cartridge-worker.js, loaded as a module): a cartridge that exports
// `dims() -> i32` returning a packed `(w<<16)|h` makes the worker allocate a
// w×h framebuffer; `renderOnce` reflects the new size via `liveDims()`. A
// cartridge with NO dims() export stays at the 256×144 default (backward
// compatible). Also checks the clamp range: an out-of-range dims() falls back.
//
//   1. compile a 64×64 dims() cartridge via the CLI -> instantiate through the
//      worker host -> assert liveDims() == [64, 64] and pixels landed.
//   2. compile a no-dims() cartridge -> assert liveDims() == [256, 144].
//   3. decodeDims() unit checks: in-range packs decode, out-of-range -> null.
//
// Run standalone:  node scripts/test-variable-resolution.mjs
// Wired into verify.sh as a stage. Exits non-zero on any FAIL.

import { execFileSync } from 'node:child_process';
import { readFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { createRequire } from 'node:module';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const require = createRequire(import.meta.url);
// The REAL worker host re-impl — single source of truth for the dims path.
const worker = require(join(ROOT, 'web', 'cartridge-worker.js'));

let fail = 0;
function check(label, cond, detail) {
  if (cond) {
    console.log(`  PASS  ${label}${detail ? '  ' + detail : ''}`);
  } else {
    fail++;
    console.error(`  FAIL  ${label}${detail ? '  ' + detail : ''}`);
  }
}

// Compile a rustlite source string to wasm via the CLI (same path as the
// corpus gate). stderr is captured + printed only on failure.
function compileSource(src) {
  mkdirSync(join(ROOT, 'target'), { recursive: true });
  const srcPath = join(ROOT, 'target', '.varres-src.rl');
  const outPath = join(ROOT, 'target', '.varres-out.wasm');
  const fs = require('node:fs');
  fs.writeFileSync(srcPath, src);
  try {
    execFileSync(
      'cargo',
      ['run', '--quiet', '--features', 'wallet', '--bin', 'localharness', '--', 'compile', srcPath, outPath],
      { cwd: ROOT, stdio: ['ignore', 'ignore', 'pipe'] },
    );
  } catch (err) {
    const stderr = err.stderr ? err.stderr.toString() : '';
    throw new Error(`compile failed:\n${stderr.trim()}`);
  }
  return readFileSync(outPath);
}

console.log('VARIABLE FRAMEBUFFER RESOLUTION — dims() convention through the worker host\n');

// ---- 1. a 64×64 dims() cartridge resizes the framebuffer -------------------
{
  // dims() = (64<<16)|64. frame() fills the whole 64×64 surface so a pixel
  // lands and the framebuffer length must be 64*64*4.
  const src = `
fn dims() -> i32 {
    (64 << 16) | 64
}
fn frame(t: i32) {
    host::display::clear(0x112233);
    host::display::fill_rect(0, 0, 64, 64, 0xff8800);
    host::display::present();
}
`;
  const wasm = compileSource(src);
  const fb = worker.renderOnce(wasm, 0);
  const [w, h] = worker.liveDims();
  check('64×64 dims() resizes framebuffer', w === 64 && h === 64, `liveDims = ${w}×${h}`);
  check('64×64 framebuffer is w*h*4 bytes', fb.length === 64 * 64 * 4, `len = ${fb.length} (want ${64 * 64 * 4})`);
  // pixel (10,10) is inside the fill -> 0xff8800 == R,G,B = 255,136,0.
  const i = (10 * 64 + 10) * 4;
  check('64×64 cartridge drew pixels', fb[i] === 255 && fb[i + 1] === 136 && fb[i + 2] === 0,
    `px(10,10) = [${fb[i]},${fb[i + 1]},${fb[i + 2]}]`);
}

// ---- 2. a cartridge WITHOUT dims() stays 256×144 (backward compatible) ------
{
  const src = `
fn frame(t: i32) {
    host::display::clear(0x000044);
    host::display::present();
}
`;
  const wasm = compileSource(src);
  const fb = worker.renderOnce(wasm, 0);
  const [w, h] = worker.liveDims();
  check('no dims() -> default 256×144', w === 256 && h === 144, `liveDims = ${w}×${h}`);
  check('default framebuffer is 256*144*4 bytes', fb.length === 256 * 144 * 4, `len = ${fb.length}`);
}

// ---- 3. decodeDims clamp range ---------------------------------------------
{
  const min = worker.FB_MIN;
  const max = worker.FB_MAX;
  check('decodeDims in-range', JSON.stringify(worker.decodeDims((320 << 16) | 240)) === JSON.stringify([320, 240]));
  check('decodeDims min edge', JSON.stringify(worker.decodeDims((min << 16) | min)) === JSON.stringify([min, min]));
  check('decodeDims max edge', JSON.stringify(worker.decodeDims((max << 16) | max)) === JSON.stringify([max, max]));
  check('decodeDims width too small -> null', worker.decodeDims(((min - 1) << 16) | 100) === null);
  check('decodeDims height too big -> null', worker.decodeDims((100 << 16) | (max + 1)) === null);
  check('decodeDims zero -> null', worker.decodeDims(0) === null);
}

console.log(`\nSUMMARY: ${fail ? fail + ' FAILED' : 'all passed'}.`);
if (fail > 0) {
  console.error('VARIABLE RESOLUTION GATE FAILED.');
  process.exit(1);
}
console.log('VARIABLE RESOLUTION OK — dims() resizes the framebuffer; no-dims() stays 256×144; clamp range enforced.');
