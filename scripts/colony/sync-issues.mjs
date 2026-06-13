#!/usr/bin/env node
// scripts/colony/sync-issues.mjs — colony pipeline rung 1: on-chain feedback
// -> GitHub issues, idempotently.
//
// Reads the FeedbackFacet by STABLE 0-based index (contract-state views, same
// numbering as scripts/harvest-feedback.sh and docs/feedback-resolved.txt),
// then EXCLUDES, in order:
//   1. indices listed in docs/feedback-resolved.txt (already addressed),
//   2. indices that already have a matching issue, OPEN OR CLOSED — matched by
//      the visible `lh-feedback:<index>` marker line stamped into every issue
//      body this script creates (found via ONE `gh issue list --search`). NOTE
//      state=all is load-bearing: deduping on open-only would RE-FILE an index
//      the moment its issue is closed (the bug that re-created the backlog),
//   3. exact-duplicate feedback texts (normalized whitespace + case collapse;
//      the first index wins, dups are listed in its footer).
//
// What remains becomes one GitHub issue each: title = first ~70 chars,
// body = full text + the marker line + an on-chain provenance footer,
// label `colony`.
//
// DRY-RUN BY DEFAULT — prints exactly what WOULD be created and touches
// nothing on GitHub. `--live` opts in to `gh issue create` (maintainer-only).
//
// Usage:
//   node scripts/colony/sync-issues.mjs                 # dry run (default)
//   node scripts/colony/sync-issues.mjs --live          # create the issues
//   node scripts/colony/sync-issues.mjs --resolved <p>  # override resolved file
// Env: LH_REPO, DIAMOND, RPC, GH_TOKEN (honored by gh automatically).

import {
  REPO,
  DIAMOND,
  MARKER_PREFIX,
  hasFlag,
  takeFlag,
  fmtCmd,
  gh,
  fetchFeedback,
  readResolvedIndices,
  parseQaEnvelope,
} from './lib.mjs';

const LIVE = hasFlag('--live');
const LABEL = 'colony';
// Optional category filter: --tag BUG|FEATURE|FEEDBACK files only feedback whose
// text carries that `[TAG]` prefix. Lets a run surface just the high-signal bugs
// without flooding the tracker with every opinion/feature item at once.
const TAG = (takeFlag('--tag', null) || '').toUpperCase() || null;

/** The `[BUG]`/`[FEATURE]`/`[FEEDBACK]` tag of a feedback item, or null. */
function tagOf(text) {
  const m = effectiveText(text).match(/^\s*\[(BUG|FEATURE|FEEDBACK)\]/i);
  return m ? m[1].toUpperCase() : null;
}

/** Effective text: the decoded body for qa/v1 fleet envelopes, raw otherwise. */
function effectiveText(text) {
  return parseQaEnvelope(text)?.body ?? text;
}

/** Exact-dup collapse key: whitespace-normalized, case-folded effective text. */
function dupKey(text) {
  return effectiveText(text).replace(/\s+/g, ' ').trim().toLowerCase();
}

/** Issue title: first ~70 chars of the flattened effective text. */
function titleFrom(text) {
  const s = effectiveText(text).replace(/\s+/g, ' ').trim();
  return s.length > 70 ? s.slice(0, 69) + '…' : s;
}

/** Issue body: full text + marker line + on-chain provenance footer. */
function bodyFrom(entry, dupIndices) {
  const env = parseQaEnvelope(entry.text);
  const iso = new Date(entry.timestamp * 1000).toISOString();
  return [
    entry.text,
    '',
    `${MARKER_PREFIX}${entry.index}`,
    '',
    '---',
    'Filed on-chain via the localharness `FeedbackFacet` (colony pipeline).',
    `- submitter: \`${entry.sender}\``,
    `- on-chain timestamp: ${entry.timestamp} (${iso})`,
    `- on-chain index: ${entry.index} (diamond \`${DIAMOND}\`; read with \`scripts/harvest-feedback.sh\`)`,
    env ? `- fleet source: ${env.source} (v${env.version})` : '',
    dupIndices.length ? `- exact duplicates collapsed into this issue: index ${dupIndices.join(', ')}` : '',
  ]
    .filter((l) => l !== '')
    .join('\n');
}

async function main() {
  // 1. Resolved indices (off-chain "addressed" ledger).
  const resolved = readResolvedIndices(takeFlag('--resolved', undefined));

  // 2. On-chain feedback minus resolved.
  const { count, entries } = await fetchFeedback(resolved);
  console.log(
    `on-chain feedback: ${count} total, ${resolved.size} resolved in docs/feedback-resolved.txt, ` +
      `${entries.length} candidate(s)`,
  );

  // 3. Indices already tracked by an issue (OPEN OR CLOSED) — one search,
  //    marker contract. GitHub's tokenizer splits on ':' so searching the
  //    prefix finds bodies containing `lh-feedback:<n>`. state=all so a CLOSED
  //    (resolved) issue keeps its index tracked — deduping open-only re-files
  //    everything the moment it's closed.
  let trackedMarked = new Set();
  try {
    const raw = gh([
      'issue', 'list',
      '--state', 'all',
      '--search', MARKER_PREFIX.replace(/:$/, ''),
      '--json', 'number,body',
      '--limit', '400',
    ]);
    const markerRe = new RegExp(`${MARKER_PREFIX}(\\d+)\\b`, 'g');
    for (const issue of JSON.parse(raw)) {
      for (const m of (issue.body || '').matchAll(markerRe)) {
        trackedMarked.add(Number(m[1]));
      }
    }
  } catch (e) {
    console.error(`warning: could not query issues (${e.message}) — assuming none tracked.`);
    if (LIVE) {
      console.error('refusing --live without the dedup check.');
      process.exit(1);
    }
  }
  let untracked = entries.filter((e) => !trackedMarked.has(e.index));
  console.log(`existing issues (open+closed) already track ${trackedMarked.size} index(es); ${untracked.length} remain`);
  if (TAG) {
    const before = untracked.length;
    untracked = untracked.filter((e) => tagOf(e.text) === TAG);
    console.log(`--tag ${TAG}: ${untracked.length} of ${before} match (rest left on-chain, unfiled)`);
  }

  // 4. Exact-dup collapse (first index wins; dups recorded in the footer).
  const byKey = new Map();
  for (const e of untracked) {
    const k = dupKey(e.text);
    if (byKey.has(k)) byKey.get(k).dups.push(e.index);
    else byKey.set(k, { entry: e, dups: [] });
  }
  const candidates = [...byKey.values()];

  if (!candidates.length) {
    console.log('nothing to sync — every unresolved feedback entry is already tracked.');
    return;
  }

  console.log(
    `\n${candidates.length} issue(s) ${LIVE ? 'to create:' : 'WOULD be created — DRY RUN (pass --live to create):'}\n`,
  );

  // In live mode, make sure the label exists (idempotent, maintainer-gated).
  if (LIVE) {
    try {
      gh(['label', 'create', LABEL, '--force', '--description', 'colony pipeline: on-chain feedback']);
    } catch {
      /* best-effort; per-issue failures surface below */
    }
  }

  let created = 0;
  for (const { entry, dups } of candidates) {
    const title = titleFrom(entry.text);
    const body = bodyFrom(entry, dups);
    const argv = ['gh', 'issue', 'create', '--repo', REPO, '--title', title, '--body', '<body below>', '--label', LABEL];
    console.log(`• index ${entry.index}${dups.length ? ` (+dups ${dups.join(',')})` : ''}: ${title}`);
    if (!LIVE) {
      console.log(`  ${fmtCmd(argv)}`);
      console.log('  body:');
      console.log(body.split('\n').map((l) => '  | ' + l).join('\n') + '\n');
      continue;
    }
    try {
      const out = gh(['issue', 'create', '--title', title, '--body', body, '--label', LABEL]);
      console.log('  created: ' + out.trim());
      created++;
    } catch (e) {
      console.error('  FAILED: ' + e.message);
    }
  }

  console.log(
    LIVE
      ? `\ncreated ${created}/${candidates.length} issue(s) on ${REPO}.`
      : `\ndry run complete — ${candidates.length} issue(s) would be created on ${REPO}.`,
  );
}

main().catch((e) => {
  console.error('sync-issues failed: ' + e.message);
  process.exit(1);
});
