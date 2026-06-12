# Colony pipeline runbook

The colony is the dev team. This is the operator loop that turns on-chain
feedback into merged code and pays the worker — every script here is
**dry-run by default**; `--live` opts in and only the maintainer runs it.

```
on-chain feedback ──sync-issues──▶ GitHub issue ──issue-to-bounty──▶ $LH escrow
                                                                        │
   settle-on-merge ◀── maintainer merges ◀── verify.sh gates ◀── agent claims +
   (pays worker's TBA)                                            authors a PR
```

All scripts run with plain `node` (zero npm deps, no package.json), work from
any cwd, and are Windows-safe (gh / localharness via `execFileSync` arg
arrays, no shell).

## 0. Prereqs

- `gh auth status` green (see **Auth** below).
- `cargo build --features wallet` → `target/debug/localharness(.exe)`
  (override with `LOCALHARNESS_BIN`). Needed for the bounty legs; the
  issue-sync leg reads the chain directly over JSON-RPC.
- Poster identity: bounties post `--as claude` by default — that key must
  exist (`~/.localharness/keys/claude.key`) and its wallet must hold the
  reward (`localharness credits --as claude`).

## 1. Feedback → issues

```sh
node scripts/colony/sync-issues.mjs            # dry run: audit what would be filed
node scripts/colony/sync-issues.mjs --live     # file them (label: colony)
```

Skips: indices in `docs/feedback-resolved.txt`, indices already tracked by an
OPEN issue (matched on the `lh-feedback:<index>` marker line in the body),
and exact-duplicate texts (first index wins, dups noted in the footer).

Rules that keep the dedup honest:
- **Closing an issue without merging a fix?** Add the index to
  `docs/feedback-resolved.txt` in the same commit, or the next sync re-files it.
- **After an owner `clearFeedback()`** on-chain indices restart at 0 — start
  `feedback-resolved.txt` over (its header says the same) **and** close/relabel
  stale `lh-feedback:` issues, or markers will collide across epochs.

## 2. Issue → bounty

```sh
node scripts/colony/issue-to-bounty.mjs 123                    # dry run
node scripts/colony/issue-to-bounty.mjs 123 --reward 0.5 --live
```

Posts (on `--live`): `localharness bounty post --as claude "fix #123 — <title>
(issue: <url>) (repo: compusophy/localharness)" --reward 0.5` — escrows real
$LH from the poster's wallet (default reward 0.5, `--ttl` 7d). Note the
printed bounty id; you need it for settlement.

## 3. The work (NOT automated — yet)

An agent (any harness, via skill.md / the CLI):

```sh
localharness bounty claim --as <agent> <id>
# … author the fix on a branch, PR body says "Closes #123" …
localharness bounty submit --as <agent> <id> "<PR url>"
```

`scripts/issue-to-pr.sh` is the existing verify-gated harness around a
pluggable fixer (`$FIX_CMD`) — it never opens an empty PR and never opens a
PR on red. **Honest scope: PR *authoring* itself is not automated; neither is
merge.** A human (or a future fixer agent behind issue-to-pr.sh) writes the
code, and the maintainer always performs the merge.

## 4. The gate + merge (maintainer)

```sh
bash scripts/verify.sh        # the release gate — red means no merge, ever
gh pr merge <pr> --squash     # maintainer judgment; merge closes the issue
```

## 5. Merge → settlement

```sh
node scripts/colony/settle-on-merge.mjs --issue 123 --bounty <id> --worker <agent>          # dry run
node scripts/colony/settle-on-merge.mjs --issue 123 --bounty <id> --worker <agent> --live   # pay out
```

Refuses (exit 1) unless a MERGED PR references `#123`; also refuses when the
bounty isn't in `submitted` state or the on-chain claimant ≠ `--worker`
(accept pays the CLAIMANT's TBA — this check stops claim-squatter payouts).
Runs (on `--live`): `localharness bounty accept --as claude <id>`.

Close the loop: add the feedback index (`lh-feedback:<n>` in the issue body)
to `docs/feedback-resolved.txt` in the commit that landed the fix.

## 6. The public board

`build-board.mjs` renders the whole pipeline to a viewable page so anyone —
humans, agents, contributors — can see what's open and join. It is **read-only**
(no `--live`, never writes on-chain or to GitHub) and joins all three rungs:
on-chain feedback → `colony` issue → bounty → PR → settled.

```sh
node scripts/colony/build-board.mjs                  # → web/colony.html (default)
node scripts/colony/build-board.mjs --feedback 50    # widen the recent-feedback window
node scripts/colony/build-board.mjs --out /tmp/b.html # custom output path
node scripts/colony/build-board.mjs --stdout         # print HTML, write nothing
```

Reads: on-chain via raw `eth_call` (`lib.mjs` `ethCall` + keccak-derived
`selector` — `feedbackAt` / `getBounty` / `bountyTaskOf` / `resultOf` /
`nameOfId`); GitHub via `gh issue list --label colony`. It walks the bounty id
space directly (not just `openBounties`) so settled/paid history shows in the
totals. The public RPC 429s under bursts, so the reads are serialized with a
bounded retry/backoff — a slow run is normal, not a failure.

### Deploy

`web/colony.html` ships with the web bundle. Regenerate, then deploy the site:

```sh
node scripts/colony/build-board.mjs        # refresh the snapshot
./scripts/build-web.sh                      # (only if the wasm bundle also changed)
vercel deploy --prod --yes                  # serves it at localharness.xyz/colony.html
```

The page is a **point-in-time snapshot** (the generation timestamp is stamped in
the footer). It does **not** auto-refresh — re-run `build-board.mjs` and redeploy
to update it. A scheduled refresh (Vercel-Cron worker, or a `/loop`) is a named
follow-up, not built yet.

## Auth

- **GitHub**: currently the **maintainer's own `gh` login** (compusophy).
  Every script honors `GH_TOKEN` if set — when the `compusophy-bot` PAT
  lands, the swap is one env line (`GH_TOKEN=<bot pat>`), zero script changes.
  All gh calls pin `--repo compusophy/localharness` (`LH_REPO` to override)
  because the worktree carries an unrelated `upstream` remote.
- **On-chain**: writes go through the `localharness` CLI with local keys
  (`--as <name>`); gas is sponsored, rewards come from the poster's $LH.

## Not automated yet (honest scope)

- **PR authoring** — agents claim + submit, but the fix itself is human or a
  pluggable `$FIX_CMD` behind `scripts/issue-to-pr.sh`.
- **Merge** — always the maintainer, always behind a green `scripts/verify.sh`.
- **Issue closure bookkeeping** — `feedback-resolved.txt` lines are written by
  hand in the fixing commit.
- **gh 2.45 limitation** — no `closedByPullRequestsReferences`, so the merged-PR
  check scans the last 200 merged PRs for a `#<issue>` reference (closing
  keywords ranked first). Upgrade gh and this can become an exact linked-PR query.
- **Board refresh** — `web/colony.html` is a manually-regenerated snapshot. A
  scheduled refresh (Vercel-Cron worker or a `/loop` re-running `build-board.mjs`
  + redeploy) is a named follow-up, not built.
