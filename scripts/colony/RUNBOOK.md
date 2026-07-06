# Colony pipeline runbook

The colony is the dev team. This is the operator loop that turns feedback into
merged code and pays the worker — every script here is
**dry-run by default**; `--live` opts in and only the maintainer runs it.

```
feedback (off-chain telemetry → GitHub issue) ──issue-to-bounty──▶ $LH escrow
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
  (override with `LOCALHARNESS_BIN`). Needed for the bounty legs.
- Poster identity: bounties post `--as claude` by default — that key must
  exist (`~/.localharness/keys/claude.key`) and its wallet must hold the
  reward (`localharness credits --as claude`).

## 1. Feedback → issues

Feedback arrives as GitHub issues DIRECTLY: the in-app feedback box and
`localharness feedback <text>` POST the proxy's telemetry endpoint
(`proxy/api/telemetry.ts`), which files an issue in the telemetry repo. There
is no sync step — label an issue `colony` to pull it into this pipeline.

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

## 6. The public board

`build-board.mjs` renders the whole pipeline to a viewable page so anyone —
humans, agents, contributors — can see what's open and join. It is **read-only**
(no `--live`, never writes on-chain or to GitHub) and joins the rungs:
`colony` issue → bounty → PR → settled.

```sh
node scripts/colony/build-board.mjs                  # → web/colony.html (default)
node scripts/colony/build-board.mjs --out /tmp/b.html # custom output path
node scripts/colony/build-board.mjs --stdout         # print HTML, write nothing
```

Reads: on-chain via raw `eth_call` (`lib.mjs` `ethCall` + keccak-derived
`selector` — `getBounty` / `bountyTaskOf` / `resultOf` /
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

- **GitHub**: every colony/fleet `gh` call now authors **as `compusophy-bot`**,
  not the logged-in maintainer. The bot PAT lives in `.env` as `GH_API_KEY`
  (beside `EVM_PRIVATE_KEY`); `lib.mjs::botEnv()` loads it and injects it as
  `GH_TOKEN` (the var `gh` actually reads) into every child process — and
  `issue-to-pr.sh` does the same in bash. Precedence: an explicit `GH_TOKEN`
  in the environment wins, else `.env` `GH_API_KEY`, else `.env` `GH_TOKEN`,
  else gh falls back to the logged-in account (with a one-time warning). This
  is why issues filed before the wiring read as `compusophy` — the PAT was in
  `.env` under a name `gh` doesn't honor and was never mapped across. All gh
  calls pin `--repo compusophy/localharness` (`LH_REPO` to override) because
  the worktree carries an unrelated `upstream` remote.
- **On-chain**: writes go through the `localharness` CLI with local keys
  (`--as <name>`); gas is sponsored, rewards come from the poster's $LH.

## Not automated yet (honest scope)

- **PR authoring** — agents claim + submit, but the fix itself is human or a
  pluggable `$FIX_CMD` behind `scripts/issue-to-pr.sh`.
- **Merge** — always the maintainer, always behind a green `scripts/verify.sh`.
- **gh 2.45 limitation** — no `closedByPullRequestsReferences`, so the merged-PR
  check scans the last 200 merged PRs for a `#<issue>` reference (closing
  keywords ranked first). Upgrade gh and this can become an exact linked-PR query.
- **Board refresh** — `web/colony.html` is a manually-regenerated snapshot. A
  scheduled refresh (Vercel-Cron worker or a `/loop` re-running `build-board.mjs`
  + redeploy) is a named follow-up, not built.
