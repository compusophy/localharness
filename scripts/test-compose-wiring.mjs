#!/usr/bin/env node
// scripts/test-compose-wiring.mjs — CARTRIDGE-IN-CARTRIDGE COMPOSITION gate.
//
// Proves host::compose end-to-end through the REAL worker host
// (web/cartridge-worker.js, loaded as a module): a PARENT framebuffer composites
// a CHILD cartridge — instantiated in its OWN buffer at its own dims() — into a
// sub-rectangle, nearest-neighbour scaled, with pointer routing into the focused
// child. NO iframes — pure pixel composition.
//
//   1. compile a child cartridge (fills its whole 64×64 surface red) via the CLI;
//      mount it into a 128×72 sub-rect of a 256×144 parent FB; instantiate its
//      bytes (simulating the main-thread compose_bytes reply); run the composite
//      pass; assert the child's red pixels appear at the SCALED rect location and
//      NOWHERE outside it (isolation).
//   2. focus the child + place the pointer over its rect; assert the child's
//      pointer cell maps to the correct child-local coords; place it outside and
//      assert the child reads "no pointer" (-1).
//   3. PARITY: blitChild / mapPointerIntoChild (the worker's JS mirrors) reproduce
//      src/compose.rs::blit_child / map_pointer_into_child on pinned test vectors
//      (the exact values the Rust unit tests assert) — guards JS↔Rust drift.
//   4. BUDGET: the child-count cap (ComposeBudget v1 = 8) refuses a 9th mount.
//   5. RECURSION (the fractal): a grandchild composites child→parent→root
//      through two blits — proof composition is a tree, not depth-1.
//   6. DEPTH CAP: the recursion terminates — a node at MAX_DEPTH can't spawn.
//
// Run standalone:  node scripts/test-compose-wiring.mjs
// Wired into verify.sh as a stage. Exits non-zero on any FAIL.

import { execFileSync } from 'node:child_process';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { createRequire } from 'node:module';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const require = createRequire(import.meta.url);
// The REAL worker host re-impl — single source of truth for the compose path.
const worker = require(join(ROOT, 'web', 'cartridge-worker.js'));

let fail = 0;
function check(label, cond, detail) {
  if (cond) console.log(`  PASS  ${label}${detail ? '  ' + detail : ''}`);
  else { fail++; console.error(`  FAIL  ${label}${detail ? '  ' + detail : ''}`); }
}

function compileSource(src, tag) {
  mkdirSync(join(ROOT, 'target'), { recursive: true });
  const srcPath = join(ROOT, 'target', `.compose-${tag}.rl`);
  const outPath = join(ROOT, 'target', `.compose-${tag}.wasm`);
  writeFileSync(srcPath, src);
  try {
    execFileSync(
      'cargo',
      ['run', '--quiet', '--features', 'wallet', '--bin', 'localharness', '--', 'compile', srcPath, outPath],
      { cwd: ROOT, stdio: ['ignore', 'ignore', 'pipe'] },
    );
  } catch (err) {
    throw new Error(`compile failed (${tag}):\n${(err.stderr ? err.stderr.toString() : '').trim()}`);
  }
  return readFileSync(outPath);
}

// Read a packed-u32 pixel (0xAABBGGRR) from a Uint32Array FB.
function pix(fb32, w, x, y) { return fb32[y * w + x]; }
// Pack 0xRRGGBB the same way the host does (opaque alpha) for comparison.
function packRgb(rgb) {
  const r = (rgb >>> 16) & 0xff, g = (rgb >>> 8) & 0xff, b = rgb & 0xff;
  return ((0xff << 24) | (b << 16) | (g << 8) | r) >>> 0;
}

console.log('CARTRIDGE-IN-CARTRIDGE COMPOSITION — host::compose through the worker host\n');

const RED = 0xff0000;
const REDP = packRgb(RED);

// ---- compile the child: a 64×64 cartridge that fills its whole surface red --
const childSrc = `
fn dims() -> i32 {
    (64 << 16) | 64
}

fn frame(t: i32) {
    host::display::clear(0xff0000);
    host::display::present();
}
`;
const childWasm = compileSource(childSrc, 'child');

// ---- 1. composite the child into a sub-rect; assert scaled + isolated -------
{
  const PW = 256, PH = 144;
  // Child rect: (96, 0, 128, 72) — the child's 64×64 surface upscaled 2x in W,
  // ~1.125x in H, placed in the top-right quadrant.
  const RX = 96, RY = 0, RW = 128, RH = 72;
  worker.composeReset();
  const handle = worker.composeMountForTest('child', RX, RY, RW, RH);
  check('1a child mounts into a slot', handle >= 0, `handle=${handle}`);
  // Simulate the main-thread compose_bytes reply: instantiate the fetched bytes.
  worker.composeInstantiateForTest(handle, childWasm.buffer.slice(childWasm.byteOffset, childWasm.byteOffset + childWasm.byteLength));
  const child = worker.composeChildren()[handle];
  check('1b child instantiated READY at its dims()', child && child.state === 1 && child.w === 64 && child.h === 64,
    child ? `state=${child.state} ${child.w}x${child.h}` : 'no child');

  // Parent FB starts black; run the composite pass (parent draws nothing here —
  // we test the child's contribution in isolation).
  const parentFb = new Uint32Array(PW * PH);
  worker.composeRunPass(parentFb, PW, PH, 0, { x: 0, y: 0, down: 0 });

  // The child filled its surface red → after the blit, every pixel INSIDE the
  // rect is red and every pixel OUTSIDE the rect is still 0 (isolation).
  check('1c rect top-left is the child colour', pix(parentFb, PW, RX, RY) === REDP, `0x${pix(parentFb, PW, RX, RY).toString(16)}`);
  check('1d rect bottom-right is the child colour', pix(parentFb, PW, RX + RW - 1, RY + RH - 1) === REDP);
  check('1e rect center is the child colour', pix(parentFb, PW, RX + RW / 2, RY + RH / 2) === REDP);
  // Just left of the rect → untouched (the child can't scribble past its rect).
  check('1f pixel left of rect untouched', pix(parentFb, PW, RX - 1, RY) === 0);
  // Below the rect → untouched.
  check('1g pixel below rect untouched', pix(parentFb, PW, RX, RY + RH) === 0);
  // Count: every pixel in the rect lit, none outside.
  let inLit = 0, outLit = 0;
  for (let y = 0; y < PH; y++) for (let x = 0; x < PW; x++) {
    const lit = pix(parentFb, PW, x, y) !== 0;
    const inside = x >= RX && x < RX + RW && y >= RY && y < RY + RH;
    if (lit && inside) inLit++;
    if (lit && !inside) outLit++;
  }
  check('1h every rect pixel is composited', inLit === RW * RH, `inLit=${inLit}/${RW * RH}`);
  check('1i no pixel bled outside the rect', outLit === 0, `outLit=${outLit}`);
}

// ---- 2. pointer routes into the focused child, gated to its rect ------------
{
  const PW = 256, PH = 144;
  const RX = 96, RY = 0, RW = 128, RH = 72;
  worker.composeReset();
  const handle = worker.composeMountForTest('child', RX, RY, RW, RH);
  worker.composeInstantiateForTest(handle, childWasm.buffer.slice(childWasm.byteOffset, childWasm.byteOffset + childWasm.byteLength));
  worker.composeFocusForTest(handle);
  check('2a focus set to the child', worker.composeFocus() === handle, `focus=${worker.composeFocus()}`);

  const parentFb = new Uint32Array(PW * PH);
  // Pointer at parent (160, 36): inside the rect. Local viewport offset
  // (160-96, 36-0) = (64, 36); scaled into the 64×64 child:
  //   cx = 64 * 64 / 128 = 32 ; cy = 36 * 64 / 72 = 32.
  worker.composeRunPass(parentFb, PW, PH, 0, { x: 160, y: 36, down: 1 });
  const c = worker.composeChildren()[handle];
  check('2b focused child sees rect-local pointer', c.ptr.x === 32 && c.ptr.y === 32 && c.ptr.down === 1,
    `(${c.ptr.x},${c.ptr.y}) down=${c.ptr.down}`);

  // Pointer OUTSIDE the rect (left strip) → the child reads "no pointer".
  worker.composeRunPass(parentFb, PW, PH, 0, { x: 10, y: 10, down: 1 });
  check('2c pointer outside the rect → no pointer (-1)', c.ptr.x === -1 && c.ptr.y === -1 && c.ptr.down === 0,
    `(${c.ptr.x},${c.ptr.y}) down=${c.ptr.down}`);

  // Unfocus (focus the parent): even a pointer over the rect doesn't reach it.
  worker.composeFocusForTest(-1);
  worker.composeRunPass(parentFb, PW, PH, 0, { x: 160, y: 36, down: 1 });
  check('2d unfocused child feels no pointer', c.ptr.x === -1 && c.ptr.down === 0, `(${c.ptr.x},${c.ptr.y})`);
}

// ---- 3. JS↔Rust parity on pinned vectors (mirror src/compose.rs unit tests) -
{
  const { blitChild, mapPointerIntoChild } = worker;

  // 3a blit identity copies child pixel-for-pixel (compose.rs
  // blit_identity_copies_child_pixel_for_pixel): 4×4 child, value = cy*4+cx,
  // identity scale at offset (2,3) in a 16×16 dst.
  {
    const dst = new Uint32Array(16 * 16);
    const cw = 4, ch = 4;
    const child = new Uint32Array(cw * ch);
    for (let i = 0; i < cw * ch; i++) child[i] = i;
    blitChild(dst, 16, 16, child, cw, ch, 2, 3, cw, ch);
    let ok = true;
    for (let cy = 0; cy < ch; cy++) for (let cx = 0; cx < cw; cx++) {
      if (dst[(3 + cy) * 16 + (2 + cx)] !== cy * cw + cx) ok = false;
    }
    ok = ok && dst[3 * 16 + 1] === 0 && dst[3 * 16 + (2 + cw)] === 0;
    check('3a blitChild identity matches Rust', ok);
  }

  // 3b blit 2x nearest-neighbour (blit_scales_2x_nearest_neighbour): child
  // [10,20,30,40] → 4×4 viewport; each source pixel a 2×2 block.
  {
    const dst = new Uint32Array(8 * 8);
    const child = new Uint32Array([10, 20, 30, 40]);
    blitChild(dst, 8, 8, child, 2, 2, 0, 0, 4, 4);
    const ok = dst[0] === 10 && dst[1 * 8 + 1] === 10 && dst[2] === 20 && dst[1 * 8 + 3] === 20 &&
               dst[2 * 8 + 0] === 30 && dst[2 * 8 + 2] === 40 && dst[3 * 8 + 3] === 40;
    check('3b blitChild 2x scale matches Rust', ok);
  }

  // 3c blit half drops source pixels (blit_scales_half_drops_source_pixels):
  // 4×4 child (value cy*4+cx) → 2×2 viewport picks src (0,0),(2,0),(0,2),(2,2).
  {
    const dst = new Uint32Array(8 * 8);
    const child = new Uint32Array(16);
    for (let i = 0; i < 16; i++) child[i] = i;
    blitChild(dst, 8, 8, child, 4, 4, 0, 0, 2, 2);
    const ok = dst[0] === 0 && dst[1] === 2 && dst[1 * 8 + 0] === 8 && dst[1 * 8 + 1] === 10;
    check('3c blitChild 0.5x scale matches Rust', ok);
  }

  // 3d blit clips at left/top WITHOUT shifting (blit_clips_at_left_and_top...):
  // 4×4 identity child at (-2,-2) shows its bottom-right; dst(0,0)=child(2,2)=10.
  {
    const dst = new Uint32Array(4 * 4);
    const child = new Uint32Array(16);
    for (let i = 0; i < 16; i++) child[i] = i;
    blitChild(dst, 4, 4, child, 4, 4, -2, -2, 4, 4);
    check('3d blitChild left/top clip matches Rust', dst[0] === 10 && dst[1 * 4 + 1] === 15 && dst[2 * 4 + 2] === 0);
  }

  // 3e fully-offscreen blit is a no-op (blit_fully_offscreen_is_a_noop).
  {
    const dst = new Uint32Array(4 * 4);
    const child = new Uint32Array([9, 9, 9, 9]);
    blitChild(dst, 4, 4, child, 2, 2, 4, 0, 2, 2);     // past right
    blitChild(dst, 4, 4, child, 2, 2, -2, 0, 2, 2);    // ends at x=0
    check('3e offscreen blitChild is a no-op', dst.every((p) => p === 0));
  }

  // 3f pointer inside maps to child-local (pointer_inside_viewport_maps...):
  // vp (10,20,64,32), child 64×32 → identity; parent (60,45) → child (50,25).
  check('3f mapPointer inside matches Rust', (() => {
    const m = mapPointerIntoChild(60, 45, 10, 20, 64, 32, 64, 32);
    return m && m[0] === 50 && m[1] === 25;
  })());

  // 3g pointer outside → null (pointer_outside_viewport_is_none). ox+w exclusive.
  check('3g mapPointer outside → null', (() => {
    return mapPointerIntoChild(9, 30, 10, 20, 64, 32, 64, 32) === null &&
           mapPointerIntoChild(74, 30, 10, 20, 64, 32, 64, 32) === null && // ox+w exclusive
           mapPointerIntoChild(30, 52, 10, 20, 64, 32, 64, 32) === null;   // oy+h exclusive
  })());

  // 3h pointer 2x scale halves into child space (pointer_scale_2x...):
  // child 32×16 in a 64×32 viewport at origin; (40,20) → (20,10).
  check('3h mapPointer 2x scale matches Rust', (() => {
    const m = mapPointerIntoChild(40, 20, 0, 0, 64, 32, 32, 16);
    return m && m[0] === 20 && m[1] === 10;
  })());

  // 3i rightmost column clamps inside the child (pointer_rightmost_column...):
  // (63,31) in a 64×32 identity viewport → (63,31), never 64/32.
  check('3i mapPointer rightmost clamps', (() => {
    const m = mapPointerIntoChild(63, 31, 0, 0, 64, 32, 64, 32);
    return m && m[0] === 63 && m[1] === 31;
  })());
}

// ---- 4. ComposeBudget child-count cap (v1 = 8) refuses a 9th mount ----------
{
  worker.composeReset();
  const handles = [];
  for (let i = 0; i < 8; i++) handles.push(worker.composeMountForTest('child', 0, 0, 16, 16));
  check('4a 8 children admitted', handles.every((h) => h >= 0), `handles=${handles.join(',')}`);
  const ninth = worker.composeMountForTest('child', 0, 0, 16, 16);
  check('4b 9th child refused (count cap)', ninth === -1, `ninth=${ninth}`);
  worker.composeReset();
}

// ---- 5. RECURSION: a grandchild composites through TWO levels (the fractal) -
// A green child A (64×64) mounted at parent (0,0,64,64) identity; a red
// grandchild B (32×32) mounted INTO A at (0,0,32,32) identity. After the
// recursive pass, A's rect is green EXCEPT B's quarter, which shows red — proof
// the grandchild's pixels folded child→A→root through two blits.
{
  const GREEN = 0x00ff00, GREENP = packRgb(GREEN);
  const ab = (w) => w.buffer.slice(w.byteOffset, w.byteOffset + w.byteLength);
  const fill = (w, h, rgb) => `fn dims() -> i32 { (${w} << 16) | ${h} }\nfn frame(t: i32) { host::display::clear(0x${rgb.toString(16)}); host::display::present(); }\n`;
  const greenWasm = compileSource(fill(64, 64, GREEN), 'green');
  const redWasm = compileSource(fill(32, 32, RED), 'red');

  const PW = 256, PH = 144;
  worker.composeReset();
  const hA = worker.composeMountForTest('a', 0, 0, 64, 64);
  worker.composeInstantiateForTest(hA, ab(greenWasm));
  const nodeA = worker.composeChildren()[hA];
  check('5a child A READY at 64×64', nodeA && nodeA.state === 1 && nodeA.w === 64, nodeA ? `${nodeA.w}x${nodeA.h}` : 'none');

  // Mount B INTO A (depth 2) and instantiate it against A's table.
  const hB = worker.composeMountInto(nodeA, 'b', 0, 0, 32, 32);
  check('5b grandchild B mounts into A (depth 2)', hB >= 0 && nodeA.children[hB] && nodeA.children[hB].depth === 2,
    nodeA.children[hB] ? `depth=${nodeA.children[hB].depth}` : 'none');
  worker.composeInstantiateForTest(hB, ab(redWasm), nodeA);
  check('5c grandchild B READY at 32×32', nodeA.children[hB].state === 1 && nodeA.children[hB].w === 32);

  const parentFb = new Uint32Array(PW * PH);
  worker.composeRunPass(parentFb, PW, PH, 0, { x: 0, y: 0, down: 0 });

  check('5d grandchild red shows at parent (0,0)', pix(parentFb, PW, 0, 0) === REDP, `0x${pix(parentFb, PW, 0, 0).toString(16)}`);
  check('5e grandchild red fills its quarter', pix(parentFb, PW, 31, 31) === REDP);
  check('5f child green outside the grandchild', pix(parentFb, PW, 40, 40) === GREENP, `0x${pix(parentFb, PW, 40, 40).toString(16)}`);
  check('5g child green at its far corner', pix(parentFb, PW, 63, 63) === GREENP);
  check('5h nothing outside child A', pix(parentFb, PW, 70, 70) === 0);
  worker.composeReset();
}

// ---- 6. DEPTH CAP: the fractal terminates — a node at MAX_DEPTH can't spawn --
{
  worker.composeReset();
  let node = null;
  let lastDepth = 0;
  // Build a chain root→d1→…→d{MAX_DEPTH}; each mount must succeed up to the cap.
  for (let d = 1; d <= worker.COMPOSE_MAX_DEPTH; d++) {
    const h = worker.composeMountInto(node, 'n', 0, 0, 8, 8);
    check(`6a depth-${d} node mounts`, h >= 0, `h=${h}`);
    node = (node ? node.children : worker.composeChildren())[h];
    lastDepth = node.depth;
  }
  check('6b deepest node is at MAX_DEPTH', lastDepth === worker.COMPOSE_MAX_DEPTH, `depth=${lastDepth}`);
  // A node AT the depth cap cannot spawn another level — recursion stops here.
  const over = worker.composeMountInto(node, 'n', 0, 0, 8, 8);
  check('6c node at the depth cap refuses to spawn', over === -1, `over=${over}`);
  worker.composeReset();
}

console.log('');
if (fail === 0) console.log('PASS: cartridge-in-cartridge composition wired (composite + pointer + parity + budget + recursion + depth cap)');
else console.error(`FAIL: ${fail} compose-wiring check(s) failed`);
process.exit(fail === 0 ? 0 : 1);
