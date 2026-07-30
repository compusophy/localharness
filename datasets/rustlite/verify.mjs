#!/usr/bin/env node
// verify.mjs — every solution_rl in this directory MUST compile.
//
// Extracts each NNN.json's solution_rl to a temp .rl file, runs
// `localharness compile <file>` on it, and prints a pass table.
// Exit 0 only on 50/50 (or N/N). Run from the repo root:
//
//   node datasets/rustlite/verify.mjs [path/to/localharness]
//
// Default binary: ./target/debug/localharness (build with `cargo build`).
import { readdirSync, readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const dir = dirname(fileURLToPath(import.meta.url));
const exe = process.argv[2] ?? join(dir, '..', '..', 'target', 'debug', process.platform === 'win32' ? 'localharness.exe' : 'localharness');
const tmp = mkdtempSync(join(tmpdir(), 'rl-dataset-'));

const files = readdirSync(dir).filter(f => /^\d{3}\.json$/.test(f)).sort();
let pass = 0;
const rows = [];
for (const f of files) {
  const { id, solution_rl, tags } = JSON.parse(readFileSync(join(dir, f), 'utf8'));
  const src = join(tmp, `${id}.rl`);
  writeFileSync(src, solution_rl);
  let ok = false, note = '';
  try {
    const out = execFileSync(exe, ['compile', src], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
    const m = out.match(/(\d+) bytes of wasm/);
    ok = /compiled/.test(out);
    note = m ? `${m[1]}B wasm` : '';
  } catch (e) {
    note = String(e.stderr || e.message).split('\n').find(l => l.includes('failed'))?.trim() ?? 'compile error';
  }
  if (ok) pass++;
  rows.push({ id, ok, note, tags: tags.join(',') });
}
rmSync(tmp, { recursive: true, force: true });

for (const r of rows) console.log(`${r.ok ? 'PASS' : 'FAIL'}  ${r.id}  ${r.note.padEnd(12)} ${r.tags}`);
console.log(`\n${pass}/${rows.length} solutions compile`);
process.exit(pass === rows.length && rows.length > 0 ? 0 : 1);
