#!/usr/bin/env node
// localharness native verifiable benchmark v1 (telemetry #81).
//
// Every task is scored by a MACHINE — compile checks, wasm export/import
// inspection, behavioral wasm calls, bashlite stdout assertions, artifact
// presence/content probes. No LLM judging anywhere in the scoring path.
//
//   node scripts/bench/run.mjs                          # offline (default): score solutions/
//   node scripts/bench/run.mjs --solutions <dir>        # score someone else's answers
//   node scripts/bench/run.mjs --only rl-clear,bl-hello # subset
//   node scripts/bench/run.mjs --live --target claude [--as claude]
//                                                       # get answers via `localharness call`
//   node scripts/bench/run.mjs --json                   # machine-readable summary on stdout
//
// Prereq: cargo build (the scorer shells out to target/debug/localharness).

import {
  readFileSync, readdirSync, writeFileSync, mkdirSync, mkdtempSync, existsSync,
  cpSync, rmSync,
} from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..', '..');
const BIN = ['localharness.exe', 'localharness']
  .map((b) => join(root, 'target', 'debug', b))
  .find(existsSync);

// ---- args -------------------------------------------------------------------
const argv = process.argv.slice(2);
const flag = (n) => argv.includes(n);
const opt = (n) => {
  const i = argv.indexOf(n);
  return i >= 0 && argv[i + 1] ? argv[i + 1] : null;
};
const LIVE = flag('--live');
const JSON_OUT = flag('--json');
const ONLY = (opt('--only') ?? '').split(',').map((s) => s.trim()).filter(Boolean);
const SOLUTIONS = resolve(opt('--solutions') ?? join(here, 'solutions'));
const AS_NAME = opt('--as');
const TARGET = opt('--target');

if (!BIN) {
  console.error('no target/debug/localharness binary — run `cargo build` first');
  process.exit(2);
}
if (LIVE && !TARGET) {
  console.error('--live needs --target <agent> (and usually --as <yourname>)');
  process.exit(2);
}

// ---- helpers ------------------------------------------------------------------
const norm = (s) => s.replace(/\r\n/g, '\n').replace(/\s+$/, '');
const tmp = () => mkdtempSync(join(tmpdir(), 'lh-bench-'));

function runBin(args, timeout = 120_000) {
  const r = spawnSync(BIN, args, { encoding: 'utf8', timeout, cwd: root });
  return { status: r.status ?? -1, stdout: r.stdout ?? '', stderr: r.stderr ?? '' };
}

// Parse the `compile --host-calls` listing: indented `host::...` lines.
const parseHostCalls = (stdout) =>
  [...stdout.matchAll(/^\s+(host::\S+)$/gm)].map((m) => m[1]);

// Stub every function import with () => 0 so any cartridge instantiates.
function instantiate(bytes) {
  const mod = new WebAssembly.Module(bytes);
  const imports = {};
  for (const im of WebAssembly.Module.imports(mod)) {
    imports[im.module] ??= {};
    if (im.kind === 'function') imports[im.module][im.name] = () => 0;
    else if (im.kind === 'memory') imports[im.module][im.name] = new WebAssembly.Memory({ initial: 16 });
    else if (im.kind === 'global') imports[im.module][im.name] = 0;
  }
  return { mod, inst: new WebAssembly.Instance(mod, imports) };
}

// ---- scorers (each returns {pass, checks:[{name, pass, detail?}]}) -----------
function scoreRustlite(task, sourcePath) {
  const checks = [];
  const push = (name, pass, detail) => checks.push(detail ? { name, pass, detail } : { name, pass });
  const s = task.scorer ?? {};
  const work = tmp();
  const out = join(work, 'out.wasm');
  try {
    const r = runBin(['compile', sourcePath, out, '--host-calls']);
    push('compiles', r.status === 0 && existsSync(out), r.status === 0 ? undefined : norm(r.stdout + r.stderr).slice(-300));
    if (!checks[0].pass) return { pass: false, checks };

    const hostCalls = parseHostCalls(r.stdout);
    for (const hc of s.required_host_calls ?? [])
      push(`host-call ${hc}`, hostCalls.includes(hc));
    for (const hc of s.forbidden_host_calls ?? [])
      push(`no host-call ${hc}`, !hostCalls.includes(hc));

    const bytes = readFileSync(out);
    if (s.max_bytes) push(`<= ${s.max_bytes} bytes`, bytes.length <= s.max_bytes, `${bytes.length}`);

    let inst;
    try {
      inst = instantiate(bytes).inst;
      push('instantiates', true);
    } catch (e) {
      push('instantiates', false, String(e).slice(0, 200));
      return { pass: false, checks };
    }
    const exports = inst.exports;
    for (const ex of s.required_exports ?? [])
      push(`export ${ex}`, typeof exports[ex] === 'function');
    for (const c of s.calls ?? []) {
      const label = `${c.export}(${(c.args ?? []).join(',')}) == ${c.expect}`;
      try {
        const got = exports[c.export](...(c.args ?? []));
        push(label, got === c.expect, got !== c.expect ? `got ${got}` : undefined);
      } catch (e) {
        push(label, false, `trap: ${String(e).slice(0, 120)}`);
      }
    }
    for (const ex of s.must_run ?? []) {
      try {
        for (let i = 0; i < 3; i++) exports[ex](i);
        push(`${ex}() runs 3x`, true);
      } catch (e) {
        push(`${ex}() runs 3x`, false, `trap: ${String(e).slice(0, 120)}`);
      }
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
  return { pass: checks.every((c) => c.pass), checks };
}

function runBashlite(scriptSource, setupFiles) {
  const work = tmp();
  for (const [rel, content] of Object.entries(setupFiles ?? {})) {
    const p = join(work, rel);
    mkdirSync(dirname(p), { recursive: true });
    writeFileSync(p, content);
  }
  const main = join(work, 'answer.bl');
  writeFileSync(main, scriptSource);
  const r = runBin(['sh', main]); // bashlite fs is ROOTED to the script's dir
  rmSync(work, { recursive: true, force: true });
  return r;
}

function scoreBashlite(task, scriptPath) {
  const s = task.scorer ?? {};
  const checks = [];
  const r = runBashlite(readFileSync(scriptPath, 'utf8'), s.setup_files);
  const stdout = norm(r.stdout);
  checks.push({ name: `exit ${s.expect_exit ?? 0}`, pass: r.status === (s.expect_exit ?? 0) });
  if (s.expect_stdout !== undefined)
    checks.push({
      name: 'stdout exact', pass: stdout === norm(s.expect_stdout),
      ...(stdout === norm(s.expect_stdout) ? {} : { detail: `got: ${JSON.stringify(stdout.slice(0, 200))}` }),
    });
  for (const sub of s.expect_stdout_contains ?? [])
    checks.push({ name: `stdout has ${JSON.stringify(sub)}`, pass: stdout.includes(sub) });
  return { pass: checks.every((c) => c.pass), checks };
}

function scoreArtifact(task, dir) {
  const s = task.scorer ?? {};
  const checks = [];
  for (const a of s.artifacts ?? [])
    checks.push({ name: `artifact ${a}`, pass: existsSync(join(dir, a)) });
  for (const [file, subs] of Object.entries(s.contains ?? {})) {
    const p = join(dir, file);
    const text = existsSync(p) ? readFileSync(p, 'utf8') : '';
    for (const sub of subs)
      checks.push({ name: `${file} has ${JSON.stringify(sub)}`, pass: text.includes(sub) });
  }
  for (const file of s.compile ?? []) {
    const p = join(dir, file);
    const r = existsSync(p) ? runBin(['compile', p]) : { status: -1 };
    checks.push({ name: `${file} compiles`, pass: r.status === 0 });
  }
  if (s.run) {
    // run in a COPY so scripts can't dirty the answers dir
    const work = tmp();
    cpSync(dir, work, { recursive: true });
    const r = runBin(['sh', join(work, s.run.script)]);
    const stdout = norm(r.stdout);
    checks.push({ name: `${s.run.script} exit ${s.run.expect_exit ?? 0}`, pass: r.status === (s.run.expect_exit ?? 0) });
    for (const sub of s.run.expect_stdout_contains ?? [])
      checks.push({ name: `${s.run.script} stdout has ${JSON.stringify(sub)}`, pass: stdout.includes(sub) });
    rmSync(work, { recursive: true, force: true });
  }
  return { pass: checks.every((c) => c.pass), checks };
}

// ---- live mode: answer via `localharness call`, then the SAME scorers --------
function extractFence(reply) {
  const fences = [...reply.matchAll(/```[a-zA-Z]*\r?\n([\s\S]*?)```/g)];
  return fences.length ? fences[fences.length - 1][1] : reply.trim();
}

function liveAnswer(task) {
  const args = ['call'];
  if (AS_NAME) args.push('--as', AS_NAME);
  args.push(TARGET, task.prompt);
  const r = runBin(args, 300_000);
  if (r.status !== 0) return { error: norm(r.stderr + r.stdout).slice(-400) };
  return { source: extractFence(r.stdout) };
}

// ---- main ---------------------------------------------------------------------
const tasks = readdirSync(join(here, 'tasks'))
  .filter((f) => f.endsWith('.json'))
  .sort()
  .map((f) => JSON.parse(readFileSync(join(here, 'tasks', f), 'utf8')))
  .filter((t) => ONLY.length === 0 || ONLY.includes(t.id));

if (tasks.length === 0) {
  console.error('no tasks selected');
  process.exit(2);
}

const results = [];
for (const task of tasks) {
  let res;
  let note = '';
  if (LIVE) {
    if (task.kind === 'artifact') {
      res = { pass: false, checks: [], skipped: true };
      note = 'skipped in --live (artifact tasks score a workspace, not a chat reply)';
    } else {
      const ans = liveAnswer(task);
      if (ans.error) {
        res = { pass: false, checks: [{ name: 'call succeeded', pass: false, detail: ans.error }] };
      } else {
        const work = tmp();
        const file = join(work, task.kind === 'rustlite' ? 'answer.rl' : 'answer.bl');
        writeFileSync(file, ans.source);
        res = task.kind === 'rustlite' ? scoreRustlite(task, file) : scoreBashlite(task, file);
        rmSync(work, { recursive: true, force: true });
      }
    }
  } else {
    const solPath = join(SOLUTIONS, task.solution);
    if (!existsSync(solPath)) {
      res = { pass: false, checks: [{ name: `answer ${task.solution} present`, pass: false }] };
    } else if (task.kind === 'rustlite') res = scoreRustlite(task, solPath);
    else if (task.kind === 'bashlite') res = scoreBashlite(task, solPath);
    else res = scoreArtifact(task, solPath);
  }
  const earned = res.pass ? task.points : 0;
  results.push({ id: task.id, kind: task.kind, points: task.points, earned, pass: !!res.pass, skipped: !!res.skipped, checks: res.checks, note });

  if (!JSON_OUT) {
    const tag = res.skipped ? 'SKIP' : res.pass ? 'PASS' : 'FAIL';
    const ok = res.checks.filter((c) => c.pass).length;
    console.log(`[${tag}] ${task.id.padEnd(14)} ${String(earned).padStart(3)}/${String(task.points).padEnd(3)} ${res.skipped ? note : `${ok}/${res.checks.length} checks`}`);
    for (const c of res.checks.filter((c) => !c.pass))
      console.log(`       x ${c.name}${c.detail ? ` — ${c.detail}` : ''}`);
  }
}

const scored = results.filter((r) => !r.skipped);
const earned = scored.reduce((a, r) => a + r.earned, 0);
const total = scored.reduce((a, r) => a + r.points, 0);
const passed = scored.filter((r) => r.pass).length;
const summary = { mode: LIVE ? 'live' : 'offline', tasks: results, passed, scored: scored.length, earned, total };

if (JSON_OUT) console.log(JSON.stringify(summary, null, 2));
else console.log(`TOTAL ${earned}/${total} points · ${passed}/${scored.length} tasks${results.length !== scored.length ? ` · ${results.length - scored.length} skipped` : ''}`);

process.exit(passed === scored.length ? 0 : 1);
