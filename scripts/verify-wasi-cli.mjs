// verify-wasi-cli.mjs — instantiate + run the committed example WASI CLI
// (examples/cli/hello.wasm) through the SAME WASI-subset host the browser uses
// (web/wasi-worker.js, Node test surface) and assert its stdout / argv / exit.
//
// This is the honest end-to-end proof for on-chain feedback #6: a real compiled
// wasm `_start` command runs under the host and its text output is captured.
//
//   node scripts/verify-wasi-cli.mjs
//
// Exits non-zero on any mismatch so it can gate a release like the other
// scripts/verify-*.mjs node proofs.

import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const require = createRequire(import.meta.url);
const here = dirname(fileURLToPath(import.meta.url));
const host = require(join(here, '..', 'web', 'wasi-worker.js'));

const wasm = readFileSync(join(here, '..', 'examples', 'cli', 'hello.wasm'));

let failures = 0;
function check(name, cond, detail) {
  if (cond) {
    console.log(`  ok   ${name}`);
  } else {
    failures++;
    console.log(`  FAIL ${name}${detail ? ' — ' + detail : ''}`);
  }
}

// 1) Run with two extra args; argv[0] is the synthetic "prog".
const r = host.runWasi(wasm, ['alpha', 'beta']);
check('exit code is 0', r.exitCode === 0, `got ${r.exitCode}`);
check('stdout has the greeting', r.stdout.includes('hello from wasm cli'), JSON.stringify(r.stdout));
check('stdout dumps argv label', r.stdout.includes('argv:'), JSON.stringify(r.stdout));
check('argv[0] is prog', r.stdout.includes('prog'), JSON.stringify(r.stdout));
check('passed arg alpha echoed', r.stdout.includes('alpha'), JSON.stringify(r.stdout));
check('passed arg beta echoed', r.stdout.includes('beta'), JSON.stringify(r.stdout));
check('stderr empty', r.stderr === '', JSON.stringify(r.stderr));
check('not truncated', r.truncated === false, String(r.truncated));

// 2) No-arg run still prints the greeting + argv[0].
const r2 = host.runWasi(wasm, []);
check('no-arg run exits 0', r2.exitCode === 0, `got ${r2.exitCode}`);
check('no-arg run greets', r2.stdout.includes('hello from wasm cli'), JSON.stringify(r2.stdout));

// 3) A non-WASI / garbage module surfaces a clear error, not a silent success.
let threw = false;
try { host.runWasi(new Uint8Array([0, 1, 2, 3])); } catch (e) { threw = true; }
check('garbage bytes throw', threw);

if (failures) {
  console.error(`\n${failures} check(s) FAILED`);
  process.exit(1);
}
console.log('\nall WASI-CLI checks passed');
