#!/usr/bin/env node
// Validate that a compiled rustlite cartridge actually instantiates and runs.
//
//   localharness compile app.rl app.wasm   # emit the wasm
//   node scripts/validate-cartridge.js app.wasm
//
// `localharness compile` already proves the source parses, typechecks, and
// codegens. This goes one step further: it instantiates the wasm with stub
// host imports and calls the exported frame()/render() across a few simulated
// pointer events, catching runtime traps (bad codegen, div-by-zero, OOB) that
// a static compile can't. Host draw calls are no-ops; state_get/state_set are
// backed by a Map so stateful logic behaves. Exits non-zero on any trap.

const fs = require('fs');
const path = process.argv[2];
if (!path) {
  console.error('usage: node scripts/validate-cartridge.js <cartridge.wasm>');
  process.exit(2);
}
const mod = new WebAssembly.Module(fs.readFileSync(path));
const state = new Map();
let ptr = { x: 0, y: 0, down: 0 };
let hostCalls = 0;
const importObj = {};
for (const imp of WebAssembly.Module.imports(mod)) {
  importObj[imp.module] = importObj[imp.module] || {};
  if (imp.kind === 'function') {
    const n = imp.name;
    importObj[imp.module][n] = (...a) => {
      hostCalls++;
      if (n === 'width') return 256;
      if (n === 'height') return 144;
      if (n === 'pointer_x') return ptr.x;
      if (n === 'pointer_y') return ptr.y;
      if (n === 'pointer_down') return ptr.down;
      if (n === 'state_get') return state.get(a[0] | 0) || 0;
      if (n === 'state_set') { state.set(a[0] | 0, a[1] | 0); return; }
      return 0;
    };
  } else if (imp.kind === 'memory') importObj[imp.module][imp.name] = new WebAssembly.Memory({ initial: 1 });
  else if (imp.kind === 'global') importObj[imp.module][imp.name] = new WebAssembly.Global({ value: 'i32', mutable: true }, 0);
  else if (imp.kind === 'table') importObj[imp.module][imp.name] = new WebAssembly.Table({ initial: 1, element: 'anyfunc' });
}
const inst = new WebAssembly.Instance(mod, importObj);
const entry = inst.exports.frame || inst.exports.render;
if (typeof entry !== 'function') {
  console.error('FAIL: cartridge exports neither frame() nor render()');
  process.exit(1);
}
try {
  for (let t = 0; t < 4; t++) entry(t);                 // a few render frames
  for (const [x, y] of [[8, 50], [200, 125], [80, 125]]) { // simulated taps
    ptr = { x, y, down: 1 }; entry(0);
    ptr.down = 0; entry(0);
  }
} catch (e) {
  console.error('FAIL: trap during execution —', e.message);
  process.exit(1);
}
console.log(`PASS: ${path} instantiates and runs (${hostCalls} host calls, no traps)`);
