# design/keeper — a decentralized scheduler keeper (P2P heartbeat)

> **STATUS: open.** The pure decision + roster cores and the live enumeration are
> SHIPPED and tested (`src/keeper.rs`, `registry::jobs_due`/`all_due_job_ids`, the
> `localharness keeper` dry-run CLI). What's NOT built — and needs a design
> decision before any contract change — is the **decentralized trigger**: how a
> permissionless keeper runs a due job and gets reimbursed without the trusted
> scheduler-role. This doc lays out that decision.

## Why

krafto, on-chain (#1.5): *"Scheduled jobs depend on a centralized Vercel cron
worker. A peer-to-peer keeper network to trigger due jobs would achieve true
autonomy."* Today `ScheduleFacet.recordRun` is **scheduler-role-only** (the
proxy meter key); one Vercel cron worker finds due jobs, runs each agent turn,
and commits the run. That worker is the last centralized dependency in the
otherwise-self-sovereign stack.

## What's already built (the safe, autonomous parts — done)

- **`src/keeper.rs` — pure decision core.** `is_fireable` mirrors the on-chain
  due-condition; `primary_keeper(job_id, epoch, count)` is a deterministic FNV-1a
  assignment so every peer agrees who owns a job (no thundering herd); `should_fire`
  gives the primary an immediate fire and backups a rank-staggered backoff
  (liveness if the primary is offline); `roster_position` derives a consistent
  `(my_index, keeper_count)` from a presence snapshot. 8 unit tests incl. the
  herd-free→liveness invariant and one-primary consensus.
- **`registry::jobs_due` / `all_due_job_ids`** — the cross-owner DUE-set
  enumeration (the on-chain `jobsDue` view the Vercel worker pages through),
  tuple-decoded + unit-tested.
- **`localharness keeper`** — a runnable DRY-RUN tick: reads the live due set,
  runs `keeper::jobs_to_fire`, prints what it would fire. Dogfooded on-chain.

A keeper can therefore already answer *"what is due, and would I fire it?"* The
machinery that's missing is *actually firing*.

## The hard part — the trust/economics of triggering

A run is two steps: (1) **run the agent turn off-chain** (a metered model call),
(2) **commit** on-chain (`recordRun`: advance `nextRun`, debit the owner's
escrowed budget by `COST_WEI`, CAS-guarded). Today the platform trusts the
single scheduler-role to do (1) honestly before (2), and the budget debit
reimburses the platform for the model cost.

Decentralizing means **anyone** can do (1)+(2) and claim the reimbursement — so a
malicious keeper could **commit without running** (advance `nextRun` + take a
run's budget while doing no work). On-chain can't verify an off-chain model call
happened. That is the whole problem; the pure cores deliberately don't touch it.

## Options (pick one — each implies a specific `ScheduleFacet` change)

**A. Semi-trusted permissionless keepers (simplest).** Add
`keeperRun(jobId, expectedNextRun, resultHash)` callable by any address in the
live keeper roster (or anyone). Keep all existing guards (not-due, CAS, budget
cap) — they already bound abuse to *at most one run's budget per due slot*, and
the job OWNER opted into the decentralized scheduler. A cheating keeper wastes
one run; reputation (`ReputationFacet`) + roster eviction deter repeat offenders.
*Pro:* shippable now, additive (a NEW selector via `diamondCut` ADD — does NOT
touch `recordRun`, so zero brick-risk to the live Vercel path). *Con:* a keeper
can advance-without-working; the budget owner bears bounded loss.

**B. Staked keepers + optimistic challenge.** A keeper stakes `$LH`, commits a
run + result; a challenge window lets anyone dispute (re-run + compare); a proven
bad commit slashes the stake to the challenger. *Pro:* trustless. *Con:* much
more contract + off-chain machinery (re-execution determinism, challenge UX) —
weeks, not a tick.

**C. Redundant keepers, platform stays authoritative.** Keepers only *nudge* —
they call a cheap `pokeJob(jobId)` that emits an event the platform (or any
keeper-of-record) acts on; commits stay role-gated. *Pro:* no trust change, real
liveness benefit (keepers ensure due jobs are noticed even if the cron stalls).
*Con:* not "true" decentralization — still a privileged committer.

## Recommendation

**Start with A**, scoped as an ADDITIVE `keeperRun` selector (zero risk to the
live scheduler), gated to the announced keeper roster (`SignalingFacet` presence
on `keccak256("localharness.keepers")`, read via the existing roster path → the
`roster_position` core). It ships the real P2P keeper with bounded, owner-opted
risk, and B can layer staking on top later. The remaining work once A is chosen:
the Solidity `keeperRun` + `Add*` script + cut (I do this with a rollback plan),
the presence wiring (announce/read roster → `RosterEntry[]`), and the trigger
wiring (run the turn, submit `keeperRun`).

This is a **trust-model decision for the maintainer**, which is why it's a doc and
not a forced implementation — picking the wrong model would be costlier to unwind
than to decide up front. Say which of A/B/C and I'll build it.
