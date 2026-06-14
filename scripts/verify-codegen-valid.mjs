#!/usr/bin/env node
// VALIDATE-PROOF for the GitHub-#80 rustlite codegen fixes.
//
// Three bugs lowered ACCEPTED source to STACK-IMBALANCED wasm:
//   (1) a value-position `if` without `else` emitted an `(if (result T))` frame
//       with no else branch,
//   (2) a short-circuit `&&`/`||` opened an if-frame for the rhs but never
//       bumped the break/continue depth, so a break/continue in the rhs branched
//       to the wrong frame,
//   (3) a non-last `_`/binding match arm emitted an unbalanced if/else chain.
//
// "It compiles" is not proof for codegen — the project rule is to VALIDATE the
// generated wasm. This harness runs `WebAssembly.validate` over each emitted
// module (and, for the runnable ones, instantiates + calls frame() to prove the
// fix produces a real, runnable module, not just a structurally-valid one).
//
// The wasm bytes come from the Rust test
//   `rustlite::codegen_valid_run_proof::emits_codegen_valid_proof`
// (run it first), which writes them under scripts/.codegen-valid-proof/.
//
//   cargo test emits_codegen_valid_proof
//   node scripts/verify-codegen-valid.mjs
//
// Exits non-zero if any module fails to validate.

import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const dir = join(here, '.codegen-valid-proof');

// Every emitted #80 cartridge: each must `WebAssembly.validate` true.
const files = [
  'elseless_if_stmt.wasm',
  'value_if_else.wasm',
  'and_break_rhs.wasm',
  'or_continue_rhs.wasm',
  'match_wildcard_last.wasm',
];

// A neutral host_display stub (same shape as verify-array-write.mjs) so the
// terminating cartridges can also be instantiated + run.
function makeHost() {
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
  return { host_display, getLast: () => lastClear };
}

let failures = 0;

for (const file of files) {
  const bytes = readFileSync(join(dir, file));
  const ok = WebAssembly.validate(bytes);
  if (ok) {
    console.log(`PASS  ${file}: WebAssembly.validate() == true`);
  } else {
    console.error(`FAIL  ${file}: WebAssembly.validate() == false (invalid module)`);
    failures++;
    continue;
  }
  // Instantiate the terminating cartridges to prove the bytes are runnable, not
  // just structurally valid. The looping ones (and_break_rhs/or_continue_rhs)
  // terminate via break/continue, so they're safe to call once.
  try {
    const { host_display } = makeHost();
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
    if (typeof frame === 'function') {
      frame(1);
      console.log(`      ${file}: instantiated + frame(1) ran`);
    }
  } catch (e) {
    console.error(`FAIL  ${file}: validated but failed to instantiate/run: ${e}`);
    failures++;
  }
}

if (failures > 0) {
  console.error(`\n${failures} module(s) FAILED to validate/run`);
  process.exit(1);
}
console.log('\nALL PASS: the #80 codegen fixes emit valid, runnable wasm');
