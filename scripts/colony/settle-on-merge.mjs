#!/usr/bin/env node
// scripts/colony/settle-on-merge.mjs — colony pipeline rung 3: a MERGED PR
// settles the bounty escrow to the worker's TBA.
//
// Given an issue number, a bounty id, and the worker's agent name, this:
//   1. verifies via gh (read-only) that a PR referencing the issue is MERGED
//      — gh 2.45 has no closedByPullRequestsReferences field, so the check
//      scans merged PRs for a `#<issue>` reference in title/body (closing
//      keywords preferred, plain reference accepted). NOT merged => exit 1.
//   2. verifies via `localharness bounty show <id>` (read-only) that the
//      bounty result is SUBMITTED and the claimant matches --worker (the
//      facet pays the CLAIMANT's identity — this catches paying a squatter).
//   3. prepares the settlement (real syntax from src/bin/localharness/
//      bounty.rs — accept is run by the POSTER and pays the claimant's TBA):
//
//        localharness bounty accept --as <poster> <bounty-id>
//
// DRY-RUN BY DEFAULT — prints the exact command, settles nothing. `--live`
// opts in to paying out the escrow (maintainer-only).
//
// Usage:
//   node scripts/colony/settle-on-merge.mjs --issue <n> --bounty <id> --worker <name>
//       [--poster claude] [--live] [--force]
// Env: LH_REPO, LOCALHARNESS_BIN, GH_TOKEN (honored by gh automatically).

import { hasFlag, takeFlag, fmtCmd, gh, resolveCli, runCli } from './lib.mjs';

const LIVE = hasFlag('--live');
const FORCE = hasFlag('--force');

function usage() {
  console.error(
    'usage: node scripts/colony/settle-on-merge.mjs --issue <n> --bounty <id> --worker <name> [--poster claude] [--live] [--force]',
  );
  process.exit(2);
}

/** Find merged PRs referencing #issueNum (title or body). Closing-keyword
 *  matches rank first so "Closes #N" beats a passing mention. */
function mergedPrsReferencing(issueNum) {
  const prs = JSON.parse(
    gh(['pr', 'list', '--state', 'merged', '--json', 'number,title,body,url,mergedAt', '--limit', '200']),
  );
  const ref = new RegExp(`(^|[^\\w/])#${issueNum}\\b`);
  const closing = new RegExp(
    `\\b(close[sd]?|fix(?:e[sd])?|resolve[sd]?)(\\s*:)?\\s+([\\w.-]+/[\\w.-]+)?#${issueNum}\\b`,
    'i',
  );
  const hits = prs
    .map((p) => ({ ...p, closes: closing.test(p.title || '') || closing.test(p.body || '') }))
    // plain `#N` reference OR a closing keyword with an owner/repo#N qualifier
    // (the qualified form has a word char before `#`, so `ref` alone misses it)
    .filter((p) => p.closes || ref.test(p.title || '') || ref.test(p.body || ''));
  hits.sort((a, b) => Number(b.closes) - Number(a.closes));
  return hits;
}

/** Read bounty state through the CLI (read-only `bounty show`). Returns
 *  { status, claimant } parsed from the human output, or null when the CLI
 *  is unavailable/unbuilt. */
function bountyState(bountyId) {
  let out;
  try {
    out = runCli(['bounty', 'show', String(bountyId)]);
  } catch (e) {
    console.error(`warning: could not read bounty #${bountyId} via the CLI (${e.message.split('\n')[0]})`);
    return null;
  }
  const status = out.match(/\[(open|claimed|submitted|paid|cancelled|reclaimed|unknown)\]/)?.[1];
  const claimant = out.match(/claimant\s+(.+)/)?.[1]?.trim();
  return { status, claimant, raw: out };
}

async function main() {
  const issueNum = Number(takeFlag('--issue'));
  const bountyId = Number(takeFlag('--bounty'));
  const worker = takeFlag('--worker');
  const poster = takeFlag('--poster', 'claude');
  if (!Number.isInteger(issueNum) || issueNum <= 0) usage();
  if (!Number.isInteger(bountyId) || bountyId < 0) usage();
  if (!worker) usage();

  // 1. THE GATE: a merged PR must reference the issue.
  const issue = JSON.parse(gh(['issue', 'view', String(issueNum), '--json', 'number,title,state,url']));
  console.log(`issue #${issue.number}: ${issue.title}  [${issue.state}]`);

  const merged = mergedPrsReferencing(issueNum);
  if (!merged.length) {
    console.error(`\nREFUSING: no MERGED PR references issue #${issueNum} — the escrow stays locked.`);
    console.error('(the colony pays for merged work, not for submitted text; merge the PR first.)');
    process.exit(1);
  }
  const pr = merged[0];
  console.log(
    `merged PR found: #${pr.number} "${pr.title}" (merged ${pr.mergedAt})${pr.closes ? ' [closing reference]' : ' [plain reference]'}`,
  );
  console.log(`  ${pr.url}`);

  // 2. Bounty sanity: result submitted, claimant == worker.
  const state = bountyState(bountyId);
  if (state) {
    console.log(`bounty #${bountyId}: status=${state.status ?? '?'}  claimant=${state.claimant ?? '(none)'}`);
    if (state.status === 'paid') {
      console.log('already settled — nothing to do.');
      return;
    }
    if (state.status !== 'submitted' && !FORCE) {
      console.error(`\nREFUSING: bounty #${bountyId} is '${state.status}', not 'submitted' — the worker`);
      console.error('must `bounty submit <id> <PR url>` before settlement (pass --force to override).');
      process.exit(1);
    }
    const claimantName = (state.claimant || '').split(/\s+/)[0];
    if (claimantName !== worker && !FORCE) {
      console.error(`\nREFUSING: claimant '${state.claimant}' does not match --worker '${worker}'`);
      console.error('(accept pays the CLAIMANT; pass --force only if you mean to pay them anyway).');
      process.exit(1);
    }
  } else if (LIVE && !FORCE) {
    console.error('\nREFUSING --live: cannot verify the bounty claimant without the localharness CLI.');
    console.error('(build it: cargo build --features wallet — or pass --force to skip the check.)');
    process.exit(1);
  }

  // 3. The settlement (pays the claimant's TBA on --live).
  const cliArgs = ['bounty', 'accept', '--as', poster, String(bountyId)];
  const display = fmtCmd([resolveCli(), ...cliArgs]);

  if (!LIVE) {
    console.log('\nDRY RUN — would settle the escrow (pass --live to pay out for real):');
    console.log('  ' + display);
    console.log(`\n  pays bounty #${bountyId}'s escrow to ${worker}'s TBA (the facet binds payout to the claimed identity).`);
    return;
  }

  console.log('\nsettling (pays real $LH to the claimant\'s TBA) …');
  console.log('  ' + display);
  runCli(cliArgs, { inherit: true });
}

main().catch((e) => {
  console.error('settle-on-merge failed: ' + e.message);
  process.exit(1);
});
