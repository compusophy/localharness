# Concurrent Agent Worktree Collaboration (GitHub #18)

## Problem

Today's colony loop is strictly **sequential**: `sync-issues` files a GitHub issue, `issue-to-bounty` escrows $LH, ONE agent claims + authors a PR (via the pluggable `$FIX_CMD` behind `scripts/issue-to-pr.sh`), the maintainer merges behind a green `scripts/verify.sh`, and `settle-on-merge` pays the worker's TBA. Two things block parallelism:

1. **`issue-to-pr.sh` owns the primary working tree.** It requires a clean tree, `git switch --create fix/issue-<n>` off `origin/main`, runs the fixer + the full verify gate in-place, then restores the start branch via an EXIT trap. Two concurrent invocations would stomp each other's branch checkout, index, and dirty-tree guard. The harness is single-tenant by construction.
2. **No claim arbitration across agents.** Nothing stops two agents from independently picking the same issue and both authoring competing PRs (duplicated reward, wasted gas, merge thrash).

We want N agents working N distinct issues at once, each isolated, each provably gated, with conflicts resolved deterministically and the maintainer (or an auto-merger) integrating green PRs.

## Key insight: the lock already exists on-chain

`BountyFacet.claimBounty(bountyId, claimantTokenId)` reverts `NotOpen()` if `status != Open` and atomically flips `Open → Claimed` recording the claimant. That is a **distributed mutex with a single source of truth** — the chain. The first agent whose `claimBounty` tx lands wins the issue; every later claimant's tx reverts. We do NOT need to invent a lock table, a lease server, or a coordination daemon (per the "existing infra before new" rule — the chain IS the coordinator). Parallel safety reduces to: (a) require an on-chain claim before any worktree work, (b) give each claimed issue its own worktree + branch, (c) keep the verify gate as the per-PR merge guard.

This means the substrate stays Tempo + git + GitHub. No new server, no new facet (BountyFacet is sufficient as-is).

## Approach

Introduce a **worktree-isolated parallel runner** that wraps the existing pieces rather than replacing them:

- One git **worktree per claimed issue** under a sibling `.colony-worktrees/issue-<n>/`, each on its own `fix/issue-<n>` branch off fresh `origin/main`. Worktrees share the object store but have independent index + HEAD + working tree, so N fixers run with zero cross-talk. Keys already resolve cwd-first then `~/.localharness/keys` (confirmed in `verify-e2e.sh`), so a worktree just needs the agent's key reachable — config-home keys work from any checkout unchanged.
- A **claim-gated dispatcher** that, before spinning up a worktree, runs `localharness bounty claim --as <agent> <id>`; if it reverts `NotOpen`, the issue is already taken — skip it. The on-chain claim is the admission ticket.
- The **verify gate runs inside each worktree** (it already runs on the working tree). A PR opens only on green, exactly as today — unchanged guarantee, now per-worktree.
- **Conflict handling is deferred to merge time**, deterministically: PRs are independent branches off the same base. The maintainer/auto-merger integrates them one at a time; each merge re-bases the queue head and **re-runs verify** before the next merge. A PR that no longer applies cleanly or fails verify post-rebase is bounced back to its agent (issue reopened, bounty stays claimed or is reclaimed on expiry).

## On-chain / contract changes

**None required for the MVP.** `BountyFacet` already gives atomic claim, claimant binding, and expiry-based reclaim (`reclaimExpired`). The claim-as-lock model rides entirely on existing selectors (`claimBounty`, `bountyStatusOf`, `reclaimExpired`).

Optional Phase 3 enhancement (named, not built): a **heartbeat / lease** so a crashed agent's claim auto-frees faster than the 7d TTL. This would need a small `BountyFacet` addition (`renewClaim(id)` extending a short claim-lease, falling back to Open if not renewed). Deferred — the existing TTL-reclaim path is correct, just coarse.

## File-by-file plan

- **`scripts/colony/run-parallel.mjs`** (NEW) — the orchestrator. Reads open `colony`-labelled issues with live bounties (reuse `lib.mjs` `gh` + a new `bountyState` read mirroring `settle-on-merge.mjs`), filters to those whose bounty is `Open`, and for each up to a `--max <N>` concurrency cap: (1) `bounty claim --as <agent> <id>` — on `NotOpen` revert, skip (already taken); (2) create an isolated worktree; (3) run the fixer + verify gate (delegates to the refactored `issue-to-pr.sh`); (4) on green, submit the bounty result (PR url) and leave the PR open for merge. Dry-run by default, `--live` to claim/submit on-chain, matching every other colony script's convention.
- **`scripts/colony/lib.mjs`** (EDIT) — add reusable helpers: `bountyState(id)` (lift the parser out of `settle-on-merge.mjs` so both share it), `claimBountyLive(agent, id)` returning `{claimed:bool, reason}` (distinguishes a real failure from the benign `NotOpen` lost-race), and `openColonyBounties()` (join `gh issue list --label colony` to on-chain bounty status). Keep zero-dep, `execFileSync` arg-arrays, Windows-safe.
- **`scripts/issue-to-pr.sh`** (EDIT) — add a `--worktree <dir>` mode. Today it mutates the primary tree (clean-tree guard, `git switch --create`, EXIT-trap restore). In worktree mode it instead `git worktree add <dir> -b fix/issue-<n> origin/<base>`, runs the fixer + verify gate **with `cwd=<dir>`**, opens the PR from there, and on cleanup `git worktree remove`s it (or leaves it on failure for inspection, mirroring `--keep-branch`). The single-tree path stays the default so nothing existing breaks. This is the load-bearing change: it makes the harness reentrant.
- **`scripts/colony/RUNBOOK.md`** (EDIT) — add a "Parallel mode" section: how claims arbitrate (first tx wins), worktree layout, the per-merge re-verify rule, and how to clean up stale worktrees (`git worktree prune`). Update the pipeline diagram to show the fan-out.
- **`scripts/colony/settle-on-merge.mjs`** (EDIT, light) — no behavior change, but import the shared `bountyState` from `lib.mjs` instead of its private copy (kills the duplication the parallel path would otherwise fork).
- **`scripts/colony/merge-queue.mjs`** (NEW, Phase 2) — the integration guard. Lists open colony PRs, and for each in turn: rebase onto latest `origin/main` in a throwaway worktree, run `scripts/verify.sh`, and only on green print the `gh pr merge --squash` command (dry-run default; `--live` merges + triggers `settle-on-merge`). Serializes merges so the verify gate is always evaluated against the post-merge tree — this is how concurrent PRs are made safe to land without a human eyeballing every conflict.
- **`.gitignore`** (EDIT) — ignore `.colony-worktrees/`.
- **`scripts/colony/build-board.mjs`** (EDIT, Phase 2) — surface per-issue claimant + worktree/PR state so the public board shows the parallel fan-out, not just a linear list.

## Risks

- **Lost-race wasted work.** Two agents could both *decide* to work issue #N before either's `claimBounty` lands; the loser wastes a worktree spin-up. Mitigation: claim is the FIRST action and is cheap (sponsored gas); the loser detects `NotOpen` and aborts before the (expensive) fixer/verify. Acceptable — the only waste is a git worktree add, not a verify run.
- **Stale claims from crashed agents.** A claimed-but-abandoned issue is locked until the 7d TTL `reclaimExpired`. Mitigation MVP: a `--reclaim-stale` sweep in `run-parallel.mjs` that reclaims expired claims before dispatch. Phase 3: the optional lease.
- **Merge conflicts between green PRs.** Two PRs both green in isolation can conflict on merge. Mitigation: `merge-queue.mjs` serializes + re-verifies after each merge; a conflicting PR is bounced (comment + reopen issue), never force-merged. Never auto-resolve conflicts.
- **Worktree leakage / Windows file locks.** Worktrees left behind brick the next run (this repo is on Windows; worktree cleanup has bitten before per the "long-loop diminishing returns" memory). Mitigation: idempotent `git worktree prune` + `git worktree remove --force` on startup, and a unique dir per issue so a stale one never blocks a new claim.
- **Verify gate cost × N.** Running the full 10-stage `verify.sh` (cargo test × 4 feature configs + 3 wasm checks + cartridge build) per worktree is heavy. Mitigation: cap concurrency (`--max`, default 2–3); the gate cost is the price of trustworthy autonomous PRs and is non-negotiable (it's the immune system).
- **Two agents, one identity.** If agents share a key, claimant binding is meaningless and payouts collide. Mitigation: each parallel worker MUST use a distinct `--as <agent>` (distinct on-chain identity/TBA); the dispatcher refuses to run two slots under the same name.

## Phased build order

**Phase 1 — Reentrant harness (the unlock).** Add `--worktree` mode to `issue-to-pr.sh`; add `bountyState`/`claimBountyLive`/`openColonyBounties` to `lib.mjs`; `.gitignore` the worktree dir. Prove: two manual `issue-to-pr.sh --worktree` runs on different issues complete concurrently without collision, both gate-green. No orchestrator yet.

**Phase 2 — Parallel dispatcher + merge queue.** Build `run-parallel.mjs` (claim-gated fan-out, dry-run default) and `merge-queue.mjs` (serialized rebase + re-verify + settle). Refactor `settle-on-merge.mjs` to the shared helper. Prove E2E with TWO of my own fleet identities claiming two distinct open colony bounties, both landing as green PRs, the merge queue integrating both, and both TBAs settled.

**Phase 3 — Robustness + visibility.** Stale-claim reclaim sweep; board fan-out columns; optional `renewClaim` lease facet addition (only if abandoned-claim latency proves painful). Wire the dispatcher into the idle-loop fleet so quiet ticks generate real concurrent colony throughput.

Phase 1 alone delivers the core ask (safe concurrent work on the same repo); Phases 2–3 make it autonomous and observable.