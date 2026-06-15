# design/keeper — a decentralized scheduler keeper (P2P heartbeat)

> **STATUS: option C SHIPPED; B open.** The decentralized HEARTBEAT (option C) is
> live: the pure decision + roster cores (`src/keeper.rs`), the cross-owner due
> enumeration (`registry::jobs_due`/`all_due_job_ids`), the proxy's public
> `?poke=<jobId>` trigger (`proxy/api/scheduler.ts`, deployed), and the
> `localharness keeper` CLI that reads the due set and pokes the proxy to run each.
> Any keeper (CLI / browser tab) is now a scheduler heartbeat, so jobs fire even if
> the single Vercel cron stalls — trust-free (run+commit stay in the proxy
> executor; a poke only ever runs a genuinely-due job once). What remains OPEN is
> **B: trustless EXECUTION** (stake + challenge/slash so the executor itself is
> decentralized, not just the heartbeat). The trust analysis below is why A was
> rejected and C shipped first.

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

**A. Semi-trusted permissionless keepers (simplest — but griefable, see con).** Add
`keeperRun(jobId, expectedNextRun, resultHash)` callable by any address in the
live keeper roster (or anyone). Keep all existing guards (not-due, CAS, budget
cap). *Pro:* shippable now, additive (a NEW selector via `diamondCut` ADD — does
NOT touch `recordRun`, so zero brick-risk to the live Vercel path).
*Con (CORRECTED — worse than first written):* a keeper can advance-WITHOUT-working
and claim the reimbursement. The not-due/CAS guards only bound it to one commit
*per due slot* — but a malicious keeper does that EVERY slot, draining the job's
WHOLE budget over time for zero work, and roster eviction is weak (a sybil
re-announces). So pure-A is griefable; it really needs a keeper **bond slashed on
a bad commit** — which drags in B's challenge machinery. Pure-A is NOT a clean
ship.

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

## Recommendation (revised)

krafto's actual complaint is the centralized **cron heartbeat** — "is anyone
watching for due jobs?" — NOT that the proxy executes the turn (the credit proxy
is the project's one accepted off-chain component). That reframes the choice:

- **Pure A is OUT** — griefable (whole-budget sybil-drain, see its con). Making it
  safe requires a bond + challenge, i.e. it collapses into **B** (trustless but
  weeks of work: re-execution determinism + challenge/slash UX).
- **C is the clean minimal win** and directly fixes the complaint: keepers become
  the decentralized HEARTBEAT (a P2P network that notices due jobs and pokes them)
  while run+commit stay with the trusted proxy executor. No trust problem, no
  griefing — a keeper poke is only acted on if the job is genuinely due. It removes
  the single-cron dependency (many keepers vs one Vercel tick) without pretending
  to remove the proxy. The decision cores already built decide *which* due jobs to
  poke; the work is a proxy `pokeJob` endpoint + keepers calling it.

**So: recommend C now** (decentralize the heartbeat, trust-free), and treat **B**
as the later "trustless execution" upgrade. This is still a **maintainer call** —
C accepts the proxy as executor; B is the heavier path to true autonomy — which is
why it's a doc, not a forced implementation. Say **C** (I'll build the heartbeat:
proxy `pokeJob` + keeper wiring over the existing cores) or **B**, and I'll start.
