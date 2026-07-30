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
//   node scripts/bench/run.mjs --seed 7                 # re-derive fixtures/args/prompts
//   node scripts/bench/run.mjs --live --target claude [--as claude]
//                                                       # get answers via `localharness call`
//   node scripts/bench/run.mjs --json                   # machine-readable summary on stdout
//
// Anti-overfit: tasks marked `"seeded": true` are PARAMETERIZED — fixtures,
// behavioral call args, and content probes are derived from --seed (default 1 =
// the frozen v1 instance, kept for baseline comparability), and every expected
// value is COMPUTED from the generated material, never hardcoded. Scores are
// only meaningful WITH their seed; the runner prints it everywhere.
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
const SEED = Number.parseInt(opt('--seed') ?? '1', 10);
if (!Number.isInteger(SEED)) {
  console.error('--seed needs an integer');
  process.exit(2);
}

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

// ---- seeded parameterization -------------------------------------------------
// Per-task PRNG stream: mulberry32 over fnv1a(task.id) ^ mix(seed), so an
// instance depends only on (seed, id) — stable under --only and task ordering.
function mulberry32(a) {
  return () => {
    a |= 0; a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
function fnv1a(s) {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) h = Math.imul(h ^ s.charCodeAt(i), 0x01000193);
  return h >>> 0;
}
const taskRng = (id) => mulberry32((fnv1a(id) ^ Math.imul(SEED, 0x9e3779b1)) >>> 0);
const ri = (rng, lo, hi) => lo + Math.floor(rng() * (hi - lo + 1)); // inclusive
const pick = (rng, arr) => arr[ri(rng, 0, arr.length - 1)];

// One generator per `"seeded": true` task. Each returns scorer OVERRIDES for a
// concrete instance: fixtures/args are generated, and every expectation is
// COMPUTED from the generated material — never hardcoded. Seed 1 emits the
// frozen v1 fixture values (through the same compute path) so historical
// baseline scores stay comparable.
const SEEDED = {
  'bl-count': (rng) => {
    const rl = SEED === 1 ? ['a', 'b', 'c']
      : ['alpha', 'beta', 'gamma', 'delta', 'epsilon', 'zeta'].slice(0, ri(rng, 2, 6));
    const decoys = SEED === 1 ? ['notes.txt']
      : Array.from({ length: ri(rng, 1, 3) }, (_, i) => `note${i}.${pick(rng, ['txt', 'md', 'json'])}`);
    const setup_files = {};
    for (const n of rl) setup_files[`cartridges/${n}.rl`] = `// ${n}\n`;
    for (const n of decoys) setup_files[`cartridges/${n}`] = 'not a cartridge\n';
    const count = Object.keys(setup_files).filter((f) => f.endsWith('.rl')).length;
    return { setup_files, expect_stdout: `${count} cartridges` };
  },
  'bl-filter': (rng) => {
    let lines;
    if (SEED === 1) {
      lines = ['INFO boot', 'ERROR disk full', 'INFO tick', 'ERROR net down', 'WARN slow'];
    } else {
      const errs = ['disk full', 'net down', 'timeout', 'bad checksum', 'oom kill'];
      const infos = ['boot', 'tick', 'sync ok', 'idle', 'ready', 'flush'];
      lines = [
        ...Array.from({ length: ri(rng, 2, 4) }, () => `ERROR ${pick(rng, errs)}`),
        ...Array.from({ length: ri(rng, 2, 4) }, () => `${pick(rng, ['INFO', 'WARN'])} ${pick(rng, infos)}`),
      ];
      for (let i = lines.length - 1; i > 0; i--) { // deterministic shuffle
        const j = ri(rng, 0, i);
        [lines[i], lines[j]] = [lines[j], lines[i]];
      }
    }
    const errLines = lines.filter((l) => l.includes('ERROR')); // computed FROM the fixture
    return {
      setup_files: { 'log.txt': lines.join('\n') + '\n' },
      expect_stdout: [...errLines, `errors: ${errLines.length}`].join('\n'),
    };
  },
  'bl-branch': (rng) => {
    const status = SEED === 1 ? 'active'
      : pick(rng, ['active', 'degraded', 'stopped', 'offline', 'active', 'maintenance']);
    return {
      setup_files: { 'status.txt': status },
      expect_stdout: status === 'active' ? 'service up' : 'service down',
    };
  },
  'bl-compose': (rng) => {
    const line = `child says ${SEED === 1 ? 'hi' : pick(rng, ['hi', 'yo', 'ping', 'pong', 'ready', 'ok'])}`;
    return {
      setup_files: { 'child.bl': `echo "${line}"\n` },
      expect_stdout: `${line}\n${line}\nparent done`,
    };
  },
  'rl-lib-math': (rng) => {
    let addArgs, mulArgs, lo, hi, probes;
    if (SEED === 1) {
      addArgs = [[2, 3], [-4, 9]]; mulArgs = [[6, 7]];
      [lo, hi] = [0, 5]; probes = [10, -3, 3];
    } else {
      addArgs = [[ri(rng, -99, 99), ri(rng, -99, 99)], [ri(rng, -99, 99), ri(rng, -99, 99)]];
      mulArgs = [[ri(rng, -12, 12), ri(rng, -12, 12)]];
      lo = ri(rng, -20, 10); hi = lo + ri(rng, 1, 40);
      probes = [hi + ri(rng, 1, 25), lo - ri(rng, 1, 25), ri(rng, lo, hi)]; // above, below, inside
    }
    return {
      calls: [
        ...addArgs.map(([a, b]) => ({ export: 'add', args: [a, b], expect: (a + b) | 0 })),
        ...mulArgs.map(([a, b]) => ({ export: 'mul', args: [a, b], expect: Math.imul(a, b) })),
        ...probes.map((v) => ({ export: 'clamp', args: [v, lo, hi], expect: Math.min(Math.max(v, lo), hi) })),
      ],
    };
  },
  'cli-scaffold': (rng) => {
    if (SEED === 1) return {}; // frozen v1 probe set (README only)
    // rotate 2 of 3 prompt-guaranteed app.rl markers into the probe set
    const pool = ['fn frame', 'host::display::clear', 'host::display::present'];
    const i = ri(rng, 0, 2);
    return { contains: { 'README.md': ['# ', 'publish'], 'app.rl': [pool[i], pool[(i + 1 + ri(rng, 0, 1)) % 3]] } };
  },
};

// Deep-copy a raw task into a concrete seeded instance: pick a prompt phrasing
// and apply the task's generator (if any). Scorers below stay seed-unaware.
function materialize(raw) {
  const task = structuredClone(raw);
  const rng = taskRng(task.id);
  const phrasings = task.prompts?.length ? task.prompts : [task.prompt];
  task.prompt = phrasings[SEED === 1 ? 0 : ri(rng, 0, phrasings.length - 1)];
  const gen = SEEDED[task.id];
  if (!!task.seeded !== !!gen) {
    console.error(`task ${task.id}: "seeded" flag and run.mjs SEEDED generator disagree`);
    process.exit(2);
  }
  if (gen) Object.assign(task.scorer ??= {}, gen(rng));
  return task;
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
  .filter((t) => ONLY.length === 0 || ONLY.includes(t.id))
  .map(materialize);

if (tasks.length === 0) {
  console.error('no tasks selected');
  process.exit(2);
}
if (!JSON_OUT) console.log(`seed ${SEED} · ${tasks.length} tasks`);

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
const summary = { mode: LIVE ? 'live' : 'offline', seed: SEED, tasks: results, passed, scored: scored.length, earned, total };

if (JSON_OUT) console.log(JSON.stringify(summary, null, 2));
else console.log(`TOTAL ${earned}/${total} points · ${passed}/${scored.length} tasks${results.length !== scored.length ? ` · ${results.length - scored.length} skipped` : ''} · seed ${SEED}`);

process.exit(passed === scored.length ? 0 : 1);
