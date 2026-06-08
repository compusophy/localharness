#!/usr/bin/env node
// scripts/test-fleet/feedback-to-issues.mjs
//
// Bridge: on-chain FeedbackFacet entries -> GitHub issues. The first rung of
// "agents file their own issues" — the test-user fleet files grounded feedback
// on-chain (run-fleet.sh), and this surfaces NEW entries as GitHub issues on the
// repo so they're tracked + actionable.
//
// DRY-RUN by default (prints what it WOULD file, touches nothing). Pass --create
// to actually open issues — that needs `gh` authed (`gh auth status`). Creating
// public issues is outward-facing, so it is opt-in, never the default.
//
// Idempotent: a ledger (docs/feedback-bridged.txt) of `<timestamp>:<sender>`
// keys already turned into issues, so re-runs only file genuinely new feedback.
// `list_feedback` is a windowed log scan with no stable on-chain index, so the
// (timestamp, sender) pair is the dedup key.
//
// Usage:
//   node scripts/test-fleet/feedback-to-issues.mjs            # dry run
//   node scripts/test-fleet/feedback-to-issues.mjs --create   # file the issues
//   LOCALHARNESS_BIN=/path/to/localharness node …             # override the CLI

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, appendFileSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';

const CREATE = process.argv.includes('--create');
const CLI =
  process.env.LOCALHARNESS_BIN ||
  (existsSync('./target/debug/localharness.exe')
    ? './target/debug/localharness.exe'
    : './target/debug/localharness');
const LEDGER = 'docs/feedback-bridged.txt';

// 1. Read the on-chain feedback log as JSON.
let feedback;
try {
  const raw = execFileSync(CLI, ['feedback', '--json'], { encoding: 'utf8', maxBuffer: 16 << 20 });
  feedback = JSON.parse(raw);
} catch (e) {
  console.error('could not read feedback — build the CLI first (`cargo build --features wallet`):');
  console.error('  ' + e.message);
  process.exit(1);
}

// 2. Dedup ledger (first whitespace token of each non-empty line = the key).
const seen = new Set(
  existsSync(LEDGER)
    ? readFileSync(LEDGER, 'utf8')
        .split('\n')
        .map((l) => l.trim().split(/\s+/)[0])
        .filter(Boolean)
    : [],
);

const TAGS = [
  { re: /^\s*\[BUG\]/i, label: 'bug' },
  { re: /^\s*\[FEATURE\]/i, label: 'enhancement' },
  { re: /^\s*\[FEEDBACK\]/i, label: 'feedback' },
];
const classify = (t) => (TAGS.find((x) => x.re.test(t)) || { label: 'feedback' }).label;
const titleFrom = (t) => {
  const s = t.replace(/\s+/g, ' ').trim();
  return s.length > 72 ? s.slice(0, 71) + '…' : s;
};

const fresh = feedback.filter((e) => !seen.has(`${e.timestamp}:${e.sender}`));
if (!fresh.length) {
  console.log(`no new on-chain feedback to bridge (${feedback.length} total, all already filed).`);
  process.exit(0);
}

console.log(
  `${fresh.length} new on-chain feedback item(s)` +
    (CREATE ? ':' : ' — DRY RUN (pass --create to file them):') +
    '\n',
);

// In --create mode, make sure the labels exist (idempotent, best-effort).
if (CREATE) {
  for (const l of ['bug', 'enhancement', 'feedback', 'from-fleet']) {
    try {
      execFileSync('gh', ['label', 'create', l, '--force'], { stdio: 'ignore' });
    } catch {
      /* label may already exist or gh may be unauthed — surfaced per-issue below */
    }
  }
}

let filed = 0;
for (const e of fresh) {
  const key = `${e.timestamp}:${e.sender}`;
  const text = e.body || e.text; // qa/v1 envelopes carry a decoded `body`
  const label = classify(text);
  const title = titleFrom(text);
  const body = [
    'Filed on-chain by the localharness test-user fleet.',
    '',
    '> ' + text.replace(/\n/g, '\n> '),
    '',
    `- **submitter:** \`${e.sender}\``,
    `- **on-chain timestamp:** ${e.timestamp}`,
    e.fleet_source ? `- **fleet source:** ${e.fleet_source}` : '',
    '- **source:** localharness `FeedbackFacet` (read with `localharness feedback`)',
    '',
    `<!-- bridge-key: ${key} -->`,
  ]
    .filter((x) => x !== '')
    .join('\n');

  console.log(`• [${label}] ${title}`);
  if (!CREATE) continue;
  try {
    const out = execFileSync(
      'gh',
      ['issue', 'create', '--title', title, '--body', body, '--label', label, '--label', 'from-fleet'],
      { encoding: 'utf8' },
    );
    console.log('  ✓ ' + out.trim());
    mkdirSync(dirname(LEDGER), { recursive: true });
    appendFileSync(LEDGER, `${key}\t${label}\t${title.replace(/\t/g, ' ')}\n`);
    filed++;
  } catch (err) {
    console.error('  ✗ gh issue create failed:', err.message.split('\n')[0]);
  }
}

console.log(
  CREATE
    ? `\nfiled ${filed} issue(s); dedup ledger: ${LEDGER}`
    : `\ndry run — re-run with --create to file these (needs \`gh\` authed).`,
);
