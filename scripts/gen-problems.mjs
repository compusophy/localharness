#!/usr/bin/env node
// scripts/gen-problems.mjs — the VERIFIED-DATASET generator harness.
//
// WHY THIS EXISTS (the moat):
//   localharness OWNS a real RLVR verifier — the rustlite compiler + verify.sh
//   actually COMPILE, INSTANTIATE, and RENDER `.rl` cartridges and assert real
//   pixels. Most "coding datasets" are scraped text with no executable oracle.
//   Here every example can be MECHANICALLY checked: a candidate solution is
//   only kept if its reference `.rl` COMPILES via the rustlite compiler AND its
//   tests PASS against the rendered framebuffer. That verifier is the moat — no
//   one else has it — and this harness turns it into a {problem, reference,
//   tests} dataset suitable for training/distilling an own-coding-model
//   (reinforcement learning from verifiable rewards: the reward = "passes the
//   gate", which we can compute for free at scale).
//
// PIPELINE (per problem spec):
//   1. ASK a teacher model (default: Claude Opus via the credit proxy, or a
//      direct Anthropic API call when an API key env is set) to emit a candidate
//      reference `.rl` + tests for the problem statement. The model call is
//      PLUGGABLE + STUBBED (see `callTeacher`): it needs a key/proxy at run
//      time, so this file ships with a clear interface and a stub that refuses
//      to fabricate — you wire a real client in to actually generate.
//   2. COMPILE the candidate `.rl` via the real rustlite compiler:
//        cargo run --features wallet --bin localharness -- compile <src> <out>
//      (the SAME command verify.sh stage 3 + test-cartridges.mjs use).
//   3. VALIDATE: instantiate the wasm with a stubbed host framebuffer and run
//      frame(t) without trapping, then RUN the problem's tests (pixel / state /
//      no-trap assertions) against the rendered framebuffer. This mirrors
//      verify.sh stages 4-5 + the test-cartridges.mjs corpus oracle.
//   4. KEEP only triples that pass every check; write them under
//      datasets/rustlite-problems/problems/<id>/.
//
// USAGE:
//   node scripts/gen-problems.mjs --verify-seeds      # re-verify the seed set
//   node scripts/gen-problems.mjs --gen specs.json    # generate from specs
//                                                     # (needs a teacher key)
//   node scripts/gen-problems.mjs --help
//
// The generation path requires a teacher model; the --verify-seeds path is
// fully self-contained (compiler + node only) and is what proves the format +
// the gate end to end. See datasets/rustlite-problems/README.md.

import { execFileSync } from 'node:child_process';
import {
  readFileSync, writeFileSync, mkdirSync, readdirSync, existsSync,
} from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');
const PROBLEMS_DIR = join(ROOT, 'datasets', 'rustlite-problems', 'problems');

const FB_W = 256;
const FB_H = 144;

// ===========================================================================
// 1. THE TEACHER-MODEL INTERFACE (pluggable + stubbed).
// ===========================================================================
//
// `callTeacher(problem)` must return `{ rl, tests }` — a candidate reference
// `.rl` source string and a tests object (same shape as a seed's tests.json).
// Two real backends are sketched; both need a key/proxy at run time which this
// worktree does NOT have, so the default STUB throws rather than fabricate.
//
// To wire a real teacher in, set ONE of:
//   - ANTHROPIC_API_KEY  -> direct Claude Messages API (model below).
//   - LH_PROXY_URL + a signer  -> the localharness $LH credit proxy
//     (api/gemini.ts multi-provider passthrough), or shell out to the
//     `localharness call` CLI which routes a headless turn through the proxy.
//
// The prompt hands the model: the problem statement, the rustlite language +
// host-ABI cheat-sheet (RUSTLITE_SYSTEM below), and the worked seed examples as
// few-shot context. The model returns a fenced `.rl` block + a JSON tests block.

const TEACHER_MODEL = 'claude-opus-4-8'; // teacher; verify against the live API.

const RUSTLITE_SYSTEM = `You write "rustlite" cartridges: a small Rust subset that compiles to wasm and
draws on a 256x144 host-owned framebuffer. Output ONE fenced \`\`\`rust block (the
.rl source) then ONE fenced \`\`\`json block (the tests).

LANGUAGE: fn defs with i32 params/returns; let / let mut; i32 only; + - * / %;
comparisons; && ||; if/else (as expressions too); while; for i in 0..n; match
(literal / lo..=hi / lo..hi / _ arms); recursion; arrays [i32; N], literals
[a,b,c], repeat-init [0; N], indexed read/write xs[i], array params (by base
pointer). NO globals, NO strings except string literals passed to host fns, NO
floats, NO structs/enums-with-data, NO standard library.

ENTRY POINT: fn frame(t: i32) (or fn render()). Persist state ONLY in the 64
host state slots via state_get/state_set.

HOST ABI (all under host::display:: unless noted):
  clear(rgb); set_pixel(x,y,rgb); fill_rect(x,y,w,h,rgb);
  draw_char(x,y,charcode,rgb,scale); draw_number(x,y,value,rgb,scale);
  draw_line(x0,y0,x1,y1,rgb); fill_triangle(x0,y0,x1,y1,x2,y2,rgb);
  present(); width()->i32; height()->i32;
  pointer_x()/pointer_y()/pointer_down()->i32;
  state_get(slot)->i32; state_set(slot,value);
  host::http::get(urlLiteral, len)->handle; ready(h); status(h); body_len(h);
  host::net / host::audio / host::agent (poll/fire-and-forget, integer ABI).
Colours are 0xRRGGBB packed into an i32. Always call present() last.`;

// The DEFAULT stub: no key available in this environment. Throws a clear error
// rather than fabricate an unverified solution (a bad triple is worthless).
async function callTeacherStub(problem) {
  throw new Error(
    `teacher model not wired: callTeacher is stubbed. Set ANTHROPIC_API_KEY ` +
    `(direct Claude) or LH_PROXY_URL + a signer (the $LH proxy) and replace ` +
    `callTeacherStub with a real client. Problem was "${problem.id}". The ` +
    `--verify-seeds path needs no model and works now.`,
  );
}

// A direct Anthropic-API teacher (only used when ANTHROPIC_API_KEY is set).
// Sketched against the Claude Messages API shape; left behind the key check so
// it never runs (and never fabricates) without an explicit key.
async function callTeacherAnthropic(problem) {
  const key = process.env.ANTHROPIC_API_KEY;
  if (!key) throw new Error('ANTHROPIC_API_KEY not set');
  const body = {
    model: process.env.LH_TEACHER_MODEL || TEACHER_MODEL,
    max_tokens: 2048,
    system: RUSTLITE_SYSTEM,
    messages: [{ role: 'user', content: renderPrompt(problem) }],
  };
  const res = await fetch('https://api.anthropic.com/v1/messages', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-api-key': key,
      'anthropic-version': '2023-06-01',
    },
    body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`teacher API ${res.status}: ${await res.text()}`);
  const json = await res.json();
  const text = (json.content || []).map((c) => c.text || '').join('\n');
  return parseTeacherReply(text);
}

// Dispatch: prefer a wired backend, else the stub. Swap freely.
async function callTeacher(problem) {
  if (process.env.ANTHROPIC_API_KEY) return callTeacherAnthropic(problem);
  // (LH_PROXY_URL path: shell out to `localharness call` or POST api/gemini.ts —
  //  left as a wiring point; same parseTeacherReply contract.)
  return callTeacherStub(problem);
}

function renderPrompt(problem) {
  const fewShot = loadFewShot();
  return [
    'Write a rustlite cartridge solving this problem, plus tests.',
    '',
    `PROBLEM (${problem.id}):`,
    problem.statement,
    '',
    'CONSTRAINTS:',
    ...(problem.constraints || []).map((c) => `- ${c}`),
    '',
    'Tests JSON shape (a list of checks; see examples): pixel_at / pixel_after_frames',
    '(expect_rgb at x,y), state_after_frames (slot == expect), no_trap, non_blank.',
    '',
    'WORKED EXAMPLES:',
    fewShot,
  ].join('\n');
}

// Load up to two existing seed triples as few-shot exemplars for the teacher.
function loadFewShot() {
  if (!existsSync(PROBLEMS_DIR)) return '(none)';
  const ids = readdirSync(PROBLEMS_DIR).filter((d) =>
    existsSync(join(PROBLEMS_DIR, d, 'reference.rl'))).sort().slice(0, 2);
  return ids.map((id) => {
    const rl = readFileSync(join(PROBLEMS_DIR, id, 'reference.rl'), 'utf8');
    const tests = readFileSync(join(PROBLEMS_DIR, id, 'tests.json'), 'utf8');
    return `--- ${id} ---\n\`\`\`rust\n${rl}\`\`\`\n\`\`\`json\n${tests}\`\`\``;
  }).join('\n\n') || '(none)';
}

// Pull the ```rust .rl block + ```json tests block out of a model reply.
function parseTeacherReply(text) {
  const rl = (text.match(/```(?:rust|rl)?\s*\n([\s\S]*?)```/) || [])[1];
  const jsonBlocks = [...text.matchAll(/```json\s*\n([\s\S]*?)```/g)];
  const testsRaw = jsonBlocks.length ? jsonBlocks[jsonBlocks.length - 1][1] : null;
  if (!rl || !testsRaw) {
    throw new Error('teacher reply missing a ```rust or ```json block');
  }
  return { rl: rl.trimEnd() + '\n', tests: JSON.parse(testsRaw) };
}

// ===========================================================================
// 2. COMPILE — the real rustlite compiler (verify.sh stage 3 command).
// ===========================================================================

function compile(src, out) {
  try {
    execFileSync(
      'cargo',
      ['run', '--quiet', '--features', 'wallet', '--bin', 'localharness',
        '--', 'compile', src, out],
      { cwd: ROOT, stdio: ['ignore', 'ignore', 'pipe'] },
    );
  } catch (err) {
    const stderr = err.stderr ? err.stderr.toString() : '';
    throw new Error(`rustlite compile failed:\n${stderr.trim()}`);
  }
}

// ===========================================================================
// 3. VALIDATE + RUN TESTS — instantiate with a real framebuffer host, run
//    frame(t), assert the problem's checks. Mirrors test-cartridges.mjs.
// ===========================================================================

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
  const drawCell = (x, y, rgb, scale) => fillRect(x, y, 5 * Math.max(1, scale | 0), 7 * Math.max(1, scale | 0), rgb);
  const host_display = {
    clear: (rgb) => fillRect(0, 0, FB_W, FB_H, rgb),
    set_pixel: setPixel,
    fill_rect: fillRect,
    draw_char: (x, y, _c, rgb, scale) => drawCell(x, y, rgb, scale),
    draw_number: (x, y, value, rgb, scale) => {
      const s = Math.max(1, scale | 0);
      const adv = 6 * s;
      const n = Math.abs(value | 0);
      let cx = x | 0;
      if ((value | 0) < 0) { drawCell(cx, y, rgb, s); cx += adv; }
      const digits = n === 0 ? 1 : Math.floor(Math.log10(n)) + 1;
      for (let d = 0; d < digits; d++) { drawCell(cx, y, rgb, s); cx += adv; }
    },
    draw_line: drawLine,
    fill_triangle: (ax, ay, bx, by, cx, cy, rgb) => {
      const minx = Math.min(ax, bx, cx), maxx = Math.max(ax, bx, cx);
      const miny = Math.min(ay, by, cy), maxy = Math.max(ay, by, cy);
      for (let y = miny; y <= maxy; y++) for (let x = minx; x <= maxx; x++) setPixel(x, y, rgb);
    },
    present: () => {},
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

const pixelRGB = (fb, x, y) => {
  const i = (y * FB_W + x) * 4;
  return [fb[i], fb[i + 1], fb[i + 2]];
};
const eqRGB = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);
const litPixels = (fb) => {
  let n = 0;
  for (let i = 0; i < fb.length; i += 4) if (fb[i] || fb[i + 1] || fb[i + 2]) n++;
  return n;
};

// Run the tests.json checks against a compiled wasm. Returns {ok, detail}.
// Supported check kinds:
//   pixel_at            {frame, x, y, expect_rgb}
//   pixel_after_frames  {frames:[...], x, y, expect_rgb}  (shared state)
//   state_after_frames  {frames:[...], slot, expect}      (shared state)
//   no_trap             {frames:[...]}
//   non_blank           {frames:[...]}
function runTests(wasmBytes, tests) {
  const needsState = !!tests.needs_state;
  // Per-check evaluation. Stateful checks share ONE state map across the frame
  // list; stateless pixel_at uses a fresh host per call.
  const driveShared = (frames) => {
    const state = new Map();
    let fb = null;
    for (const t of frames) {
      const host = makeHost(state);
      const { entry } = instantiate(wasmBytes, host);
      if (typeof entry !== 'function') throw new Error('no frame()/render() export');
      entry(t | 0);
      fb = host.fb;
    }
    return { fb, state };
  };
  const driveOnce = (t) => {
    const host = makeHost(needsState ? new Map() : undefined);
    const { entry } = instantiate(wasmBytes, host);
    if (typeof entry !== 'function') throw new Error('no frame()/render() export');
    entry(t | 0);
    return host;
  };

  for (const c of tests.checks || []) {
    try {
      if (c.kind === 'pixel_at') {
        const host = driveOnce(c.frame | 0);
        const got = pixelRGB(host.fb, c.x | 0, c.y | 0);
        if (!eqRGB(got, c.expect_rgb)) {
          return { ok: false, detail: `pixel_at(${c.x},${c.y})@${c.frame}: expected [${c.expect_rgb}], got [${got}]` };
        }
      } else if (c.kind === 'pixel_after_frames') {
        const { fb } = driveShared(c.frames);
        const got = pixelRGB(fb, c.x | 0, c.y | 0);
        if (!eqRGB(got, c.expect_rgb)) {
          return { ok: false, detail: `pixel after [${c.frames}] at (${c.x},${c.y}): expected [${c.expect_rgb}], got [${got}]` };
        }
      } else if (c.kind === 'state_after_frames') {
        const { state } = driveShared(c.frames);
        const got = state.get(c.slot | 0) | 0;
        if (got !== (c.expect | 0)) {
          return { ok: false, detail: `state[${c.slot}] after [${c.frames}]: expected ${c.expect}, got ${got}` };
        }
      } else if (c.kind === 'no_trap') {
        driveShared(c.frames); // throws on a trap -> caught below
      } else if (c.kind === 'non_blank') {
        const { fb } = driveShared(c.frames);
        if (litPixels(fb) === 0) return { ok: false, detail: `blank after [${c.frames}]` };
      } else {
        return { ok: false, detail: `unknown check kind "${c.kind}"` };
      }
    } catch (err) {
      return { ok: false, detail: `TRAP / error in ${c.kind}: ${err.message}` };
    }
  }
  return { ok: true, detail: `${(tests.checks || []).length} checks passed` };
}

// ===========================================================================
// THE GATE: compile + validate + run tests. Returns {ok, detail}. This is the
// reusable "verifiable reward" — true == this triple is dataset-worthy.
// ===========================================================================
function verifyTriple(id, rl, tests) {
  mkdirSync(join(ROOT, 'target'), { recursive: true });
  const src = join(ROOT, 'target', `.gen-${id}.rl`);
  const out = join(ROOT, 'target', `.gen-${id}.wasm`);
  writeFileSync(src, rl);
  try {
    compile(src, out);
  } catch (err) {
    return { ok: false, detail: err.message };
  }
  const wasmBytes = readFileSync(out);
  return runTests(wasmBytes, tests);
}

// ===========================================================================
// DRIVERS
// ===========================================================================

// Re-verify every seed triple already on disk (no teacher model needed).
function verifySeeds() {
  if (!existsSync(PROBLEMS_DIR)) {
    console.error(`no problems dir: ${PROBLEMS_DIR}`);
    process.exit(2);
  }
  const ids = readdirSync(PROBLEMS_DIR)
    .filter((d) => existsSync(join(PROBLEMS_DIR, d, 'reference.rl')))
    .sort();
  if (ids.length === 0) {
    console.error('no seed triples found');
    process.exit(2);
  }
  console.log(`VERIFIED-DATASET GATE — ${ids.length} seed triples (compile -> instantiate -> run -> assert)\n`);
  let pass = 0, fail = 0;
  for (const id of ids) {
    const rl = readFileSync(join(PROBLEMS_DIR, id, 'reference.rl'), 'utf8');
    const tests = JSON.parse(readFileSync(join(PROBLEMS_DIR, id, 'tests.json'), 'utf8'));
    const r = verifyTriple(id, rl, tests);
    if (r.ok) { pass++; console.log(`  PASS  ${id.padEnd(26)} ${r.detail}`); }
    else { fail++; console.error(`  FAIL  ${id.padEnd(26)} ${r.detail}`); }
  }
  console.log(`\nSUMMARY: ${pass}/${ids.length} triples pass the gate${fail ? `, ${fail} FAILED` : ''}.`);
  if (fail) {
    console.error('SEED SET FAILED the verifier — a triple does not compile/render to spec.');
    process.exit(1);
  }
  console.log('SEED SET OK — every triple compiles, instantiates, runs, and matches its tests.');
}

// Generate new triples from a specs file: [{id, statement, constraints, ...}].
// Asks the teacher, runs the gate, KEEPS only passers (writes the triple).
async function generate(specsPath) {
  const specs = JSON.parse(readFileSync(specsPath, 'utf8'));
  console.log(`GENERATING ${specs.length} problems via teacher "${TEACHER_MODEL}" + the verify gate\n`);
  let kept = 0, dropped = 0;
  for (const problem of specs) {
    let candidate;
    try {
      candidate = await callTeacher(problem); // {rl, tests}
    } catch (err) {
      dropped++; console.error(`  DROP  ${problem.id.padEnd(26)} teacher error: ${err.message}`); continue;
    }
    const r = verifyTriple(problem.id, candidate.rl, candidate.tests);
    if (!r.ok) { dropped++; console.error(`  DROP  ${problem.id.padEnd(26)} gate: ${r.detail}`); continue; }
    const dir = join(PROBLEMS_DIR, problem.id);
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, 'problem.json'), JSON.stringify(problem, null, 2) + '\n');
    writeFileSync(join(dir, 'reference.rl'), candidate.rl);
    writeFileSync(join(dir, 'tests.json'), JSON.stringify(candidate.tests, null, 2) + '\n');
    kept++; console.log(`  KEEP  ${problem.id.padEnd(26)} ${r.detail}`);
  }
  console.log(`\nDONE: kept ${kept}, dropped ${dropped} (of ${specs.length}). Kept triples passed the verify gate.`);
}

function usage() {
  console.log(`gen-problems.mjs — verified rustlite dataset generator

  --verify-seeds        re-verify every seed triple on disk (no model needed)
  --gen <specs.json>    generate triples from a specs file (needs a teacher key:
                        ANTHROPIC_API_KEY for direct Claude, or wire the $LH proxy)
  --help

The moat: every kept triple passes the SAME verifier verify.sh runs — compile
via the rustlite compiler, instantiate the wasm, render, assert pixels/state.`);
}

const arg = process.argv[2];
if (arg === '--verify-seeds') {
  verifySeeds();
} else if (arg === '--gen') {
  if (!process.argv[3]) { console.error('--gen needs a specs.json path'); process.exit(2); }
  generate(process.argv[3]).catch((e) => { console.error(e); process.exit(1); });
} else {
  usage();
}
