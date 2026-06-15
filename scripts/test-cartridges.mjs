#!/usr/bin/env node
// scripts/test-cartridges.mjs — the CARTRIDGE CORPUS gate.
//
// Closes the codegen-validation gap: rustlite's emit unit tests only check the
// wasm MAGIC HEADER, so a cartridge can compile to BAD wasm that traps or
// misbehaves at runtime, undetected (this is part of why complex cartridges
// failed). This harness, for every `examples/cartridges/*.rl`:
//
//   1. COMPILES it to wasm via the CLI (`localharness compile <src> <out>`).
//   2. INSTANTIATES it in node with stubbed host imports — a real Uint32Array
//      framebuffer behind host_display + no-op host_net/host_audio/host_abort,
//      matching the render-*.js host pattern (the present-after-frame model).
//   3. RUNS it: calls frame(t) at several t values (or render()), asserting the
//      instantiate + every call DON'T TRAP.
//   4. ASSERTS: (a) the framebuffer was actually written (drawing cartridges
//      change pixels), (b) animated cartridges produce DIFFERENT frames across
//      t, (c) deterministic compute cartridges land a KNOWN expected pixel/value.
//
// It triples as a codegen regression gate, worked examples for the in-tab agent,
// and proof rustlite builds real things. Run standalone:  node scripts/test-cartridges.mjs
// Wired into verify.sh as a stage. Exits non-zero on any FAIL.
//
// A cartridge that exposes a real CODEGEN BUG (bad wasm that traps / wrong
// pixel) is a VALUABLE find — it is reported precisely, not hidden by deleting
// the cartridge. That is the point of the corpus.

import { execFileSync } from 'node:child_process';
import { readFileSync, mkdirSync, readdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const CORPUS_DIR = join(ROOT, 'examples', 'cartridges');

const FB_W = 256;
const FB_H = 144;

// ---- host: a real framebuffer behind host_display, no-op everything else -----
// Mirrors scripts/render-cartridge.js / render-compose.js: present() is a NO-OP
// and the host owns the framebuffer (present-after-frame). A fresh host per
// instantiation, except `state` which a caller can pass in to persist across
// frames (the counter cartridge needs state to survive between frame() calls).
function makeHost(state = new Map(), ptr = { x: 0, y: 0, down: 0 }) {
  const fb = new Uint8Array(FB_W * FB_H * 4);

  const setPixel = (x, y, rgb) => {
    x |= 0; y |= 0;
    if (x < 0 || y < 0 || x >= FB_W || y >= FB_H) return;
    const i = (y * FB_W + x) * 4;
    fb[i] = (rgb >>> 16) & 255;
    fb[i + 1] = (rgb >>> 8) & 255;
    fb[i + 2] = rgb & 255;
    fb[i + 3] = 255;
  };
  const fillRect = (x, y, w, h, rgb) => {
    const x0 = Math.max(0, x | 0), y0 = Math.max(0, y | 0);
    const x1 = Math.min(FB_W, (x | 0) + (w | 0)), y1 = Math.min(FB_H, (y | 0) + (h | 0));
    for (let yy = y0; yy < y1; yy++) for (let xx = x0; xx < x1; xx++) setPixel(xx, yy, rgb);
  };
  // Bresenham line — enough to mark the touched pixels (the timer draws lines).
  const drawLine = (x0, y0, x1, y1, rgb) => {
    x0 |= 0; y0 |= 0; x1 |= 0; y1 |= 0;
    const dx = Math.abs(x1 - x0), dy = -Math.abs(y1 - y0);
    const sx = x0 < x1 ? 1 : -1, sy = y0 < y1 ? 1 : -1;
    let err = dx + dy;
    for (;;) {
      setPixel(x0, y0, rgb);
      if (x0 === x1 && y0 === y1) break;
      const e2 = 2 * err;
      if (e2 >= dy) { err += dy; x0 += sx; }
      if (e2 <= dx) { err += dx; y0 += sy; }
    }
  };
  // 5x7 glyph stub: light a few pixels per cell so a number/char visibly draws
  // (fidelity is covered by test-worker-host-parity.mjs; here we only need that
  // SOMETHING lands, so a drawn label counts as a non-blank contribution).
  const drawCell = (x, y, rgb, scale) => {
    const s = Math.max(1, scale | 0);
    fillRect(x, y, 5 * s, 7 * s, rgb);
  };

  const host_display = {
    clear: (rgb) => fillRect(0, 0, FB_W, FB_H, rgb),
    set_pixel: setPixel,
    fill_rect: fillRect,
    draw_char: (x, y, _code, rgb, scale) => drawCell(x, y, rgb, scale),
    draw_number: (x, y, value, rgb, scale) => {
      // Mark one cell per digit (advance 6*scale) so multi-digit numbers draw.
      const s = Math.max(1, scale | 0);
      const adv = 6 * s;
      let n = Math.abs(value | 0);
      let cx = x | 0;
      if ((value | 0) < 0) { drawCell(cx, y, rgb, s); cx += adv; }
      const digits = n === 0 ? 1 : Math.floor(Math.log10(n)) + 1;
      for (let d = 0; d < digits; d++) { drawCell(cx, y, rgb, s); cx += adv; }
    },
    draw_line: drawLine,
    fill_triangle: (ax, ay, bx, by, cx, cy, rgb) => {
      // A coarse bounding-box stub is enough to register touched pixels.
      const minx = Math.min(ax, bx, cx), maxx = Math.max(ax, bx, cx);
      const miny = Math.min(ay, by, cy), maxy = Math.max(ay, by, cy);
      for (let y = miny; y <= maxy; y++) for (let x = minx; x <= maxx; x++) setPixel(x, y, rgb);
    },
    present: () => {}, // NO-OP — host presents after frame() (snapshot the fb).
    width: () => FB_W,
    height: () => FB_H,
    pointer_x: () => ptr.x,
    pointer_y: () => ptr.y,
    pointer_down: () => ptr.down,
    state_get: (s) => state.get(s | 0) || 0,
    state_set: (s, v) => { state.set(s | 0, v | 0); },
  };
  return { fb, host_display, state, ptr };
}

// Instantiate a cartridge module with a given host. Any other import module
// (host_net, host_audio, …) gets stubbed: functions return 0, memory is a fresh
// page — so a cartridge that imports them still instantiates (none of the corpus
// does, but this keeps the harness general, like render-cartridge.js).
function instantiate(wasmBytes, host) {
  const mod = new WebAssembly.Module(wasmBytes);
  const importObj = {};
  for (const imp of WebAssembly.Module.imports(mod)) {
    importObj[imp.module] = importObj[imp.module] || {};
    if (imp.module === 'host_display') {
      importObj.host_display = host.host_display;
    } else if (imp.kind === 'function') {
      importObj[imp.module][imp.name] = () => 0;
    } else if (imp.kind === 'memory') {
      importObj[imp.module][imp.name] = new WebAssembly.Memory({ initial: 1 });
    }
  }
  const inst = new WebAssembly.Instance(mod, importObj);
  const entry = inst.exports.frame || inst.exports.render;
  return { inst, entry };
}

function litPixels(fb) {
  let n = 0;
  for (let i = 0; i < fb.length; i += 4) {
    if (fb[i] || fb[i + 1] || fb[i + 2]) n++;
  }
  return n;
}
function pixelRGB(fb, x, y) {
  const i = (y * FB_W + x) * 4;
  return [fb[i], fb[i + 1], fb[i + 2]];
}
function eqRGB(a, b) {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}
function framesDiffer(a, b) {
  if (a.length !== b.length) return true;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return true;
  return false;
}

// ---- per-cartridge expectations -------------------------------------------
//
// Each entry: { needsState, check(api) -> {ok, detail} }. `api` exposes
// `renderAt(t)` (fresh host unless `needsState`), `litPixels`, `pixelRGB`,
// `eqRGB`, the shared `state` map (when needsState), and FB dims. A cartridge
// not listed gets the default check (non-blank at t=0).

const SPECS = {
  // v = ((7*6)+100)/2 - 3%4 = 68 -> clear 0x000044. The cleared pixel anywhere
  // (pick a corner) must be opaque [0,0,68].
  'arithmetic.rl': (api) => {
    const fb = api.renderAt(0);
    const px = api.pixelRGB(fb, 0, 0);
    if (!api.eqRGB(px, [0, 0, 68])) {
      return { ok: false, detail: `expected cleared pixel [0,0,68] (v=68), got [${px}]` };
    }
    return { ok: true, detail: 'compute = 68, clear colour 0x000044 confirmed' };
  },

  // while-sum(1..=10) == for-sum(0..10) == 55. Agreement -> clear 0x000037;
  // disagreement -> red 0xFF0000. Pin the cleared pixel to 55.
  'loops.rl': (api) => {
    const fb = api.renderAt(0);
    const px = api.pixelRGB(fb, 0, 0);
    if (api.eqRGB(px, [255, 0, 0])) {
      return { ok: false, detail: 'while-loop and for-loop sums DISAGREE (sentinel red painted)' };
    }
    if (!api.eqRGB(px, [0, 0, 55])) {
      return { ok: false, detail: `expected clear [0,0,55] (sum=55), got [${px}]` };
    }
    return { ok: true, detail: 'while and for both sum to 55 (loop counts match)' };
  },

  // match arms paint four bands. Pin one pixel per band to its arm's colour.
  'match_ranges.rl': (api) => {
    const fb = api.renderAt(0);
    const blue = api.pixelRGB(fb, 10, 10);    // band 0..36 -> classify(3) 0..=5
    const green = api.pixelRGB(fb, 10, 50);   // band 36..72 -> classify(7) 6..10
    const red = api.pixelRGB(fb, 10, 90);     // band 72..108 -> classify(42) _
    const white = api.pixelRGB(fb, 10, 130);  // band 108..144 -> classify(100) lit
    const want = [
      ['inclusive 0..=5 -> blue', blue, [0, 0, 255]],
      ['exclusive 6..10 -> green', green, [0, 255, 0]],
      ['wildcard _ -> red', red, [255, 0, 0]],
      ['literal 100 -> white', white, [255, 255, 255]],
    ];
    for (const [label, got, exp] of want) {
      if (!api.eqRGB(got, exp)) {
        return { ok: false, detail: `${label}: expected [${exp}], got [${got}]` };
      }
    }
    return { ok: true, detail: 'all four match arms (literal/incl-range/excl-range/wildcard) correct' };
  },

  // arrays: t=0 clears to palette[0]=red. Bars + marker make it non-blank; the
  // marker height encodes total=100 (total/5 = 20px). Pin red clear + a marker
  // pixel at y=100+10 (inside the 20px marker) being white.
  'arrays.rl': (api) => {
    const fb = api.renderAt(0);
    const clear = api.pixelRGB(fb, 200, 10); // top-right, away from bars/marker
    if (!api.eqRGB(clear, [255, 0, 0])) {
      return { ok: false, detail: `palette[0] clear: expected red [255,0,0], got [${clear}]` };
    }
    // Marker rect is fill_rect(0,100,8, total/5=20, white). Pixel (2,110) inside.
    const marker = api.pixelRGB(fb, 2, 110);
    if (!api.eqRGB(marker, [255, 255, 255])) {
      return { ok: false, detail: `width-sum marker (total=100 -> 20px tall): expected white at (2,110), got [${marker}]` };
    }
    // t=1 must select palette[1]=green as the clear colour (variable index).
    const fb1 = api.renderAt(1);
    const clear1 = api.pixelRGB(fb1, 200, 10);
    if (!api.eqRGB(clear1, [0, 255, 0])) {
      return { ok: false, detail: `palette[t%4] at t=1: expected green [0,255,0], got [${clear1}]` };
    }
    return { ok: true, detail: 'array reads: const-index bars + var-index palette + summed widths (100) all correct' };
  },

  // recursion: fact(5)=120, fib(10)=55. clear = fib*256 + fact = 55*256+120
  // = 14200 = 0x003778 -> [0x00, 0x37, 0x78] = [0,55,120].
  'recursion.rl': (api) => {
    const fb = api.renderAt(0);
    const px = api.pixelRGB(fb, 0, 0);
    if (!api.eqRGB(px, [0, 55, 120])) {
      return { ok: false, detail: `expected [0,55,120] (fib(10)=55, fact(5)=120), got [${px}]` };
    }
    return { ok: true, detail: 'fact(5)=120 and fib(10)=55 — recursive call/return sound' };
  },

  // timer: animated. Non-blank every frame + the framebuffer at t=0 differs
  // from t=30 (it actually animates).
  'timer.rl': (api) => {
    const f0 = api.renderAt(0);
    const f30 = api.renderAt(30);
    if (api.litPixels(f0) === 0) return { ok: false, detail: 'blank at t=0' };
    if (api.litPixels(f30) === 0) return { ok: false, detail: 'blank at t=30' };
    if (!framesDiffer(f0, f30)) {
      return { ok: false, detail: 'framebuffer identical at t=0 and t=30 — frame(t) is not animating' };
    }
    return { ok: true, detail: `animates: ${api.litPixels(f0)} lit @t0, ${api.litPixels(f30)} lit @t30, frames differ` };
  },

  // bouncing ball: animated, deterministic position. At t=0 the ball's top-left
  // sits at (8,8) (triangle(0,*)=0). Pin a pixel inside the 12x12 ball + assert
  // motion between t=0 and t=20.
  'bouncing_ball.rl': (api) => {
    const f0 = api.renderAt(0);
    const ball = api.pixelRGB(f0, 12, 12); // inside the 12x12 ball at (8..20,8..20)
    if (!api.eqRGB(ball, [255, 255, 255])) {
      return { ok: false, detail: `ball at start corner: expected white at (12,12), got [${ball}]` };
    }
    const f20 = api.renderAt(20);
    if (!framesDiffer(f0, f20)) {
      return { ok: false, detail: 'ball does not move between t=0 and t=20' };
    }
    return { ok: true, detail: 'ball starts at (8,8) and moves with t (triangle-wave bounce)' };
  },

  // life: STATEFUL Conway's Game of Life on an 8x8 grid in the 64 state slots
  // (slot = y*8 + x). frame(0) seeds a HORIZONTAL blinker at (2,4),(3,4),(4,4)
  // then steps one generation; each later frame steps once more. The blinker is
  // a PERIOD-2 oscillator, so reading the slots back after each frame:
  //   after frame(0): VERTICAL   — column x=3, rows y=3,4,5 -> slots 27,35,43
  //   after frame(1): HORIZONTAL — row y=4, cols x=2,3,4    -> slots 34,35,36
  //   after frame(2): VERTICAL again (period 2)
  // and the live-cell COUNT is exactly 3 at every step. This is a deterministic
  // Game-of-Life correctness check exercising indexed array writes (next[i]=v),
  // array reads, the state slots, and nested loops — not "it drew something".
  'life.rl': {
    needsState: true,
    check: (api) => {
      // Read the 64 slots into the live-cell index list + count.
      const liveCells = () => {
        const cells = [];
        for (let i = 0; i < 64; i++) if ((api.state.get(i) | 0) === 1) cells.push(i);
        return cells;
      };
      const sameSet = (got, want) =>
        got.length === want.length && want.every((v, i) => got[i] === v);

      const VERT = [27, 35, 43]; // x=3, y=3,4,5
      const HORIZ = [34, 35, 36]; // y=4, x=2,3,4

      // frame(0): seed horizontal + step once -> VERTICAL.
      const fb0 = api.renderAt(0);
      const g0 = liveCells();
      if (g0.length !== 3) {
        return { ok: false, detail: `gen after frame(0): expected 3 live cells, got ${g0.length} ([${g0}]) — array writes / rules wrong` };
      }
      if (!sameSet(g0, VERT)) {
        return { ok: false, detail: `gen after frame(0): expected VERTICAL blinker [${VERT}], got [${g0}]` };
      }
      if (api.litPixels(fb0) === 0) {
        return { ok: false, detail: 'grid drew nothing at frame(0)' };
      }
      // A live cell at (3,3) -> pixel inside its 16px block (px=48..63, py=48..63).
      const liveBlock = api.pixelRGB(fb0, 52, 52);
      if (!api.eqRGB(liveBlock, [0, 255, 0])) {
        return { ok: false, detail: `live cell (3,3) block: expected green [0,255,0] at (52,52), got [${liveBlock}]` };
      }

      // frame(1): step again -> HORIZONTAL (period 2, half-way).
      api.renderAt(1);
      const g1 = liveCells();
      if (g1.length !== 3 || !sameSet(g1, HORIZ)) {
        return { ok: false, detail: `gen after frame(1): expected HORIZONTAL blinker [${HORIZ}] (count 3), got [${g1}] (count ${g1.length})` };
      }

      // frame(2): step again -> VERTICAL (full period-2 cycle complete).
      api.renderAt(2);
      const g2 = liveCells();
      if (g2.length !== 3 || !sameSet(g2, VERT)) {
        return { ok: false, detail: `gen after frame(2): expected VERTICAL blinker [${VERT}] again (period 2), got [${g2}] (count ${g2.length})` };
      }

      return { ok: true, detail: 'blinker oscillates H->V->H->V across frames (period 2, 3 cells) — array-write next-gen correct' };
    },
  },

  // counter: STATEFUL. Drive frame() N times against ONE shared state map; the
  // stored slot 0 must equal N (it increments per call, surviving across frames).
  'counter.rl': {
    needsState: true,
    check: (api) => {
      // api.renderAt here shares api.state across calls (needsState).
      api.renderAt(0);
      api.renderAt(1);
      const fb3 = api.renderAt(2); // 3rd call total
      const stored = api.state.get(0);
      if (stored !== 3) {
        return { ok: false, detail: `state[0] expected 3 after 3 frames, got ${stored} — state_get/set not persisting` };
      }
      if (api.litPixels(fb3) === 0) {
        return { ok: false, detail: 'blank after 3 frames' };
      }
      // The bar at count=3 is fill_rect(0,60, 12, 30, green). Pin (4,70).
      const bar = api.pixelRGB(fb3, 4, 70);
      if (!api.eqRGB(bar, [0, 255, 0])) {
        return { ok: false, detail: `count bar: expected green at (4,70), got [${bar}]` };
      }
      return { ok: true, detail: 'state[0] increments 1->2->3 across frames; bar tracks the count' };
    },
  },

  // shadow: REGRESSION guard for the local-allocation bug. A name declared in an
  // inner block AND again in the body, plus a same-scope self-ref shadow, pack
  // p=11,q=22,r=4 into the clear colour. A slot collision (old bug) or a wrong
  // emit order would regress this pixel away from [11,22,4].
  'shadow.rl': (api) => {
    const fb = api.renderAt(0);
    const px = api.pixelRGB(fb, 0, 0);
    if (!api.eqRGB(px, [11, 22, 4])) {
      return { ok: false, detail: `expected [11,22,4] (p=11,q=22 distinct slots; r=3->4 self-ref), got [${px}] — shadowing miscompiled` };
    }
    return { ok: true, detail: 'shadowed lets get distinct slots; self-ref shadow reads the old value' };
  },
};

// Default check for any cartridge without an explicit spec: must be non-blank.
function defaultCheck(api) {
  const fb = api.renderAt(0);
  const lit = api.litPixels(fb);
  if (lit === 0) return { ok: false, detail: 'framebuffer blank at t=0 (rendered nothing)' };
  return { ok: true, detail: `non-blank (${lit} lit pixels)` };
}

// ---- driver ---------------------------------------------------------------

// Compile one cartridge via the CLI. cargo's stderr (which replays the crate's
// cached build warnings on EVERY `cargo run`, even when nothing rebuilds) is
// captured, not streamed — it's printed ONLY if the compile fails, so a genuine
// rustlite compile error in a cartridge still surfaces while the unrelated
// crate warning never drowns the PASS/FAIL lines.
function compile(src, out) {
  try {
    execFileSync(
      'cargo',
      ['run', '--quiet', '--features', 'wallet', '--bin', 'localharness', '--', 'compile', src, out],
      { cwd: ROOT, stdio: ['ignore', 'ignore', 'pipe'] },
    );
  } catch (err) {
    const stderr = err.stderr ? err.stderr.toString() : '';
    throw new Error(`cartridge failed to compile:\n${stderr.trim()}`);
  }
}

function run() {
  mkdirSync(join(ROOT, 'target'), { recursive: true });

  const files = readdirSync(CORPUS_DIR)
    .filter((f) => f.endsWith('.rl'))
    .sort();
  if (files.length === 0) {
    console.error(`no cartridges found in ${CORPUS_DIR}`);
    process.exit(2);
  }

  console.log(`CARTRIDGE CORPUS — ${files.length} cartridges (compile -> instantiate -> run -> assert)\n`);

  let pass = 0;
  let fail = 0;

  for (const file of files) {
    const src = join(CORPUS_DIR, file);
    const out = join(ROOT, 'target', `.corpus-${file}.wasm`);
    let result;
    try {
      // 1. compile
      compile(src, out);
      const wasmBytes = readFileSync(out);

      // Build the spec (object or function form).
      const rawSpec = SPECS[file];
      const needsState = !!(rawSpec && rawSpec.needsState);
      const checkFn = typeof rawSpec === 'function' ? rawSpec : (rawSpec && rawSpec.check) || defaultCheck;

      // Shared state when the cartridge persists across frames (counter).
      const sharedState = new Map();

      // renderAt(t): instantiate (fresh host unless stateful) + call frame(t),
      // snapshot the fb. Instantiation OR the call trapping throws -> caught
      // below and reported as a runtime trap (the codegen-bug signal).
      const renderAt = (t) => {
        const host = needsState ? makeHost(sharedState) : makeHost();
        const { entry } = instantiate(wasmBytes, host);
        if (typeof entry !== 'function') {
          throw new Error('no frame()/render() export');
        }
        entry(t | 0);
        return host.fb;
      };

      const api = {
        renderAt,
        litPixels,
        pixelRGB,
        eqRGB,
        state: sharedState,
        FB_W,
        FB_H,
      };

      // 2-4. run + assert
      result = checkFn(api);
    } catch (err) {
      // Instantiation or a frame() call trapped — the high-value codegen-bug
      // signal. Report it precisely (which cartridge, the trap message).
      result = { ok: false, detail: `TRAP / runtime error: ${err.message}` };
    }

    if (result.ok) {
      pass++;
      console.log(`  PASS  ${file.padEnd(20)} ${result.detail}`);
    } else {
      fail++;
      console.error(`  FAIL  ${file.padEnd(20)} ${result.detail}`);
    }
  }

  console.log(`\nSUMMARY: ${pass}/${files.length} passed${fail ? `, ${fail} FAILED` : ''}.`);
  if (fail > 0) {
    console.error('CARTRIDGE CORPUS FAILED — a cartridge compiled to bad/wrong wasm (see FAIL lines above).');
    process.exit(1);
  }
  console.log('CARTRIDGE CORPUS OK — every cartridge compiles, instantiates, runs without trapping, and draws/computes correctly.');
}

run();
