// scripts/colony/lib.mjs — shared plumbing for the colony pipeline scripts
// (sync-issues / issue-to-bounty / settle-on-merge). Zero npm deps: on-chain
// reads are raw JSON-RPC eth_call via global fetch (node >= 18), GitHub access
// is the `gh` CLI as a subprocess (execFileSync with arg arrays — no shell, so
// it is Windows-safe), and on-chain WRITES go through the `localharness` CLI.
//
// Auth model: `gh` uses the maintainer's logged-in account by default and
// AUTOMATICALLY honors GH_TOKEN when set — the future compusophy-bot swap is
// `GH_TOKEN=<bot pat>` in the environment, no script change. We never strip or
// rewrite the child env, so that contract holds for every gh call here.

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

// ---------------------------------------------------------------- constants

/** Repo root = two levels up from scripts/colony/ (works from any cwd). */
export const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');

/** GitHub repo every gh call pins via --repo (the worktree has TWO remotes —
 *  origin + an unrelated upstream — so gh must never guess). */
export const REPO = process.env.LH_REPO || 'compusophy/localharness';

/** Registry diamond + Tempo Moderato RPC (CLAUDE.md canonical addresses). */
export const DIAMOND = process.env.DIAMOND || '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c';
export const RPC = process.env.RPC || 'https://rpc.moderato.tempo.xyz';

/** The visible marker line stamped into every colony-synced issue body; the
 *  dedup contract between on-chain feedback indices and GitHub issues. */
export const MARKER_PREFIX = 'lh-feedback:';

// FeedbackFacet view selectors, precomputed once via `cast sig` (selectors are
// immutable for a fixed signature, so vanilla node needs no keccak):
//   cast sig "feedbackCount()"      -> 0x2ed3f65b
//   cast sig "feedbackAt(uint256)"  -> 0x5274f07a
const SEL_FEEDBACK_COUNT = '0x2ed3f65b';
const SEL_FEEDBACK_AT = '0x5274f07a';

// ------------------------------------------------------------ arg utilities

/** True when argv contains the bare flag (e.g. hasFlag('--live')). */
export function hasFlag(name, argv = process.argv.slice(2)) {
  return argv.includes(name);
}

/** Value of `--name <value>` from argv, else `def`. */
export function takeFlag(name, def, argv = process.argv.slice(2)) {
  const i = argv.indexOf(name);
  if (i === -1 || i + 1 >= argv.length) return def;
  return argv[i + 1];
}

/** Positional args = argv minus known `--flag value` pairs and bare flags. */
export function positionals(valueFlags, bareFlags, argv = process.argv.slice(2)) {
  const out = [];
  for (let i = 0; i < argv.length; i++) {
    if (valueFlags.includes(argv[i])) {
      i++; // skip the value
    } else if (!bareFlags.includes(argv[i])) {
      out.push(argv[i]);
    }
  }
  return out;
}

/** Render an argv array as a copy-pasteable one-line command (display only —
 *  execution always uses the array form, never a shell string). */
export function fmtCmd(argv) {
  return argv.map((a) => (/[\s"']/.test(a) ? `"${a.replace(/"/g, '\\"')}"` : a)).join(' ');
}

// ------------------------------------------------------------------ gh + CLI

/** Run `gh <args> --repo REPO`, return stdout. Throws with gh's stderr line on
 *  failure. READ-ONLY callers only, except behind an explicit --live gate. */
export function gh(args, { repoFlag = true } = {}) {
  const full = repoFlag ? [...args, '--repo', REPO] : args;
  try {
    return execFileSync('gh', full, { encoding: 'utf8', maxBuffer: 64 << 20 });
  } catch (e) {
    const detail = (e.stderr || e.message || '').toString().trim().split('\n')[0];
    throw new Error(`gh ${args[0]} ${args[1] || ''} failed: ${detail}`);
  }
}

/** Resolve the localharness CLI binary: LOCALHARNESS_BIN env > repo-local debug
 *  build (.exe first — the maintainer is on Windows) > `localharness` on PATH. */
export function resolveCli() {
  if (process.env.LOCALHARNESS_BIN) return process.env.LOCALHARNESS_BIN;
  for (const rel of ['target/debug/localharness.exe', 'target/debug/localharness']) {
    const p = join(REPO_ROOT, rel);
    if (existsSync(p)) return p;
  }
  return 'localharness';
}

/** Run the localharness CLI, returning stdout (read-only subcommands), or
 *  inheriting stdio when `inherit` (the --live write path). */
export function runCli(args, { inherit = false } = {}) {
  const cli = resolveCli();
  if (inherit) {
    execFileSync(cli, args, { stdio: 'inherit' });
    return '';
  }
  return execFileSync(cli, args, { encoding: 'utf8', maxBuffer: 16 << 20 });
}

// ------------------------------------------------------------- keccak256

// Minimal pure-JS keccak256 (Ethereum's hash, NOT FIPS SHA3 — node's crypto
// only ships SHA3, which differs in the padding byte, so we can't borrow it).
// Used solely to derive 4-byte function selectors at build time, so a fixed
// implementation beats hardcoding a growing table of magic hex by hand.
// Verified against lib.mjs's known feedbackCount()/feedbackAt() selectors.
const _RC = [
  0x0000000000000001n, 0x0000000000008082n, 0x800000000000808an, 0x8000000080008000n,
  0x000000000000808bn, 0x0000000080000001n, 0x8000000080008081n, 0x8000000000008009n,
  0x000000000000008an, 0x0000000000000088n, 0x0000000080008009n, 0x000000008000000an,
  0x000000008000808bn, 0x800000000000008bn, 0x8000000000008089n, 0x8000000000008003n,
  0x8000000000008002n, 0x8000000000000080n, 0x000000000000800an, 0x800000008000000an,
  0x8000000080008081n, 0x8000000000008080n, 0x0000000080000001n, 0x8000000080008008n,
];
const _ROT = [
  [0, 36, 3, 41, 18], [1, 44, 10, 45, 2], [62, 6, 43, 15, 61],
  [28, 55, 25, 21, 56], [27, 20, 39, 8, 14],
];
const _M64 = (1n << 64n) - 1n;
const _rotl = (x, n) => ((x << BigInt(n)) | (x >> BigInt(64 - n))) & _M64;

function _keccakF(s) {
  for (let round = 0; round < 24; round++) {
    const c = new Array(5);
    for (let x = 0; x < 5; x++) c[x] = s[x] ^ s[x + 5] ^ s[x + 10] ^ s[x + 15] ^ s[x + 20];
    const d = new Array(5);
    for (let x = 0; x < 5; x++) d[x] = c[(x + 4) % 5] ^ _rotl(c[(x + 1) % 5], 1);
    for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) s[x + 5 * y] ^= d[x];
    const b = new Array(25);
    for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) {
      b[y + 5 * ((2 * x + 3 * y) % 5)] = _rotl(s[x + 5 * y], _ROT[x][y]);
    }
    for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) {
      s[x + 5 * y] = b[x + 5 * y] ^ (~b[((x + 1) % 5) + 5 * y] & b[((x + 2) % 5) + 5 * y]);
    }
    s[0] ^= _RC[round];
  }
}

/** keccak256(bytes) -> 0x-hex digest (32 bytes). Rate 136 bytes, pad 0x01/0x80. */
export function keccak256(input) {
  const msg = typeof input === 'string' ? Buffer.from(input, 'utf8') : Buffer.from(input);
  const rate = 136;
  const padded = Buffer.alloc(Math.ceil((msg.length + 1) / rate) * rate);
  msg.copy(padded);
  padded[msg.length] ^= 0x01;
  padded[padded.length - 1] ^= 0x80;
  const s = new Array(25).fill(0n);
  for (let off = 0; off < padded.length; off += rate) {
    for (let i = 0; i < rate / 8; i++) {
      let lane = 0n;
      for (let b = 0; b < 8; b++) lane |= BigInt(padded[off + i * 8 + b]) << BigInt(8 * b);
      s[i] ^= lane;
    }
    _keccakF(s);
  }
  const out = Buffer.alloc(32);
  for (let i = 0; i < 4; i++) {
    let lane = s[i];
    for (let b = 0; b < 8; b++) {
      out[i * 8 + b] = Number(lane & 0xffn);
      lane >>= 8n;
    }
  }
  return '0x' + out.toString('hex');
}

/** 4-byte function selector = first 4 bytes of keccak256("<sig>"). */
export function selector(sig) {
  return keccak256(sig).slice(0, 10); // '0x' + 8 hex
}

// -------------------------------------------------------- on-chain feedback

export async function ethCall(data) {
  const res = await fetch(RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_call',
      params: [{ to: DIAMOND, data }, 'latest'],
    }),
  });
  if (!res.ok) throw new Error(`RPC HTTP ${res.status}`);
  const json = await res.json();
  if (json.error) throw new Error(`RPC error: ${json.error.message || JSON.stringify(json.error)}`);
  return json.result;
}

/** `feedbackCount()` — total on-chain feedback entries (stable array length). */
export async function feedbackCount() {
  const hex = await ethCall(SEL_FEEDBACK_COUNT);
  return Number(BigInt(hex));
}

/** ABI-decode the `feedbackAt(uint256)` return: (address sender, uint64 ts,
 *  string text). Hand-rolled head/tail decode — the shape is fixed. */
export function decodeFeedbackAt(hex) {
  const buf = Buffer.from(hex.replace(/^0x/, ''), 'hex');
  const word = (i) => buf.subarray(i * 32, (i + 1) * 32);
  const sender = '0x' + word(0).subarray(12).toString('hex');
  const timestamp = Number(BigInt('0x' + word(1).toString('hex')));
  const strOffset = Number(BigInt('0x' + word(2).toString('hex')));
  const strLen = Number(BigInt('0x' + buf.subarray(strOffset, strOffset + 32).toString('hex')));
  const text = buf.subarray(strOffset + 32, strOffset + 32 + strLen).toString('utf8');
  return { sender, timestamp, text };
}

/** Read every UNSKIPPED feedback entry from contract state (the same stable
 *  0-based view harvest-feedback.sh prints — NOT the windowed log scan behind
 *  `localharness feedback --json`, which has no stable index). Resolved
 *  indices are skipped before the RPC so the read stays cheap. */
export async function fetchFeedback(skip = new Set()) {
  const count = await feedbackCount();
  const wanted = [];
  for (let i = 0; i < count; i++) if (!skip.has(i)) wanted.push(i);
  const out = [];
  const CHUNK = 8; // polite concurrency against the public RPC
  for (let at = 0; at < wanted.length; at += CHUNK) {
    const slice = wanted.slice(at, at + CHUNK);
    const rows = await Promise.all(
      slice.map(async (i) => {
        const arg = i.toString(16).padStart(64, '0');
        const hex = await ethCall(SEL_FEEDBACK_AT + arg);
        return { index: i, ...decodeFeedbackAt(hex) };
      }),
    );
    out.push(...rows);
  }
  return { count, entries: out };
}

/** Parse docs/feedback-resolved.txt: first whitespace token of every
 *  non-comment, non-blank line is a resolved index (same rule as
 *  harvest-feedback.sh's is_resolved awk). Missing file => empty set. */
export function readResolvedIndices(path = join(REPO_ROOT, 'docs', 'feedback-resolved.txt')) {
  const set = new Set();
  if (!existsSync(path)) return set;
  for (const line of readFileSync(path, 'utf8').split('\n')) {
    const t = line.trim();
    if (!t || t.startsWith('#')) continue;
    const idx = Number(t.split(/\s+/)[0]);
    if (Number.isInteger(idx) && idx >= 0) set.add(idx);
  }
  return set;
}

/** Mirror of the CLI's parse_qa_envelope: `qa/v1 source=<s> v<ver>: <body>`.
 *  Returns { source, version, body } or null for non-fleet text. */
export function parseQaEnvelope(text) {
  if (!text.startsWith('qa/v1 ')) return null;
  const rest = text.slice('qa/v1 '.length);
  const sep = rest.indexOf(': ');
  if (sep === -1) return null;
  const header = rest.slice(0, sep);
  const body = rest.slice(sep + 2);
  const toks = header.split(/\s+/);
  const source = toks.find((t) => t.startsWith('source='))?.slice('source='.length);
  const version = toks.find((t) => /^v\d/.test(t))?.slice(1);
  if (!source || !version || !body.trim()) return null;
  return { source, version, body };
}
