# localharness — Agent scheduling (`/loop` + `/schedule`, baked in)

> **Status: DESIGN ONLY — no code.** This is the triage/plan the user asked for
> *before* any implementation. It specifies a new on-chain `ScheduleFacet` (the
> durable, tab-independent job registry), the **Vercel-Cron-reads-the-chain**
> worker that fires due jobs through the *existing* headless `call` path, the
> agent `schedule_task` tool + `localharness schedule` CLI, the recursion /
> "agent ping-pong" safety model, and a phased plan with the genuine open
> questions called out. Read alongside [`autonomous-loop.md`](autonomous-loop.md)
> (the trigger-driven QA fleet — which assumes a *running process*, the gap this
> doc closes), [`invites.md`](invites.md) (the escrow/facet conventions this
> mirrors), and [`economy-reputation.md`](economy-reputation.md) (the `$LH`
> spend seams).

## 0. The ask, stated precisely

Verbatim user intent:

> "I LOVE this `/loop` and `/schedule` feature… WOULD LOVE TO BAKE THAT IN…
> THEN THE AGENTS COULD BE PERSISTENT ON A JOB RECURSIVELY WITHOUT ME HAVING TO
> KEEP THE TAB OPEN!!" + "agent ping-pong… if we need to figure out cron jobs."

The model is the harness's own two primitives:

- **`/loop`** — run a prompt (or slash command) on a recurring interval; the
  same task fires again and again until cancelled.
- **`/schedule`** — a *durable, cloud-side* cron: the routine runs on a schedule
  **whether or not the originating client is open**.

Decomposed into hard requirements:

1. An agent (or a user, via the agent or CLI) can **schedule a task** to run a
   `<name>.localharness.xyz` agent on a **recurring schedule** (interval or cron).
2. The scheduled job **survives the browser tab closing** and **survives any
   single process dying** — "persistent without me keeping the tab open" is the
   whole point.
3. Jobs can be **recursive** — a job that runs agent A can schedule or call agent
   B, and B can call back to A ("agent ping-pong").
4. The whole thing must spend **bounded `$LH`** — a recursive scheduler that can
   spawn work and spend money MUST have a hard stop.
5. It must fit the substrate: **Tempo + the user's browser + the ONE accepted
   server (the credit proxy)**. No new per-user server, no database, no daemon.

---

## 1. The crux: where does a "persistent" agent actually run?

This is the load-bearing constraint and the reason scheduling is not just
"`every()` on a timer in the browser."

A localharness agent normally runs **in the browser tab** (wasm). Close the tab
and the wasm context is gone — `triggers.rs::every()`, `spawn_local`, the whole
agent loop, all dead. There is **no per-user server** to move the loop to; the
project's substrate is deliberately Tempo + the user's browser
(`feedback_no_offchain_infra`). So "persistent without a tab" is a contradiction
*until you separate three things that are usually fused*:

| Concern | Usually lives… | For a tab-independent job, must live… |
|---|---|---|
| **The job definition + its budget** (what to run, how often, how much `$LH`) | in tab memory / a `Trigger` struct | **on-chain** — the only durable, serverless store that survives any tab/process death |
| **The schedule clock** (who notices a job is due, and when) | the tab's interval timer | **a single shared cron** — the proxy is the one server that's always up |
| **The execution** (running the agent turn) | the tab's wasm agent loop | a **server-free PROCESS** — which already exists: the headless `call` path |

The third row is the unlock. The headless `call` primitive
(`src/bin/localharness.rs::run_agent_turn`, shipped) **already runs a full agent
turn with no browser tab**: it builds a `GeminiAgentConfig`/`AnthropicAgentConfig`
pointed at the credit proxy (`with_base_url(proxy)`), authenticates with an
identity key (Ethereum personal-sign token), runs the turn under the **target's
on-chain persona** (`registry::persona_of`), pays **per-request `$LH`** through
the meter, and returns the reply. No tab, no relay, no per-user server. **This is
the execution primitive for a scheduled job.**

What's missing is (a) a durable place to keep the job + its budget, and (b)
something that periodically notices a job is due and *fires that headless turn*.
(a) is a new facet. (b) is a Vercel Cron on the proxy. The rest of this doc is
those two pieces and their safety envelope.

> **Why not just keep the tab's `every()` loop?** Because the user's literal ask
> is "without me having to keep the tab open." A browser `every()` loop is a fine
> *bonus* path (§2.4 — the tab fires due jobs when it happens to be open, saving
> the cron a tick) but it cannot be the *primary* mechanism: the job dies with
> the tab. The durable answer must live off the tab entirely.

---

## 2. The worker — who executes due jobs

### 2.1 Decision: a Vercel Cron on the proxy reads the on-chain registry and fires due jobs through the headless `call` path

The credit proxy
(`https://proxy-tau-ten-15.vercel.app`, the ONE accepted off-chain component)
is a Vercel project, and **Vercel supports Cron Jobs** — scheduled invocations of
a serverless function on a crontab. That is the natural, already-paid-for "who
fires the scheduled jobs" answer. The flow:

```
            ┌──────────────────────── on-chain (Tempo diamond) ───────────────┐
            │  ScheduleFacet: Job{ id, owner, target, task, schedule,         │
            │                      next_run, budget_lh, runs_left, enabled }  │
            └───────────────────────────────▲─────────────────┬──────────────┘
                                   jobsDue(now) │   recordRun(id,next) │
                                   (eth_call)   │   (signed tx)        │
   Vercel Cron (crontab in vercel.json)         │                      │
   ───── every N minutes ─────▶  /api/scheduler ─┘                      │
                                      │                                 │
                                      │ for each due job:               │
                                      │   • check budget ≥ cost         │
                                      │   • run the agent turn  ────────┼──▶ credit proxy
                                      │     (headless `call` path:      │     (/v1beta or /v1)
                                      │      target persona, model,     │     debits job budget
                                      │      task prompt)               │     in $LH per request
                                      │   • persist transcript (§5)     │
                                      └── recordRun(id, nextFire) ──────┘
```

Concretely, `/api/scheduler` (a new Edge/Node function in `proxy/api/`, fired by
a `crons` entry in `proxy/vercel.json`) does, each tick:

1. **Read the due set** — `eth_call` `jobsDue(now)` on the diamond (the same
   `fetch(TEMPO_RPC, eth_call)` pattern `gemini.ts::sessionExpiryOf` already
   uses). Returns the job ids whose `next_run <= now && enabled && runs_left > 0
   && budget_lh >= perRunCost`.
2. **For each due job**, run the agent turn. The proxy already *is* the model
   bridge — but the scheduler is a *server-side caller of itself*, so the cleanest
   shape is to factor the headless turn so both the CLI and the scheduler reach
   it identically (see §2.3). The turn:
   - loads the target's persona (`persona_of(target)`),
   - sends the job's `task` prompt,
   - runs under the **job owner's identity** for billing (the per-request meter
     debits the *job's on-chain `$LH` budget* — §5.1),
   - captures the reply.
3. **Debit + record** — the per-request `$LH` is debited from the job's budget
   on-chain (§5.1), then `recordRun(id, nextFire)` advances `next_run` and
   decrements `runs_left`/`budget_lh`. `recordRun` is the *only* state write the
   worker makes to a job, and it's idempotency-guarded (§2.5).

### 2.2 Why this model beats the alternatives (named honestly)

| Worker model | How it'd fire jobs | Why it loses here |
|---|---|---|
| **Vercel Cron reads on-chain registry** *(chosen)* | the one existing server ticks a crontab, reads `jobsDue`, runs the headless turn | **Zero new infra** (the proxy + Vercel cron are already there), survives every tab/process death (job lives on-chain), and the trust envelope is *the same one the proxy already has* (§2.6). The honest fit. |
| **Browser tab `every()` loop** | the open tab fires due jobs | Dies with the tab — fails the literal ask. Kept as an *opportunistic* path only (§2.4). |
| **OS cron running `localharness`** | the user's machine runs `localharness schedule-tick` on a system crontab | Real and zero-trust-to-us, but it's a **per-user server/daemon** — exactly what the substrate forbids, and it dies when the laptop sleeps. Offered as a *self-hosted* opt-in (§2.7), never the default. |
| **Permissionless keeper network** | anyone runs a keeper that earns a bounty for firing due jobs | The *correct* long-arc decentralized answer (Chainlink-Automation-shaped). But it needs a bounty/staking economy and multiple independent keepers to be live — heavy, and premature while there's exactly one operator. **The on-chain registry is deliberately keeper-agnostic** (§2.8) so this is a drop-in upgrade later, not a rewrite. |

The decision is **Vercel-Cron-reads-the-chain for the worker, on-chain registry
for durability** — because it's the only model that is *both* tab-independent
*and* adds no new server beyond the one already accepted, *and* leaves the door
open to a keeper network without a redesign.

> **Why the registry must be on-chain and not "in the proxy's database."** The
> proxy has no database — it's a stateless Edge function (`gemini.ts` holds only
> env keys). Putting jobs in a proxy KV store would (a) introduce the database
> the project rejects, and (b) make the job + its budget **trust the operator's
> server** for safekeeping. On-chain, the job and its escrowed `$LH` budget
> survive the proxy itself going down, can't be silently edited by the operator,
> and are auditable. The worker is *stateless and replaceable*; the durable
> state is the chain. That's the whole "survives a tab/process dying" property.

### 2.3 Factor the headless turn so the cron and the CLI share it

`run_agent_turn` (`src/bin/localharness.rs`) is the existing tab-free turn, but
it's (a) Rust, in the CLI binary, and (b) the proxy is TypeScript. Two clean
options, decision deferred to §7:

- **(A) The cron calls the agent the same way a CLI would** — i.e. the scheduler
  function builds the same proxy request the Rust client builds (system =
  target persona, the task as the user message) and POSTs it to the proxy's own
  `/v1beta`/`/v1` handler (an internal `fetch` to itself, or a direct call to the
  shared handler). This keeps **all model-routing/billing logic in one place**
  (the existing `gemini.ts` handler) and the scheduler is a thin "build request +
  loop the tool turns" driver. *Lean here* — it reuses the exact metering path.
- **(B) A tiny native `localharness schedule-tick` the cron shells out to** — the
  cron triggers a native run of the *Rust* `run_agent_turn`. Cleaner code reuse
  (literally the same function the CLI uses) but Vercel cron functions don't run
  arbitrary native Rust binaries; you'd need a separate always-on host. That
  reintroduces a server. **Rejected** for the default; it's really the §2.7
  self-hosted path.

The recommendation is **(A): the scheduler is a proxy function that reuses the
proxy's own model-bridge handler**, so there is exactly one billing/metering code
path and the cron adds no new model-access surface — just the loop "read due →
build the turn → record."

### 2.4 The tab is an *opportunistic* second firer (not required)

When a user *does* have a tab open, the browser can fire its own due jobs to make
the loop snappier than the cron cadence: `triggers.rs::every()` (already used in
`autonomous-loop.md`) ticks, reads `jobsDue(now)` for jobs *it owns*, and runs
them through the in-tab agent loop, then `recordRun`. Because `recordRun` is
idempotency-guarded on `next_run` (§2.5), the tab and the cron racing on the same
job is harmless — whoever lands `recordRun` first advances `next_run`; the loser's
`recordRun` reverts (its `expectedNextRun` no longer matches) and it simply
doesn't double-bill. So the tab is a *latency optimization*, never a correctness
dependency. This is the bridge between the user's familiar `/loop` (tab open) and
`/schedule` (cloud, tab closed): **same on-chain job, two firers, the cron is the
floor.**

### 2.5 Idempotency + missed fires

The two failure modes a cron-driven scheduler must handle:

- **Double-fire** (two cron invocations overlap, or the tab + cron both fire):
  guarded by **compare-and-swap on `next_run`**. `recordRun(id, expectedNextRun,
  newNextRun)` reverts unless the job's stored `next_run == expectedNextRun`. The
  worker reads `next_run`, runs the turn, then `recordRun` with that read value;
  whoever commits first wins, the other reverts and **does not debit again**. The
  `$LH` debit (the meter call) and the `recordRun` should be ordered so a job
  can't be debited without `next_run` advancing — debit *then* record, and on a
  `recordRun` revert, the run is treated as wasted (the §2.6 trust model bounds
  the loss to the job's own budget). Cleaner still: fold the debit *into*
  `recordRun` (the facet pulls the per-run `$LH` from the job's on-chain budget
  atomically with advancing `next_run`), so a single tx is the commit point and
  double-fire is structurally impossible (§5.1 prefers this).
- **Missed fire** (the cron was down, or `next_run` is far in the past because a
  job was due while no firer ran): **do NOT replay every missed interval**
  (a job idle for a day shouldn't suddenly fire 288 times and drain its budget).
  `recordRun` computes the *next* fire from `now`, not from the old `next_run +
  interval` — i.e. **fire once, then schedule forward from now** ("skip, don't
  pile up"). A `missedFires` counter can be incremented for observability, but
  the default is **at-most-once-per-tick, catch up to one run**. This is the safe
  default; an opt-in "strict cadence" mode is a §7 open question.

### 2.6 Trust — what the worker can and cannot do

The worker (the proxy's scheduler function + its meter key) gains **no new
authority beyond what the proxy already holds**. Its envelope:

- **It can only run *scheduled* jobs.** The worker reads `jobsDue` and runs those
  exact `(target, task)` pairs the *owner* put on-chain. It cannot invent a task,
  retarget a job, or run an unscheduled agent. The job's definition is
  owner-signed on-chain; the worker is a dumb executor of it.
- **It can only debit a job's *own* budget.** Billing goes through the existing
  `CreditMeterFacet.meter` path (the proxy's `PROXY_METER_KEY`, already the
  diamond's meter key — `gemini.ts::meterDebit`). The scheduler debits the
  **job's** budget, which is capped on-chain (§5.1). It **cannot** drain the
  owner's general `$LH` wallet — only the `$LH` the owner committed to that job.
  This is the same "the proxy can already debit metered balances" power, narrowed
  to per-job budgets.
- **It cannot steal.** The worker never holds the owner's identity key. It never
  signs *as* the owner for anything except the meter debit (which it already can
  do as the meter key) and `recordRun` (which only advances a clock + decrements
  a budget — see §5.1 on whether `recordRun` is owner-signed or meter-key-signed;
  recommendation: a dedicated **scheduler role** the diamond owner grants the
  proxy's key, authorized for `recordRun` + the per-job debit *only*). Same blast
  radius as the proxy holding the model key: if the proxy key leaks, an attacker
  can fire *already-scheduled* jobs and burn *their committed budgets* — annoying,
  bounded, non-catastrophic, and exactly the risk profile already accepted for the
  meter key.
- **Gas is sponsored.** Every `recordRun`/debit tx is paid by the embedded
  AlphaUSD sponsor (`SPONSOR_KEY`), same as every other user-facing write. The
  owner and the worker hold **zero gas**.

The honest one-liner: **the scheduler is the proxy with a crontab — it gains the
power to fire owner-defined jobs and spend their pre-committed budgets, nothing
more.** It's the same trust boundary the system already lives inside.

### 2.7 The self-hosted escape hatch (offered, not default)

For a user who wants *zero* trust in the operator's cron, the same on-chain
registry supports an **OS-cron path**: `localharness schedule-tick` (a new
subcommand) reads `jobsDue` for jobs *it owns*, runs `run_agent_turn` natively,
and `recordRun`s — driven by the user's own system crontab. This is a per-user
server (forbidden as a *default*), but as an **opt-in** it's a legitimate
"run your own keeper" mode that needs no platform change because the registry is
keeper-agnostic (§2.8). Named so it's a known door, not the recommended path.

### 2.8 The registry is keeper-agnostic by design

`jobsDue(now)` / `recordRun(...)` make **no assumption about who calls them**.
Today exactly one keeper exists (the proxy cron). Tomorrow a **permissionless
keeper network** (anyone runs a keeper, earns a small `$LH` bounty per fired job,
skin-in-the-game via stake) is a drop-in: add a `keeperBounty` to the job and pay
the caller of `recordRun` out of the budget. The MVP deliberately ships the
single-keeper proxy cron *with the storage shaped so the bounty/stake fields can
be appended later* (append-only struct rule, §3.1). This is the same "leave the
seam, don't build the heavy version yet" discipline as `invites.md`'s owner-knobs.

---

## 3. On-chain design — a new `ScheduleFacet`

### 3.1 New facet + `LibScheduleStorage`

**Decision: new `ScheduleFacet` + `LibScheduleStorage`** at
`keccak256("localharness.schedule.storage.v1")` — the diamond convention (each
facet's storage in its own lib at its own slot; CLAUDE.md on-chain stack). It
collides with nothing cut. Cut via a standard `script/AddScheduleFacet.s.sol`
(template `AddSignalingFacet.s.sol` — `new ScheduleFacet()` → `diamondCut(Add,
selectors)`).

It is **not** an extension of any existing facet: it owns a durable job table
with a budget escrow + a state machine, distinct from SessionFacet (time-boxed
proxy access) and CreditMeterFacet (a flat per-address balance). Welding it onto
either would entangle two value semantics, the exact lesson `invites.md` §1.1
draws for InviteFacet.

### 3.2 Storage layout

Slot `keccak256("localharness.schedule.storage.v1")`. One record per job, keyed
by a monotonic `uint256 id` (`nextJobId++`, like `LocalharnessRegistryFacet`'s
`nextId`):

```text
struct Job {
    address owner;        // who scheduled it; the budget refund recipient; the billing identity
    uint256 targetId;     // tokenId of the agent to run (name resolved off-chain via nameOfId)
    uint64  interval;     // seconds between runs (the simple cadence; cron string is a pointer, §3.4)
    uint64  nextRun;      // unix seconds of the next due fire (the CAS key for recordRun)
    uint64  created;      // unix seconds, for expiry/audit
    uint64  expiry;       // unix seconds; 0 = none (bounded by MAX_TTL anyway)
    uint128 budgetWei;    // $LH escrowed for this job; debited per run; refundable on cancel
    uint32  runsLeft;     // max remaining runs (the hard count cap); 0 disables
    uint32  runsCount;    // runs executed (audit / recursion accounting)
    uint8   depth;        // recursion depth of the chain that created this job (§4)
    Status  status;       // Active | Paused | Done | Cancelled  (uint8 enum)
    // task: stored separately (see below) — strings don't pack
}
// mapping(uint256 => Job)      jobs;            // id -> record
// mapping(uint256 => bytes)    task;            // id -> the prompt (or an off-chain pointer, §3.4)
// mapping(address => uint256)  budgetEscrowedOf;// owner -> total $LH locked across their jobs (per-owner cap input, §4)
// mapping(address => uint32)   activeJobsOf;    // owner -> count of Active jobs (rate-limit input, §4)
// uint256 nextJobId;
```

The scalar fields pack into a small number of slots; `owner(160)+interval(64)`
fills slot 0's spare with `+ status`/`depth` etc. **The `task` prompt lives in
its own `mapping(uint256=>bytes)`** because storing strings on-chain is the
gas-hungry path CLAUDE.md warns about (`setMetadata`/`submitFeedback` ~7.6k
gas/byte) — so the *task* should usually be a **pointer**, not inline prose (§3.4).

> **Append-only rule** (mirrors `LibRedeemStorage`): new fields (e.g. a future
> `keeperBounty`, `lastRunTxOrHash`, `missedFires`) go at the **end** of the
> struct, never reordered — diamond storage is positional.

### 3.3 Functions + events

```text
// --- schedule (permissionless to create; owner escrows the budget) --------
scheduleJob(uint256 targetId, bytes task, uint64 interval, uint32 maxRuns,
            uint128 budgetWei, uint64 ttlSeconds, uint8 depth) -> uint256 id
    -> require interval in [MIN_INTERVAL, MAX_INTERVAL]   // no 1-second hammer
    -> require budgetWei >= MIN_BUDGET && maxRuns > 0 && maxRuns <= MAX_RUNS
    -> require ttl in [0, MAX_TTL]; depth <= MAX_DEPTH (§4)
    -> transferFrom(msg.sender, diamond, budgetWei)       // escrow the $LH budget
    -> store Job{owner: msg.sender, ..., nextRun: now+interval, status: Active}
    -> budgetEscrowedOf[owner] += budgetWei; activeJobsOf[owner]++
    -> emit JobScheduled(id, owner, targetId, interval, budgetWei, nextRun)

// --- run accounting (the WORKER's only write; scheduler-role-gated) --------
recordRun(uint256 id, uint64 expectedNextRun, uint128 spentWei) -> uint64 newNextRun
    -> require msg.sender is the scheduler role (or a future bountied keeper)
    -> load job; require status==Active && now >= nextRun
    -> require nextRun == expectedNextRun                  // CAS: defeats double-fire (§2.5)
    -> require spentWei <= budgetWei && spentWei <= MAX_SPEND_PER_RUN
    -> budgetWei -= spentWei; budgetEscrowedOf[owner] -= spentWei   // debit the JOB budget
    ->   (the spentWei moves diamond->payee per the billing model, §5.1)
    -> runsCount++; runsLeft--
    -> newNextRun = computeNext(now, interval)            // SKIP, don't pile up (§2.5)
    -> if runsLeft == 0 || budgetWei < minRunCost || (expiry!=0 && now>expiry):
           status = Done; refund remaining budgetWei to owner; activeJobsOf[owner]--
       else: nextRun = newNextRun
    -> emit JobRan(id, runsCount, spentWei, newNextRun, status)

// --- owner controls -------------------------------------------------------
cancelJob(uint256 id)                                     // owner-only
    -> require msg.sender == job.owner; status==Active||Paused
    -> status = Cancelled; refund remaining budgetWei to owner; activeJobsOf--
    -> emit JobCancelled(id, refundedWei)
pauseJob(uint256 id) / resumeJob(uint256 id)              // owner-only; status flip, no refund
topUpJob(uint256 id, uint128 addWei)                      // owner-only; escrow more $LH
    -> transferFrom(owner, diamond, addWei); budgetWei += addWei

// --- views (the worker + UIs read these) ----------------------------------
jobsDue(uint64 now, uint256 cursor, uint256 limit) -> uint256[] ids
    -> paginated scan of Active jobs with nextRun<=now && budget/runs ok
jobOf(uint256 id) -> Job
taskOf(uint256 id) -> bytes
jobsOfOwner(address owner) -> uint256[] ids               // for the "my jobs" UI
nextJobId() -> uint256
```

**Events** (indexed for off-chain harvest, same discipline as
`FeedbackSubmitted`/`Redeemed`): `JobScheduled(uint256 indexed id, address
indexed owner, uint256 indexed targetId, uint64 interval, uint128 budgetWei,
uint64 nextRun)`, `JobRan(uint256 indexed id, uint32 runsCount, uint128 spentWei,
uint64 nextRun, uint8 status)`, `JobCancelled(uint256 indexed id, uint128
refundedWei)`. `JobRan` is the durable audit trail of every scheduled execution —
queryable like the feedback log.

> **`jobsDue` is paginated, not "return all due."** The diamond has no cheap
> "iterate the mapping" — `jobsDue` scans an enumerable index of Active jobs (an
> `id[]` maintained on schedule/cancel, the same enumerable-index pattern
> `DeviceRegistryFacet`/`OwnedTokensFacet` use to avoid log-scraping). The worker
> pages through with `(cursor, limit)`. If the active-job count grows large, the
> index can be **bucketed by `nextRun` window** (a future optimization) so the
> worker reads only the current bucket. MVP: a flat enumerable index + pagination.

### 3.4 The `task`: inline prompt vs. pointer

On-chain string storage is the expensive path (CLAUDE.md gas gotcha). Three
shapes for `task`, by cost:

1. **Short inline prompt (MVP-fine for small tasks).** The `bytes task` is the
   literal prompt ("Check the deploy and report if it's down"). Gas scales with
   length (~7.6k/byte); a one-line task is cheap, a paragraph is not. Cap the
   inline length (e.g. 512 bytes) and budget gas length-scaled like `publish`.
2. **A metadata pointer (RECOMMENDED for richer tasks).** `task` holds a
   `keccak256` key into the target's existing `setMetadata` store (the same store
   that holds personas/app.wasm). The full prompt lives under that key (written
   once, reused every run), and the job just references it — so the per-job
   storage is a 32-byte key, not the prose. This also lets the owner *edit the
   task* without rescheduling (update the metadata; the job re-reads it each run).
3. **A "use the persona's standing instruction" sentinel.** Empty `task` = "run
   the target under its on-chain persona with a default tick prompt" (e.g. the
   persona itself says what to do on each tick — a self-running test-fleet probe,
   §5.4). Cheapest of all; the behavior is entirely in the already-published
   persona.

**Recommendation: support inline (short, MVP) + pointer (rich, Phase 2).** The
worker resolves `taskOf(id)`; if it's a pointer, it does one extra `eth_call` to
fetch the prompt. Most scheduled jobs are short ("ping the deploy", "summarize
new feedback"), so inline covers the MVP; the pointer is the scale path.

### 3.5 Budget escrow mechanics (gas + token, mirrors `depositCredits`)

`scheduleJob` escrows the budget with the **exact approve→pull pattern** already
shipped in `deposit_credits_sponsored` (`registry.rs`) and reused by `invites.md`:
two calls in one sponsored Tempo tx — `approve(diamond, budgetWei)` on the `$LH`
token, then `scheduleJob(...)` which does `transferFrom(owner, diamond,
budgetWei)`. So `scheduleJob`'s gas profile ≈ `depositCredits` (one
approve+transferFrom) + the Job struct's cold SSTOREs + the task write
(length-scaled) + event. **Budget via `cast estimate`, never guess** (CLAUDE.md —
cold SSTOREs + on-chain strings dominate; both `submitFeedback` and `redeem`
under-set their first cap and silently out-of-gassed). `recordRun`/`cancelJob`
are cheaper (a status/scalar flip + a `transfer` + event).

New `registry.rs` helpers mirroring the existing ones: `schedule_job_sponsored`
(approve+schedule, like `deposit_credits_sponsored`), `cancel_job_sponsored`,
`top_up_job_sponsored`, `record_run_sponsored` (the worker's), and reads
`jobs_due`, `job_of`, `task_of`, `jobs_of_owner`, `budget_escrowed_of`.

---

## 4. Recursion / "agent ping-pong" — the critical safety section

The user wants recursion: a job for A calls B; B's run can schedule/call A. This
is the most dangerous part — a recursive scheduler that spends money and spawns
work **must be hard-bounded**, or a loop drains funds and floods the network. The
safety model is *layered*, and the **budget is the ultimate stop**: even if every
softer guard fails, a job that spends its committed `$LH` *stops*.

### 4.1 The hard stops (in order of bluntness)

| Guard | Where enforced | What it bounds |
|---|---|---|
| **Per-job `$LH` budget** *(the ultimate stop)* | `ScheduleFacet` (`budgetWei`, debited per run) | total spend of a single job. Runs out → job → `Done`, remaining refunded. A runaway A↔B loop drains *both jobs' budgets* and **halts** — money is the hard floor. |
| **Per-job max-runs** | `runsLeft` (cap `MAX_RUNS`) | a job fires at most N times ever, regardless of budget. |
| **Per-job expiry / TTL** | `expiry`, bounded by `MAX_TTL` | a job stops by a wall-clock date even if under-fired. |
| **Min interval** | `MIN_INTERVAL` on `scheduleJob` | no sub-minute hammering; bounds firing *rate*. |
| **Recursion depth** | `depth` field, `MAX_DEPTH` | a job created *by another job's run* carries `depth = parent.depth + 1`; `scheduleJob` reverts above `MAX_DEPTH`. Caps how deep a ping-pong chain can nest. |
| **Per-owner active-job cap** | `activeJobsOf[owner]`, `MAX_ACTIVE_PER_OWNER` | one owner can't stand up 10,000 jobs; bounds breadth. |
| **Per-owner total escrow cap** | `budgetEscrowedOf[owner]`, `MAX_ESCROW_PER_OWNER` (owner-knob, default generous) | bounds an owner's total at-risk `$LH`. |
| **Cycle detection** | worker-side + a `rootJobId`/chain lineage | A→B→A within one fired chain is detected and refused (below). |

### 4.2 How recursion is even possible — and bounded

A scheduled run is a headless turn with a *tool surface*. For ping-pong, that
surface must include `schedule_task` and/or `call_agent`. But note the
load-bearing constraint from `autonomous-loop.md` §"The honest gap": **the
headless `call` path today has NO tools** (`enabled_tools: Some(vec![])`) — a
remote/scheduled turn must not get the caller's destructive tools by default. So
recursion is **opt-in per job**: a job declares whether its run may schedule
follow-ups, and the worker grants `schedule_task`/`call_agent` *only* to jobs
flagged recursive, *only* within the depth/budget envelope. A plain "ping the
deploy and report" job has **no** scheduling tools and cannot recurse at all.

When a recursive job's run *does* schedule a child:

- the child's `depth = parent.depth + 1` (worker stamps it; `scheduleJob` reverts
  past `MAX_DEPTH`),
- the child's budget is **drawn from the parent job's remaining budget** (the
  parent can't spawn a child funded beyond what the parent itself holds — so the
  *total* `$LH` a root job's whole tree can spend is bounded by the root budget,
  not multiplied per level), or from the *owner's* wallet with the per-owner
  escrow cap — **decision in §7**; the budget-subtree model is safer (one root
  budget bounds the entire tree),
- a `rootJobId` + lineage is carried so **cycle detection** can refuse a child
  whose `targetId` already appears in its ancestor chain within the same fired
  generation (A→B→A is refused; A→B→A *on a later independent tick* is fine —
  that's just two agents talking over time, which is the point).

### 4.3 Rate limits + the global circuit breaker

- **Per-target rate limit.** An agent can be the *target* of at most K scheduled
  runs per window (so a swarm can't all schedule the same victim agent every
  minute). Enforced as a worker-side throttle + an optional on-chain `lastRunAt`
  per `(targetId)`.
- **Global worker budget.** The cron itself has a per-tick ceiling on how many
  jobs it fires and how much sponsor gas it spends, so a pathological surge can't
  drain the AlphaUSD sponsor (the same "the sponsor/relay is the real
  chokepoint" lesson as `invites.md` §4.4 and `economy-reputation.md`). Over the
  ceiling, jobs spill to the next tick (fair-queued by `nextRun`).
- **The autonomy dial reused.** `autonomous-loop.md`'s `observe`/`exercise`/
  `propose` dial generalizes: scheduling that only *reads/reports* is low-risk;
  scheduling that *spawns recursive work* is the `exercise`+ rung. A user (or the
  platform) can cap the max depth/budget a given owner's jobs may reach.

### 4.4 The framing that makes it safe

The whole recursion design keeps every autonomous action **below the
typed-confirmation line** (CLAUDE.md hard convention): a scheduled job can spend
its *own pre-committed budget* and spawn *budget-bounded children*, but it can
**never** do an irreversible/destructive act unattended (release a name, cut the
diamond, drain the owner's general wallet). Those still require a human typing the
exact confirmation. Recursion lives entirely inside the "spend bounded `$LH`,
append bounded work" envelope — so the ping-pong is *playful within a sandbox*,
not a foot-gun. **The budget is the leash; the leash is short and the owner sets
its length.**

---

## 5. Relationship to the existing pieces

### 5.1 Per-request billing — each run pays from the job budget

A scheduled run is a per-request `$LH` spend, exactly like a `localharness call`.
Two ways to wire the debit, decision in §7:

- **(A) Debit the job budget *inside* `recordRun` (RECOMMENDED).** The worker
  runs the turn, then `recordRun(id, expectedNextRun, spentWei)` *atomically*
  decrements `budgetWei` and advances `next_run` in one tx. The per-run cost is
  the model's per-request price (`COST_PER_REQUEST_WEI` for Gemini, the per-model
  `PRICE_ANTHROPIC[...]` for Claude — `gemini.ts::priceOf`). This makes the budget
  debit and the clock advance a **single commit point** → double-fire is
  structurally impossible (§2.5) and the budget is the hard stop (§4.1). The
  `$LH` moves diamond→(platform treasury or the model-cost sink), mirroring how
  the meter debit works today.
- **(B) Reuse `CreditMeterFacet.meter` against the job owner's metered balance.**
  Simpler (the proxy already calls `meter`), but it debits the owner's *general*
  meter balance, not a *job-scoped* budget — so a job could overspend the owner's
  wallet, weakening the §4 hard stop. **Rejected** unless paired with a per-job
  sub-balance.

**Lean (A)** — a job-scoped budget debited in `recordRun` is what makes "the
runaway loop drains its budget and stops" *true*. The per-run price is read from
the same `priceOf(provider, model)` table the proxy already uses, so Gemini stays
flat and Claude is per-model — a scheduled Opus job costs more budget per run than
a scheduled Haiku job, automatically.

### 5.2 x402 — a job can *pay the target agent*

A scheduled job that calls another agent can settle in `$LH` to that agent's TBA
via the existing x402 path (`X402Facet.settle`, the same the hosted
`/mcp` endpoint and `call_agent`'s x402 hook use). So a job's per-run spend can
be **split**: the model-access cost (to the proxy/treasury) *plus* an x402
payment to the *target agent's wallet* — "schedule a job that pays agent B 0.001
`$LH` each time it runs B." This is agent-to-agent commerce on a timer, drawn
from the job budget, and it's a small extension: the job carries an optional
`payTargetWei`, and `recordRun` settles it from the budget alongside the model
cost. The **test-fleet** (§5.4) becomes a *paying* fleet this way.

### 5.3 Personas — the scheduled turn runs under the target's published persona

`run_agent_turn` already loads `persona_of(targetId)` and runs the turn *as* that
agent. A scheduled job inherits this for free: scheduling agent `alice` means each
run answers under alice's on-chain persona. So "schedule alice to summarize the
feedback log every morning" runs *alice* (her persona, her voice), billed to the
*scheduler's* budget. No persona work needed — it's the same `persona_of` path the
CLI `call` already uses.

### 5.4 The test-fleet becomes self-running (the killer demo)

`autonomous-loop.md` designs a QA fleet driven by `triggers.rs::every()` — but
that requires a **running native `localharness probe` process** (a server/daemon,
or a human keeping it alive). **Scheduling closes that gap exactly:** instead of
an always-on process, each fleet specialist (`qa-security`, `qa-fuzz`, …) is a
*scheduled job* — `scheduleJob(targetId=qa-fuzz, task="run the fuzz suite",
interval=1h, budget=…)`. The proxy cron fires them on cadence with **no process
running anywhere**; the fleet's `qa_report` writes land on the same on-chain
feedback log the triage agent reads. The "standing population of cheap probes"
metaphor becomes literally true — a **self-running fleet that survives every tab
and process death**, which is precisely the persistence the user is asking for,
pointed at the platform's own health. (The tool-surface/sandbox safety from
`autonomous-loop.md` §1b–c still applies to *what* a fired probe may do; this doc
supplies the *firing*.)

This is the demo: schedule the fleet once, close every tab, and the platform
keeps testing itself, reporting on-chain, indefinitely, until the budgets run out.

---

## 6. Agent + CLI surface

### 6.1 The agent tool — `schedule_task`

A new tool registered in `chat.rs::start_session` (alongside `create_subdomain`,
`call_agent`, etc.), so an agent can schedule itself or another:

```
schedule_task(target, task, interval, budget_lh, max_runs?, recursive?)
  → { job_id, target, next_run, budget_lh, tx_hash }
```

- `target` — a `<name>` (resolved to `targetId` via `id_of_name`) or "self".
- `task` — the prompt to run each tick (inline, or a pointer for rich tasks).
- `interval` — human-friendly ("5m", "1h", "daily") parsed to seconds, bounded by
  `MIN_INTERVAL`/`MAX_INTERVAL` (mirrors `/loop`'s interval arg).
- `budget_lh` — the **hard spend cap** (escrowed at schedule time; the run-out
  stop). The tool **must surface this to the user and treat scheduling a paid,
  recurring, self-spawning job as a value-moving action** — like `send_lh`,
  confirm target + interval + budget with the user before committing (CLAUDE.md:
  value moves are confirmed; recursion makes this non-negotiable).
- `max_runs?` / `recursive?` — the count cap and the opt-in recursion flag (§4.2;
  default `recursive=false`, so a scheduled job can't ping-pong unless asked).

Companion read/cancel tools: `list_scheduled_jobs()` (the owner's jobs +
state/budget/next-run) and `cancel_scheduled_job(job_id, confirmation)` — cancel
is *reversible-ish* (it refunds), so it's lower-ceremony than `release_subdomain`,
but still a deliberate act. `schedule_task` is **owner-only, NOT granted to
subagents** (the same restriction `send_lh`/`create_subdomain` carry), and a
scheduled *recursive* job only gets the scheduling tools at run time if flagged.

### 6.2 The CLI — `localharness schedule …`

Mirror the `/loop` UX in the harness-agnostic CLI (`src/bin/localharness.rs`), so
any shell-capable agent schedules without a browser:

```sh
localharness schedule [--as me] <target> <task> --every 1h --budget 5 [--runs 24] [--recursive]
localharness schedule list   [--as me]                 # your jobs: id, target, next run, budget, state
localharness schedule cancel [--as me] <job_id>        # cancel + refund remaining budget
localharness schedule pause  [--as me] <job_id>
localharness schedule top-up [--as me] <job_id> <amount>
```

`schedule` signs `schedule_job_sponsored` with the caller's identity key (the same
key `call`/`create` use), escrows the budget, prints the job id + next-run. It is
the CLI twin of the `schedule_task` tool and the exact "bake in `/schedule`"
surface: once scheduled, **the job runs via the proxy cron with no `localharness`
process running** — the CLI's role ends at `scheduleJob`. (`schedule-tick` — the
self-hosted keeper from §2.7 — is the *opt-in* "run the firer yourself" path.)

### 6.3 Browser studio panel

A `[schedules]` panel in the studio admin chrome (where `send_lh`/the public-face
picker live), all `maud` templates + fragment swaps (no imperative DOM, no JS
alerts — `feedback_ui_no_dom`, `feedback_no_js_alerts`): a form (target / task /
interval / budget / max-runs / recursive toggle) → `Action::ScheduleJob` →
`events::run_schedule_job`, plus a "your scheduled jobs" list reading
`jobs_of_owner` with per-row `[pause]`/`[cancel]`/`[top up]` and the next-run +
remaining-budget. This is the in-tab face of a fundamentally tab-independent
feature: you *set it up* in the tab, then close the tab and it keeps running.

---

## 7. Phased plan + open questions

### 7.1 Phasing

**MVP — durable single-keeper scheduling, no recursion, hard budget cap:**
- `ScheduleFacet` + `LibScheduleStorage` + `script/AddScheduleFacet.s.sol` (I run
  the cut — memory: I do all deploys/cuts, key in `./.env`). `scheduleJob` /
  `recordRun` (CAS-guarded, debit-in-record) / `cancelJob` / `pauseJob` /
  `topUpJob` + paginated `jobsDue` + views. TTL/interval/runs/budget bounded.
- `registry.rs` helpers (`schedule_job_sponsored`, `cancel_job_sponsored`,
  `record_run_sponsored`, `jobs_due`, `job_of`, `jobs_of_owner`); gas via
  `cast estimate`.
- **The worker:** `proxy/api/scheduler.ts` + a `crons` entry in `proxy/vercel.json`
  (e.g. every 5 min). Reads `jobsDue`, runs each due job through the proxy's own
  model-bridge (§2.3 option A — reuse the `gemini.ts` handler), debits the job
  budget in `recordRun`. Idempotent (CAS), skip-don't-pile-up on missed fires.
- The `schedule_task` tool + `localharness schedule` CLI + studio `[schedules]`
  panel. Inline task only; `recursive=false` only (no ping-pong yet).
- A `scripts/` E2E smoke (be-the-e2e-tester): schedule a short-interval job
  against my own `claude.localharness.xyz`, wait two cron ticks, assert `JobRan`
  fired twice + budget decremented + a reply transcript persisted; cancel, assert
  refund.

**Phase 2 — recursion / ping-pong + ergonomics:**
- Opt-in `recursive` jobs: grant `schedule_task`/`call_agent` to flagged runs,
  with `depth`/`MAX_DEPTH`, budget-subtree funding, cycle detection, per-target
  rate limits (§4). The A↔B ping-pong demo.
- Pointer tasks (§3.4 option 2) for rich/editable prompts.
- x402 `payTargetWei` so a job pays the agent it runs (§5.2).
- The self-running test-fleet (§5.4): schedule the QA specialists; close all tabs.
- Off-chain harvest of `JobRan` for a "scheduled-run history" view.

**Phase 3 — decentralize the keeper + richer schedules:**
- Cron *strings* (not just intervals) for calendar schedules ("0 9 * * *").
- Permissionless **keeper network** with an on-chain `keeperBounty` paid from the
  job budget to whoever lands `recordRun` (§2.8) — the storage seam is already
  there; this removes the proxy as the sole firer.
- Bucketed-by-`nextRun` `jobsDue` index if active-job counts grow large.
- The self-hosted `localharness schedule-tick` OS-cron path (§2.7) documented as
  the zero-trust-in-operator option.

### 7.2 Trade-offs to be honest about

- **The proxy cron is a single point of firing (and a trust point).** If the
  proxy is down, jobs don't fire until it's back (they don't *lose* state — the
  chain holds it — they just pause). And the operator's cron key can fire/burn
  *already-scheduled* jobs' budgets (§2.6). Both are the **same** availability +
  trust profile the proxy already carries for *all* model access; scheduling adds
  no new server, just a crontab. The keeper network (Phase 3) is the
  decentralized fix when it's worth the weight. Named so it's a decision.
- **On-chain task storage gas.** Inline prompts cost ~7.6k gas/byte; the pointer
  model (Phase 2) makes rich tasks cheap. MVP caps inline length and budgets gas
  length-scaled (the `publish`/`submitFeedback` lesson).
- **"Skip, don't pile up" vs strict cadence.** The safe default (fire once, catch
  up to one run) means a job idle through an outage doesn't burst-drain. A user
  who *wants* strict cadence (every missed tick replayed) is a §7.3 question — but
  the default protects the budget.
- **Recursion is genuinely dangerous.** It's gated behind an opt-in flag, a
  budget-subtree, a depth cap, and cycle detection — but it's the part most likely
  to surprise. MVP ships *without* it deliberately; recursion is Phase 2 so the
  durable-scheduling foundation is proven first.

### 7.3 Open questions the user should decide

1. **Cron cadence of the worker.** How often should the Vercel cron tick — every
   1 min (snappier, more sponsor gas/eth_calls) or every 5–15 min (cheaper, coarser
   `MIN_INTERVAL`)? *(Recommendation: 5 min MVP; `MIN_INTERVAL` ≥ the tick.)*
2. **Recursion budget model.** Should a recursive child draw from the **parent
   job's** remaining budget (one root budget bounds the whole tree — safer) or
   from the **owner's wallet** (more flexible, capped by `MAX_ESCROW_PER_OWNER`)?
   *(Recommendation: budget-subtree — the root budget is the whole tree's ceiling.)*
3. **Who signs `recordRun`?** A dedicated **scheduler role** the diamond owner
   grants the proxy key (narrow: only `recordRun` + per-job debit), or reuse the
   existing meter key? *(Recommendation: a distinct scheduler role, so the firing
   authority is separable from the metering authority.)*
4. **Billing wiring.** Debit the job budget *in* `recordRun` (job-scoped, the hard
   stop — §5.1A) or via the existing `CreditMeterFacet.meter` on the owner's
   balance (simpler, weaker cap — §5.1B)? *(Recommendation: in `recordRun`.)*
5. **Missed-fire policy.** Default "skip, don't pile up" for everyone, or expose an
   opt-in "strict cadence / replay missed" per job? *(Recommendation: skip default;
   strict as an opt-in flag later.)*
6. **MVP recursion.** Ship MVP *without* ping-pong (durable scheduling first), or
   pull recursion into MVP given it's the headline excitement? *(Recommendation:
   durable single-shot scheduling first — recursion on a proven foundation is the
   safe order; happy to pull it forward if you want the ping-pong demo immediately.)*
7. **Bounds.** Confirm `MIN_INTERVAL` (≥ cron tick), `MAX_RUNS`, `MAX_TTL`,
   `MAX_DEPTH`, `MAX_ACTIVE_PER_OWNER`, `MIN_BUDGET`, `MAX_SPEND_PER_RUN`.

---

## 8. Summary of the recommended approach

A new on-chain **`ScheduleFacet`** (storage `keccak256(
"localharness.schedule.storage.v1")`) holds the durable job table —
`Job{ owner, targetId, interval, nextRun, budgetWei, runsLeft, depth, status }`
plus a separate `task` mapping — where any holder **escrows their own `$LH`
budget** (the exact `approve`+`transferFrom` pattern `deposit_credits_sponsored`
already ships) to back a recurring job. The job + its budget live **on-chain**, so
they **survive any tab or process dying** — the answer to "persistent without
keeping the tab open." A **Vercel Cron on the credit proxy** (the ONE accepted
server) ticks a crontab, reads `jobsDue(now)` from the diamond, and runs each due
job through the **existing tab-free headless `call` path** (`run_agent_turn` —
target persona, model via the proxy, per-request `$LH`), debiting the **job's**
budget atomically in a CAS-guarded `recordRun` that also advances `nextRun`
(idempotent, skip-don't-pile-up). The worker gains **no new authority** — it can
only fire owner-defined jobs and spend their pre-committed budgets, the same
envelope the proxy already lives in; gas is sponsored. Agents schedule via a
`schedule_task(target, task, interval, budget)` tool and a
`localharness schedule …` CLI that mirrors the `/loop` UX; the studio gets a
`[schedules]` panel. **Recursion / "agent ping-pong"** is opt-in and hard-bounded
— a per-job `$LH` budget is the ultimate stop (a runaway A↔B loop drains its
budget and halts), layered with max-runs, expiry, min-interval, recursion-depth,
per-owner active-job + escrow caps, and cycle detection, all kept *below* the
typed-confirmation line so nothing destructive ever runs unattended. It slots onto
every existing primitive: per-request billing (the per-run spend), x402 (a job can
pay the agent it runs), personas (the run answers *as* the target), and the
test-fleet (scheduled persona probes = a self-running fleet that survives every
tab close — closing the exact gap `autonomous-loop.md` left open).

**Top open questions for the user:** (1) worker cron cadence (1 vs 5–15 min);
(2) recursion budget model (parent-subtree vs owner-wallet); (3) a distinct
scheduler role vs reusing the meter key for `recordRun`; (4) debit the job budget
in `recordRun` (job-scoped hard stop) vs the existing meter; (5) whether to ship
MVP without ping-pong (durable scheduling first) or pull recursion into MVP.
