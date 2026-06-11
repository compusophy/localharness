#!/usr/bin/env node
// scripts/colony/issue-to-bounty.mjs — colony pipeline rung 2: GitHub issue ->
// on-chain bounty (the demand signal an agent can claim and work).
//
// Reads the issue via gh (read-only), derives the bounty task text, and
// prepares the escrow post through the localharness CLI:
//
//   localharness bounty post --as <poster> "fix #<n> — <title> (issue: <url>)
//     (repo: compusophy/localharness)" --reward <amt> [--ttl <dur>]
//
// (real syntax from src/bin/localharness/bounty.rs: `bounty post [--as <me>]
// <task...> --reward <amt> [--ttl <dur>]`; the task is the joined positional
// remainder, so it is passed as ONE argv element — no shell quoting games.)
//
// DRY-RUN BY DEFAULT — prints the exact command and posts nothing. `--live`
// opts in to actually escrowing $LH (maintainer-only; spends the poster's
// wallet balance via a sponsored tx).
//
// Usage:
//   node scripts/colony/issue-to-bounty.mjs <issue-number>
//       [--reward 0.5] [--as claude] [--ttl 7d] [--live] [--force]
// Env: LH_REPO, LOCALHARNESS_BIN, GH_TOKEN (honored by gh automatically).

import { REPO, MARKER_PREFIX, hasFlag, takeFlag, positionals, fmtCmd, gh, resolveCli, runCli } from './lib.mjs';

const LIVE = hasFlag('--live');
const FORCE = hasFlag('--force');

function usage() {
  console.error(
    'usage: node scripts/colony/issue-to-bounty.mjs <issue-number> [--reward 0.5] [--as claude] [--ttl 7d] [--live] [--force]',
  );
  process.exit(2);
}

async function main() {
  const pos = positionals(['--reward', '--as', '--ttl'], ['--live', '--force']);
  const issueNum = Number(pos[0]);
  if (pos.length !== 1 || !Number.isInteger(issueNum) || issueNum <= 0) usage();

  const reward = takeFlag('--reward', '0.5');
  const poster = takeFlag('--as', 'claude');
  const ttl = takeFlag('--ttl', undefined);
  if (!/^\d+(\.\d+)?$/.test(reward) || Number(reward) <= 0) {
    console.error(`--reward must be a positive $LH amount, got '${reward}'`);
    process.exit(2);
  }

  // 1. Read the issue (read-only — fine in dry-run too).
  const issue = JSON.parse(
    gh(['issue', 'view', String(issueNum), '--json', 'number,title,url,state,body,labels']),
  );
  const labels = (issue.labels || []).map((l) => l.name).join(', ') || '(none)';
  console.log(`issue #${issue.number}: ${issue.title}`);
  console.log(`  state: ${issue.state}   labels: ${labels}`);
  console.log(`  url:   ${issue.url}`);
  const marker = (issue.body || '').match(new RegExp(`${MARKER_PREFIX}(\\d+)\\b`));
  if (marker) console.log(`  on-chain feedback index: ${marker[1]}`);

  if (issue.state !== 'OPEN' && !FORCE) {
    console.error(`refusing: issue #${issueNum} is ${issue.state}, not OPEN (pass --force to override).`);
    process.exit(1);
  }

  // 2. Derive the task text (issue URL included so a claiming agent can read
  //    the full context; repo named so the PR lands in the right place).
  const task = `fix #${issue.number} — ${issue.title} (issue: ${issue.url}) (repo: ${REPO})`;

  // 3. The bounty post (escrows the poster's $LH on --live).
  const cliArgs = ['bounty', 'post', '--as', poster, task, '--reward', reward];
  if (ttl) cliArgs.push('--ttl', ttl);
  const display = fmtCmd([resolveCli(), ...cliArgs]);

  if (!LIVE) {
    console.log('\nDRY RUN — would post this bounty (pass --live to escrow for real):');
    console.log('  ' + display);
    console.log(`\n  reward: ${reward} $LH escrowed from '${poster}'s wallet, paid to the claimant's TBA on accept.`);
    return;
  }

  console.log('\nposting bounty (escrows real $LH) …');
  console.log('  ' + display);
  runCli(cliArgs, { inherit: true });
  console.log('\nnext: an agent claims it (`localharness bounty claim --as <agent> <id>`),');
  console.log(`authors a PR that references #${issue.number}, and submits the PR URL as the result.`);
}

main().catch((e) => {
  console.error('issue-to-bounty failed: ' + e.message);
  process.exit(1);
});
