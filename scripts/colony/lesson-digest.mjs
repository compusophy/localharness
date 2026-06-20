#!/usr/bin/env node
// scripts/colony/lesson-digest.mjs — colony pipeline: GLOBAL LESSONS SWEEP.
//
// The lessons analog of the feedback sweep (sync-issues.mjs). Every localharness
// agent accumulates short self-recorded lessons on-chain (one per real error/
// correction; see src/lessons.rs + the record_lesson tool), folded only into
// THAT agent's prompt. This sweep harvests EVERY recent agent's lessons, curates
// one bounded GLOBAL set, and writes it to web/global-lessons.txt — which the web
// bundle serves and the browser folds into the DEFAULT system prompt (see
// src/app/chat/session.rs::fetch_global_lessons_section) AFTER each agent's own
// lessons. Net: one agent's hard-won lesson makes every future agent better.
//
// Source: the proxy's public read-only harvest GET <proxy>/api/lessons?recent=N
// (proxy/api/lessons.ts) — it walks nextId() down and returns the most-recently-
// registered agents that actually have lessons: { count, lessons: [{ id, name,
// lessons }] } where each `lessons` is that agent's blob (one lesson per line).
//
// Curation (mirrors src/lessons.rs invariants): flatten every blob to lines,
// drop empties/comments, NORMALIZE (collapse whitespace, lowercase) for dedup
// (first occurrence's original text wins), clamp each line to MAX_LINE_CHARS,
// and keep the most-recent MAX_LESSONS unique lines (recent agents are scanned
// first, so "most recent" = first seen). One lesson per line out.
//
// Zero npm deps: the harvest is a plain global fetch (node >= 18); the write is
// node:fs. Like the other colony scripts it is DRY-RUN BY DEFAULT — it prints the
// curated set and the diff vs the current file, and only writes with --live.
//
// Usage:
//   node scripts/colony/lesson-digest.mjs                 # dry run (default)
//   node scripts/colony/lesson-digest.mjs --live          # write web/global-lessons.txt
//   node scripts/colony/lesson-digest.mjs --recent 50     # widen the agent scan (cap 50)
//   node scripts/colony/lesson-digest.mjs --max 40        # cap on curated lines (default 40)
//   node scripts/colony/lesson-digest.mjs --out <path>    # override output file
// Env: LH_PROXY (proxy base URL, default the canonical credit proxy).

import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { REPO_ROOT, hasFlag, takeFlag } from './lib.mjs';

// Canonical credit proxy (CLAUDE.md). `/api/lessons` is the public harvest.
const PROXY = process.env.LH_PROXY || 'https://proxy-tau-ten-15.vercel.app';

// Curation bounds — mirror src/lessons.rs so the file the browser folds never
// exceeds what the prompt can absorb (the browser ALSO re-clamps as defense in
// depth: GLOBAL_LESSONS_MAX_LINES / _CHARS in session.rs).
const MAX_LESSONS_DEFAULT = 40;
const MAX_LINE_CHARS = 240;
// Agents to scan, newest-first. The proxy caps ?recent at 50 (RECENT_CAP).
const RECENT_DEFAULT = 50;

const LIVE = hasFlag('--live');
const RECENT = clampInt(takeFlag('--recent', String(RECENT_DEFAULT)), 1, 50, RECENT_DEFAULT);
const MAX_LESSONS = clampInt(takeFlag('--max', String(MAX_LESSONS_DEFAULT)), 1, 200, MAX_LESSONS_DEFAULT);
const OUT = takeFlag('--out', join(REPO_ROOT, 'web', 'global-lessons.txt'));

/** Header preserved at the top of the written file (comment lines the browser
 *  fold skips). Explains provenance so a human reading the file knows it is
 *  machine-curated, and seeds nothing the sweep can't reproduce. */
const FILE_HEADER = [
  '# global-lessons.txt — curated lessons the WHOLE platform learned.',
  '#',
  '# Machine-curated by scripts/colony/lesson-digest.mjs: it harvests every recent',
  '# agent\'s on-chain lessons via GET <proxy>/api/lessons, dedups + caps the set,',
  '# and overwrites this file (one lesson per line). The browser folds it into the',
  '# DEFAULT system prompt as "=== GLOBAL LESSONS (learned across the platform) ==="',
  '# AFTER each agent\'s own lessons. Comment lines (#) and blanks are ignored.',
  '#',
];

function clampInt(v, lo, hi, def) {
  const n = Number(v);
  if (!Number.isInteger(n)) return def;
  return Math.max(lo, Math.min(hi, n));
}

/** Normalized dedup key: collapse whitespace, lowercase, trim. Matches the
 *  spirit of src/lessons.rs::normalize (which also collapses newlines). */
function dupKey(line) {
  return line.replace(/\s+/g, ' ').trim().toLowerCase();
}

/** Curate the per-agent blobs into one bounded, deduped, newest-first list.
 *  `agents` is most-recent-first (the proxy returns highest tokenId first), so
 *  the FIRST occurrence of a lesson wins and "newest" lessons survive the cap. */
function curate(agents) {
  const seen = new Set();
  const out = [];
  for (const a of agents) {
    for (const raw of String(a.lessons || '').split('\n')) {
      const line = raw.trim();
      if (!line || line.startsWith('#')) continue;
      const clamped = [...line].slice(0, MAX_LINE_CHARS).join('');
      const key = dupKey(clamped);
      if (!key || seen.has(key)) continue;
      seen.add(key);
      out.push(clamped);
      if (out.length >= MAX_LESSONS) return out;
    }
  }
  return out;
}

/** Current curated lines in the output file (comments/blanks stripped) — for
 *  the dry-run diff so an operator sees what WOULD change. */
function currentLines(path) {
  if (!existsSync(path)) return [];
  return readFileSync(path, 'utf8')
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l && !l.startsWith('#'));
}

async function fetchHarvest() {
  const url = `${PROXY}/api/lessons?recent=${RECENT}`;
  const res = await fetch(url, { headers: { accept: 'application/json' } });
  if (!res.ok) throw new Error(`GET ${url} -> HTTP ${res.status}`);
  const body = await res.json();
  if (!Array.isArray(body.lessons)) throw new Error('unexpected /api/lessons shape (no `lessons` array)');
  return body.lessons;
}

async function main() {
  console.log(`harvesting recent ${RECENT} agents' lessons from ${PROXY}/api/lessons …`);
  let agents = [];
  try {
    agents = await fetchHarvest();
  } catch (e) {
    // Offline / proxy down: do NOT clobber the seeded file. The dry run still
    // prints the existing curated set so the operator sees what's live.
    console.error(`!! harvest failed: ${e.message}`);
    console.error('   keeping the existing web/global-lessons.txt (no network = no overwrite).');
    const existing = currentLines(OUT);
    console.log(`\ncurrent ${OUT} has ${existing.length} curated lesson(s):`);
    for (const l of existing) console.log('  • ' + l);
    process.exit(LIVE ? 1 : 0);
  }

  console.log(`harvested lessons from ${agents.length} agent(s).`);
  const curated = curate(agents);
  const before = currentLines(OUT);

  console.log(
    `\ncurated ${curated.length} unique global lesson(s) (cap ${MAX_LESSONS}, ${MAX_LINE_CHARS} chars/line):\n`,
  );
  for (const l of curated) console.log('  • ' + l);

  // Diff vs the current file (set-wise, by normalized key).
  const beforeKeys = new Set(before.map(dupKey));
  const afterKeys = new Set(curated.map(dupKey));
  const added = curated.filter((l) => !beforeKeys.has(dupKey(l)));
  const dropped = before.filter((l) => !afterKeys.has(dupKey(l)));
  console.log(`\nvs current file: +${added.length} new, -${dropped.length} dropped`);
  for (const l of added) console.log('  + ' + l);
  for (const l of dropped) console.log('  - ' + l);

  if (!curated.length) {
    console.error('\nrefusing to write an EMPTY digest (every agent had no lessons, or the harvest was empty).');
    console.error('keeping the existing file so the fold still has the seeded platform lessons.');
    process.exit(LIVE ? 1 : 0);
  }

  const contents = [...FILE_HEADER, ...curated, ''].join('\n');
  if (!LIVE) {
    console.log(`\nDRY RUN — pass --live to overwrite ${OUT} (${curated.length} lesson(s)).`);
    return;
  }
  writeFileSync(OUT, contents, 'utf8');
  console.log(`\nwrote ${curated.length} global lesson(s) to ${OUT}.`);
  console.log('redeploy the web bundle to serve it: vercel deploy --prod --yes');
}

main().catch((e) => {
  console.error('lesson-digest failed: ' + e.message);
  process.exit(1);
});
