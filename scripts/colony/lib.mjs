// scripts/colony/lib.mjs — shared plumbing for the colony pipeline scripts
// (issue-to-bounty / settle-on-merge / build-board). Zero npm deps: on-chain
// reads are raw JSON-RPC eth_call via global fetch (node >= 18), GitHub access
// is the `gh` CLI as a subprocess (execFileSync with arg arrays — no shell, so
// it is Windows-safe), and on-chain WRITES go through the `localharness` CLI.
//
// Auth model: every gh call runs AS THE COLONY BOT (compusophy-bot), not the
// maintainer. The bot PAT lives in `.env` as `GH_API_KEY` (alongside
// EVM_PRIVATE_KEY); `botEnv()` loads it and injects it as `GH_TOKEN` (the var
// `gh` actually honors) into the child env. Precedence: an explicit `GH_TOKEN`
// already in the environment wins; else `.env`'s `GH_API_KEY`; else `.env`'s
// `GH_TOKEN`; else the child inherits the ambient env (gh falls back to the
// logged-in account). This is why issues/PRs are authored by the bot, not the
// human who happens to be `gh auth`'d — the bug that made every early issue
// read as `compusophy` instead of `compusophy-bot`.

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

// ------------------------------------------------------------------ bot auth

/** Minimal `.env` reader: value of KEY from REPO_ROOT/.env (quotes + inline
 *  whitespace stripped), or undefined. No npm dep; tolerant of comments and
 *  `export ` prefixes. */
function envFileValue(key, path = join(REPO_ROOT, '.env')) {
  if (!existsSync(path)) return undefined;
  for (const raw of readFileSync(path, 'utf8').split('\n')) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const m = line.match(/^(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/);
    if (m && m[1] === key) return m[2].trim().replace(/^['"]|['"]$/g, '');
  }
  return undefined;
}

let _botTokenMemo; // resolve once per process
/** The colony bot PAT, or undefined. Precedence: ambient GH_TOKEN > .env
 *  GH_API_KEY > .env GH_TOKEN. */
export function loadBotToken() {
  if (_botTokenMemo !== undefined) return _botTokenMemo || undefined;
  _botTokenMemo =
    process.env.GH_TOKEN ||
    envFileValue('GH_API_KEY') ||
    envFileValue('GH_TOKEN') ||
    '';
  return _botTokenMemo || undefined;
}

let _warnedNoBot = false;
/** Child env for `gh` with the bot token injected as GH_TOKEN. Falls back to
 *  the ambient env (logged-in account) with a one-time warning if no token is
 *  found — so a bot-less checkout still works, just not as the bot. */
export function botEnv() {
  const token = loadBotToken();
  if (!token) {
    if (!_warnedNoBot) {
      console.error('!! no GH_API_KEY/GH_TOKEN found — gh runs as the logged-in account, NOT the bot');
      _warnedNoBot = true;
    }
    return process.env;
  }
  return { ...process.env, GH_TOKEN: token };
}

// ------------------------------------------------------------------ gh + CLI

/** Run `gh <args> --repo REPO` AS THE BOT (see botEnv), return stdout. Throws
 *  with gh's stderr line on failure. READ-ONLY callers only, except behind an
 *  explicit --live gate. */
export function gh(args, { repoFlag = true } = {}) {
  const full = repoFlag ? [...args, '--repo', REPO] : args;
  try {
    return execFileSync('gh', full, { encoding: 'utf8', maxBuffer: 64 << 20, env: botEnv() });
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
// Verified against known `cast sig` selectors (e.g. getBounty(uint256)).
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

// ---------------------------------------------------------- on-chain reads

const _sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function ethCall(data, { retries = 6 } = {}) {
  // The public Tempo RPC rate-limits (429) and occasionally 5xxs, especially
  // right after a fleet run hammers it. Exponential backoff makes the colony
  // bridge resilient instead of dying on the first throttle.
  let delay = 700;
  for (let attempt = 0; ; attempt++) {
    let res;
    try {
      res = await fetch(RPC, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          jsonrpc: '2.0',
          id: 1,
          method: 'eth_call',
          params: [{ to: DIAMOND, data }, 'latest'],
        }),
      });
    } catch (e) {
      if (attempt >= retries) throw e;
      await _sleep(delay);
      delay = Math.min(delay * 2, 8000);
      continue;
    }
    if (res.status === 429 || res.status >= 500) {
      if (attempt >= retries) throw new Error(`RPC HTTP ${res.status} after ${retries} retries`);
      await _sleep(delay);
      delay = Math.min(delay * 2, 8000);
      continue;
    }
    if (!res.ok) throw new Error(`RPC HTTP ${res.status}`);
    const json = await res.json();
    if (json.error) throw new Error(`RPC error: ${json.error.message || JSON.stringify(json.error)}`);
    return json.result;
  }
}

