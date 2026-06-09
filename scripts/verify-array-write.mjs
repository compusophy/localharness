#!/usr/bin/env node
// RUN-PROOF for rustlite INDEXED ARRAY WRITES (`arr[i] = value`).
//
// Codegen is the kind of change you cannot trust by "it compiles" — the
// project rule is: validate generated wasm by INSTANTIATING + RUNNING it, not
// by sniffing the magic header. This harness loads the compiler-emitted .wasm
// in node, stubs the host_display imports (the same shape scripts/render-
// cartridge.js / test-worker-host-parity.mjs use), calls the exported
// frame()/render(), and asserts the WRITTEN value reads back.
//
// The wasm bytes are produced by the Rust test
//   `rustlite::array_write_run_proof::emits_wasm_for_node_proof`
// (run it first: `cargo test emits_wasm_for_node_proof`), which compiles the
// rustlite snippets below and writes them next to this script under
// scripts/.array-write-proof/. This file then runs each and checks the
// observable result via host_display::clear (which receives the int the
// cartridge read back out of the array).
//
//   cargo test emits_wasm_for_node_proof
//   node scripts/verify-array-write.mjs
//
// Exits non-zero if any round-trip assertion fails.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const dir = join(here, '.array-write-proof');

// Run one cartridge: instantiate, call frame(t), and capture the LAST value the
// cartridge passed to host_display::clear (cartridges in this proof end their
// frame by clear()-ing with the value they read back out of the array).
function runCartridge(file, t) {
  let lastClear = null;
  const host_display = {
    clear: (rgb) => { lastClear = rgb | 0; },
    set_pixel: () => {},
    fill_rect: () => {},
    draw_char: () => {},
    draw_number: () => {},
    present: () => {},
    width: () => 256,
    height: () => 144,
    pointer_x: () => 0,
    pointer_y: () => 0,
    pointer_down: () => 0,
    state_get: () => 0,
    state_set: () => {},
  };
  const bytes = readFileSync(join(dir, file));
  const mod = new WebAssembly.Module(bytes);
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
  if (typeof frame !== 'function') throw new Error(`${file}: no frame()/render() export`);
  frame(t | 0);
  return lastClear;
}

let failures = 0;
function assertEq(label, got, want) {
  if (got === want) {
    console.log(`PASS  ${label}: read back ${got}`);
  } else {
    console.error(`FAIL  ${label}: expected ${want}, got ${got}`);
    failures++;
  }
}

// 1) Single write then read-back:
//    let mut a = [0,0,0,0]; a[2] = 42; clear(a[2]);
// Proves a write lands and the matching read sees it (same layout).
assertEq('single write a[2]=42', runCartridge('single.wasm', 0), 42);

// 2) Loop-fill then read several back:
//    let mut a = [0,0,0,0,0]; for i in 0..5 { a[i] = i*10; } clear(a[3]);
// Proves a *variable* index write inside a loop addresses each element
// distinctly (a[3] must be 30, not 0 and not some other cell).
assertEq('loop-fill a[3]=3*10', runCartridge('loopfill.wasm', 0), 30);
//    same cartridge, different cell read (frame argument selects which):
//    clear(a[t]) — t=4 must read 40.
assertEq('loop-fill a[4]=4*10', runCartridge('loopfill_t.wasm', 4), 40);
assertEq('loop-fill a[1]=1*10', runCartridge('loopfill_t.wasm', 1), 10);

// 3) Overwrite: write a cell twice, the later write wins.
//    let mut a = [0,0]; a[0] = 7; a[0] = 99; clear(a[0]);
assertEq('overwrite a[0]=99', runCartridge('overwrite.wasm', 0), 99);

if (failures > 0) {
  console.error(`\n${failures} assertion(s) FAILED`);
  process.exit(1);
}
console.log('\nALL PASS: indexed array writes round-trip through real wasm');
