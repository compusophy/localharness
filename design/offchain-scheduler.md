# design/offchain-scheduler — scheduling moves off-chain

> **STATUS: v1 SHIPPED (proxy + browser chat tools); CLI + agent-from-CLI staged.**
> The browser `schedule_task`/`cancel_task` chat tools now POST the proxy's
> off-chain store. The CLI (`schedule`/`goal`/`jobs`/`unschedule`) and any
> on-chain legacy jobs still use ScheduleFacet — the cron fires BOTH stores.

## Why

Scheduled jobs lived ON-CHAIN in `ScheduleFacet`. `scheduleJob` cost **~2.88M
sponsored gas** (cold SSTOREs + enumerable index pushes + the task bytes
@7.6k/byte) plus a refundable **escrow lock**, and `recordRun` per fire. For the
dominant case — *"notify me in 15 minutes"* — that was absurd: a one-shot push
cost two sponsored mainnet writes and locked `$LH` it could never even spend (a
0.1 `$LH` budget can't fund a 1 `$LH`/call run → the job was inert). The cron
heartbeat was never the cost; the on-chain **writes** were.

This is the third instance of the same lesson the platform already learned: apps
moved off-chain (setMetadata drained the sponsor → GitHub store), feedback moved
off-chain (telemetry repo). Scheduling follows.

## The model

Two orthogonal concerns were being conflated:
1. **The trigger** ("who notices a due job each minute") — the Vercel cron + the
   keeper heartbeat. Reads are free; this was never the cost.
2. **The store** ("where the ~6 job fields live between fires") — was on-chain
   (gas). Now a **GitHub repo** (`localharness-jobs`), via the bot token — the
   same free off-chain store apps/feedback use. No KV/Redis: a managed DB was
   considered and rejected in favor of the store we already run.

Job kinds:
- **`reminder`** — a future web-push of the task text. No model call, no `$LH`,
  zero chain. *This is the "notify me in 15 minutes" case → free.*
- **`agent`** — run the target agent each fire (bounded ping-pong), debit the
  **owner's existing meter** per call (same cost as an interactive message — no
  schedule tax). No escrow; billed as it runs.

## Pieces

- **`proxy/api/_jobstore.ts`** — the GitHub-backed store. One JSON file per job;
  the due time is encoded in the FILENAME (`<nextRun_pad>__<id>.json`) so the
  cron finds due jobs from a single directory LISTING (no per-job body read) and
  fetches only the bodies it fires. `advanceAfterFire` writes the new-nextRun
  file then deletes the old (or deletes on exhaust).
- **`proxy/api/schedule.ts`** — `POST /api/schedule` (`create`/`cancel`/`list`),
  personal-sign authed (`verifyAuthToken`). The authed address IS the job owner
  (billing + push identity); cancel/list only touch the caller's own jobs.
- **`proxy/api/scheduler.ts`** — the existing cron, extended: after the on-chain
  batch it calls `fireOffchainDue` (sharing the per-tick spend ledger). Reminder
  = `sendOwnerPush`; agent = `runPingPong` (child-jobs disabled via the new
  `allowChildJobs=false` gate — no on-chain parent escrow) + `meterDebit(owner)`.
  The on-chain scan failure now FALLS THROUGH (records `onchainError`) instead of
  502-ing, so off-chain reminders fire even on a chain RPC hiccup.
- **`registry::{create,cancel,list}_offchain_job`** — cross-target client (CLI
  `SystemTime`, browser `js_sys::Date::now()`), POST via
  `http_post_json_authed_returning`.
- **`src/app/chat/tools/misc.rs`** — `schedule_task`/`cancel_task` chat tools now
  POST the endpoint; `schedule_task` gained a `kind` (default `reminder`), dropped
  `budget`/escrow. `cancel_task`'s `job_id` is now the uuid string.

## Invariants / caveats (honest)

- **CAS via claim-by-delete.** Firing is gated by `claimJob(path, sha)` — a GitHub
  Contents-API DELETE conditioned on the file's read-time blob sha (an optimistic
  compare-and-swap). Of N overlapping cron ticks reading the same due file, only
  ONE delete matches the sha and wins; the rest skip without running/billing. The
  winner runs+bills, then `writeNextSlot` writes the next slot. So a job fires —
  and an agent job CHARGES — **at most once per due slot**, even if Vercel runs
  overlapping ticks. This replaces `recordRun`'s `StaleNextRun` guard. (The public
  `?poke` keeper heartbeat still stays ON-CHAIN-only; an off-chain keeper would
  ride the same claim CAS.)
- **Drift-corrected advance** (ported from `ScheduleFacet.recordRun`): a LATE fire
  jumps to the first grid slot after `now` — fires once, never bursts/burst-bills.
- **Lose-not-duplicate:** a crash between the claim-delete and the next-slot write
  drops ONE fire rather than risking a double-charge (the advance write retries
  once on a transient error first).
- **Per-owner job cap** (`MAX_JOBS_PER_OWNER`, default 50): free reminders can't
  flood the shared store and crowd the cron's due scan.
- **Reminders fire first**, exempt from the shared tick wall-clock budget, so the
  free "remind me in 15 minutes" case isn't starved by a slow agent/on-chain batch.
- **Commit-per-fire** for recurring jobs (one GitHub commit each run). Fine for
  reminders + low-frequency jobs; for high-frequency recurring load, swap
  `_jobstore.ts` for a KV adapter — the cron/endpoint don't change.
- A broke agent job (owner out of `$LH`) is **skipped but consumed** (advanced),
  never hot-looped — mirrors the on-chain dust-close.

## Provisioning (the one external dep)

The store needs a GitHub repo + push token:
- `GH_JOBS_REPO` (default `compusophy/localharness-jobs`) — **must be created**,
  bot given push (like `localharness-apps`).
- `GH_JOBS_TOKEN` — falls back to `GH_TELEMETRY_TOKEN`, so it works the moment the
  repo exists if that PAT can write it.

Then `cd proxy && vercel --prod` (separate deploy) + `build-web.sh` + web deploy
for the browser tool change.

## Staged (next increment)

- **CLI** `schedule`/`goal`/`jobs`/`unschedule` → the off-chain client (drop the
  on-chain escrow path + `registry::schedule_job_sponsored`). Held this turn to
  avoid a dead-code cascade through the on-chain helpers in one shot.
- **Off-chain keeper heartbeat** with dedup, so `?poke` can fire off-chain jobs
  safely (decentralized trigger for the off-chain store).
- Decommission ScheduleFacet once legacy on-chain jobs drain.
