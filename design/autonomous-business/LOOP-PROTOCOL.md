# LOOP-PROTOCOL.md — the enforceable per-tick checklist

> Operationalizes `RISKS.md` into a literal checklist the 30-minute loop follows on
> every tick. This is the gate, not a guideline: each box is a hard, checkable
> control. If any **HARD STOP** (§6) would be tripped, or any budget ceiling (§3) is
> exceeded, or the secret-scan (§5) fails, the tick **aborts** and files a
> `budget-exceeded` / `gate-failed` ledger note — it does not "try a smaller version".
>
> Companion docs: `RISKS.md` (why each fence exists), `BACKLOG.md` (what to pull),
> `LEDGER.md` (append-only proof of work), `loop-secret-scan.sh` (the §5 gate),
> `design/autonomous-loop.md` (autonomy dial), `design/offchain-scheduler.md`
> (claim-by-delete CAS idempotency).
>
> **Scope of the loop's authority:** work lands on the `autonomous-business` branch
> only. The loop **never** merges `main`, deploys, releases, `cargo publish`es, cuts a
> facet, spends `$LH`, or posts to social. Those are human acts (§6).

---

## 0. Definitions (computed once at tick start)

```sh
# UTC tick-window id — floors the current half-hour, so a cron double-fire inside
# the same 30-min slot computes the SAME id and is detected as a repeat (§4).
H=$(date -u +%H); M=$(date -u +%M)
SLOT=$([ "$M" -lt 30 ] && echo 00 || echo 30)
WINDOW="$(date -u +%Y-%m-%d)T${H}${SLOT}Z"     # e.g. 2026-06-30T1430Z
LEDGER=design/autonomous-business/LEDGER.md
```

- **Tick-window** = the UTC half-hour slot this tick belongs to. One unit of loop
  work per window, max.
- **Idempotency key** (per unit of work) = `keccak(role‖YYYY-MM-DD‖task-slug)` — used
  in §4 to skip a task already done today.
- **Autonomy dial** (`design/autonomous-loop.md`) = master switch. Default `observe`.
  This whole protocol runs only at `observe`/`exercise`/`propose`; a missing or
  `OFF` dial → the tick no-ops immediately.

---

## 1. PRE-TICK — environment & state (run in order; any failure ⇒ abort)

- [ ] **1.1 Dial check.** Read the autonomy dial. If absent or `OFF` → log `dial-off`,
      `exit 0`. No further steps.
- [ ] **1.2 On the right branch.** `git rev-parse --abbrev-ref HEAD` **must** be
      `autonomous-business`. If not: `git checkout autonomous-business`. Never work on
      `main`, never on another agent's `worktree-*` branch.
- [ ] **1.3 Clean tree.** `git status --porcelain` is empty (no stray uncommitted work
      from a crashed prior tick). If dirty → **abort** + file `dirty-tree`; a human
      reconciles. Do **not** `git stash`/`git checkout -- .` blindly (that can destroy
      a parallel agent's WIP — the `git add -A` scar, RISKS.md b.3/b.5).
- [ ] **1.4 Pull latest.** `git fetch && git rebase origin/autonomous-business` (or
      fast-forward). Stops the tick from branching off a stale head.
- [ ] **1.5 Read state.** Read `LEDGER.md` (newest entry = what last shipped) and
      `BACKLOG.md` (NEXT TICK first, then the ranked queue). The tick's task set comes
      from BACKLOG, never invented.
- [ ] **1.6 Credential check — assert the loop holds NO forbidden creds.** The loop's
      environment must contain *only* a PR-only GitHub token (no merge, no `main`
      push) and, if marketing is live, draft-only scoped tokens. The following must be
      **absent** (assert empty):
      `VERCEL_TOKEN`, `CARGO_REGISTRY_TOKEN`, any `*_MAINNET_*KEY*` /
      diamond-owner / sponsor key, any live-post social token outside the approved
      queue. If any forbidden cred is present → **abort** + file `creds-overscoped`;
      a human fixes scope. *(The loop reads creds from env/`.env.marketing`, never
      from tracked files; §5 enforces they're never committed.)*
- [ ] **1.7 Circuit-breaker check.** If the last **3** ticks each ended in `abort`/
      `error`, or the per-day spend (§3) is already at ceiling → flip the dial to
      `OFF`, notify the human, `exit 0`. No silent thrash (RISKS.md b.7).

---

## 2. IDEMPOTENCY GATE — has this window already run? (§4 has the full rules)

- [ ] **2.1 Window already stamped?**
      ```sh
      grep -q "tick-window: ${WINDOW}" "$LEDGER" && { echo "window ${WINDOW} done — no-op"; exit 0; }
      ```
      A ledger entry for this window already exists ⇒ a double-fire ⇒ **no-op exit 0**.
- [ ] **2.2 Overlap guard.** Honor the harness `TURN_ACTIVE` / `send_when_idle`
      discipline: if a previous tick's turn is still live, **do not stack** — the new
      tick no-ops. One turn at a time (RISKS.md b.6).
- [ ] **2.3 Per-task claim (claim-by-delete CAS).** For each task pulled from BACKLOG,
      compute its idempotency key and claim it via the off-chain scheduler's
      claim-by-delete CAS (`design/offchain-scheduler.md`) **before** doing the work.
      A task whose key is already present/claimed is **skipped**, not redone. Exactly
      one worker claims a job.

---

## 3. BUDGET CEILING — concrete, per-tick (checked BEFORE acting; exceed ⇒ abort tick)

These are hard numbers. A check runs before every spend/post/agent-spawn; the first
breach **aborts the tick and files `budget-exceeded`** — it does not degrade-and-retry.

| Resource | Per-tick ceiling | Per-UTC-day ceiling | Default | Justification |
|---|---|---|---|---|
| **Parallel role-agents** | **6** | — | 6 | Matches the role roster and Tick 1's proven fan-out of 6; each agent gets its **own worktree** (no two agents share a tree — RISKS.md b.5), so 6 bounds tree-creation, context, and cost to a predictable envelope. >6 risks tree collisions + an unreviewable single-tick diff. |
| **On-chain writes** | **0** | **0** | 0 | All work lands on the `autonomous-business` branch. On-chain writes spend the single drainable sponsor float and are irreversible; 0 means the tick never even *attempts* a typed-confirm-gated/value action. Raising it is an explicit operator act, never a loop decision. |
| **`$LH` spent** | **0** | **0** | 0 | Every `$LH` move rides the typed-confirmation gate the loop structurally cannot satisfy (RISKS.md b.4 / d.6). 0 is the only value consistent with "no human inside the tick." |
| **Live social posts** | **0** | **0** | 0 (until creds + per-post approval) | Social posting is never a closed loop (RISKS.md a.4 / d.2). The loop holds no live-post credentials; its ceiling is **draft → review queue**. Stays 0 until creds land in gitignored `.env.marketing` **and** a human approves each post/batch. Even then, per-platform cadence caps apply (§7). |
| **LLM spend** | **$5.00 USD** | **$40.00 USD** | $5 / $40 | Per-tick $5 bounds ≤6 agents' combined token burn so one runaway agent can't spend unbounded; exceed ⇒ abort + `budget-exceeded`. Per-day $40 ≈ 8 full-budget ticks — chosen well below the theoretical 48-tick × $5 ceiling so sustained max-spend trips the **dial OFF** (§1.7) instead of running unbounded. |
| **Commits** | **1 per role-branch** (≤6) | — | 1/role | One reviewable commit per role keeps branch history auditable; prevents a fan-out agent from churning many partial commits. |
| **Files staged / commit** | role's **declared output paths only** | — | explicit | Never `git add -A`/`.` (§5). Stage only the paths the role's task names. |

> Any breach ⇒ **abort the tick**, append a `budget-exceeded` ledger note with the
> resource + observed value, leave the branch untouched. Do **not** retry smaller.

---

## 4. IDEMPOTENCY — how a double-firing cron no-ops (the mechanics)

A 30-min cron **will** double-fire (overlap, retry, restart). Three layers make a
repeat a no-op:

1. **Window stamp (the primary check, §2.1).** Every completed tick writes a
   machine-readable stamp into its ledger entry:
   ```
   <!-- tick-window: 2026-06-30T1430Z -->
   ```
   The next firing computes `WINDOW` (§0) and greps the ledger for it **before doing
   any work**. Present ⇒ this window already ran ⇒ `exit 0`. Because `WINDOW` floors
   the half-hour, *any* re-fire inside the same 30-min slot collides on the same id.
2. **Per-task claim-by-delete CAS (§2.3).** Within a window, each task is claimed in
   the off-chain job store before work begins; a claimed/absent key is skipped. This
   makes individual units of work exactly-once even if two workers race.
3. **Overlap guard (§2.2).** `TURN_ACTIVE` / `send_when_idle` — a tick that starts
   while the prior turn is unfinished no-ops instead of stacking.

**The stamp is load-bearing:** a tick that does real work but fails to write its
window stamp is a bug — the next fire would redo it. Writing the ledger entry
(with the stamp) is the **last** committed step (§8) so a crash mid-tick leaves the
window unstamped and the work safely re-runnable, never half-committed-then-stamped.

---

## 5. COMMIT GATE — secret-scan + explicit-path add (no commit ships without this)

- [ ] **5.1 Stage explicit paths ONLY.** `git add <path> <path> …` naming each file the
      task produced. **Never** `git add -A`, **never** `git add .`. (RISKS.md b.3/d.5:
      `git add -A` is the single most likely way a token or seed leaks.)
- [ ] **5.2 Run the secret-scan on the staged set.** It hard-fails on token/key/secret
      patterns and **gates the commit**:
      ```sh
      sh design/autonomous-business/loop-secret-scan.sh   # default: scans staged files
      ```
      Non-zero exit ⇒ **do NOT commit**. Unstage, remove the secret, file
      `secret-scan-blocked`. A secret that reached staging is also a `creds-in-tree`
      incident — investigate how it got there.
- [ ] **5.3 `.gitignore` sanity.** Confirm the staged set contains no `.env*` (except
      `.env.example`), no `*.key`, no `*.localharness.key`, no `.lh_*`, no
      `.env.marketing` / `~/.lh_marketing_secrets`. (These are already gitignored; the
      scan is the backstop, this is the belt.)
- [ ] **5.4 Commit message** names the role + tick-window + backlog item. One commit
      per role-branch (§3). End with the project's `Co-Authored-By` trailer.
- [ ] **5.5 Push the branch only** (`git push origin auto/<role>/<window>` or
      `autonomous-business`). **Never** push `main`. **Never** `--force` a shared branch.

---

## 6. HARD STOPS — the loop NEVER does these (no flag weakens them)

If a task would require any of the below, the tick **stops and queues it for a human**
— it does not attempt, work around, or "ask for confirmation" itself.

- [ ] ⛔ **Merge to `main`** — PR is the ceiling; a human merges (RISKS.md b.1/d.3).
- [ ] ⛔ **`vercel deploy` / `vercel --prod`** — web or proxy, *especially* from a
      worktree (spawns a stray Vercel project; clobbers prod). The loop holds no
      deploy creds (RISKS.md b.2).
- [ ] ⛔ **`cargo publish` / `release.sh` / `release.ps1` / tag a version** —
      irreversible; releasing is a human act (RISKS.md b.2/d.3).
- [ ] ⛔ **`diamondCut` / facet upgrade / any owner-gated admin** — the owner key is
      not in the repo; keep it that way.
- [ ] ⛔ **`send_lh` / `release_subdomain` / any value-moving or destructive tool** —
      the typed-confirmation gate (`chat::confirm_guard`) requires a human-echoed
      single-use code the loop cannot produce. **Do not weaken this gate** (d.6).
- [ ] ⛔ **Commit anything touching secrets / `.env` / keys** (§5).
- [ ] ⛔ **Two agents on one working tree** — worktree-per-agent or serialize (b.5).
- [ ] ⛔ **Any live social post** without §7 approval.
- [ ] ⛔ **Cross-agent engagement** — loop agents never like/RT/upvote/comment on each
      other's posts (voting-ring/astroturf → domain ban; RISKS.md d.12).

---

## 7. SOCIAL-POSTING APPROVAL GATE (draft-only until a human approves each post)

The autonomous ceiling for social is **draft → review queue**. Live posting is `0`
(§3) until **both** conditions hold, per post/batch:

- [ ] **7.1 Creds present.** Scoped, draft/post-only social tokens exist in gitignored
      `.env.marketing` (per `marketing/CREDENTIALS.template.md`). Absent ⇒ channel is
      "disabled"; the agent only *prepares* assets.
- [ ] **7.2 Disclosure + label attached at DRAFT time** (content-generation-time, not
      post-time). Every draft carries all three or it is refused into the queue:
      (a) AI-generated disclosure, (b) material-connection disclosure if it endorses
      the product, (c) the platform's native AI/bot label (RISKS.md a.2/d.9).
- [ ] **7.3 Topic denylist passes.** Draft refused outright if it contains a
      financial/earnings/investment claim about `$LH`, political content, a competitor
      attack, or names/impersonates a real third party (RISKS.md a.3/d.10). Flagged
      drafts go to a human, never the queue.
- [ ] **7.4 Per-platform cadence + de-dup budget OK** (RISKS.md d.11; `GROWTH.md` §2):
      e.g. X ≤1 substantive post/day and no near-duplicate across accounts; Reddit
      ≤1 self-promo per ~2 weeks per sub under the 9:1 rule; **HN = human-only, never
      programmatic, never solicit upvotes**; official APIs only, no bypass tools.
- [ ] **7.5 Human approves the specific post/batch.** Explicit operator act — never
      inferred from the agent's own confidence. The loop holds no live-post credential
      in its autonomous path; the post token lives only in the approval service.
- [ ] **7.6 Domain-reputation guard.** Any post linking `localharness.xyz` / a
      subdomain at volume into HN/Reddit is an extra human gate (one-way-door domain
      shadowban; RISKS.md a.1/d.14).

---

## 8. POST-TICK — ledger entry is the LAST step (so a crash stays re-runnable)

- [ ] **8.1 Append one `LEDGER.md` entry** (newest at top) for this window, including:
      - the `<!-- tick-window: <WINDOW> -->` stamp (§4 — without it the window re-runs);
      - what each role shipped, with file paths and the branch/PR opened;
      - findings worth carrying forward;
      - any `budget-exceeded` / `gate-failed` / human-blocked notes;
      - the realized LLM spend for the tick (for the per-day running total, §3).
- [ ] **8.2 Update `BACKLOG.md`** — move completed items out of NEXT TICK; re-rank.
- [ ] **8.3 Emit a structured audit record** (who/what/tick-id/cost) to the existing
      telemetry bus (RISKS.md b.7/d.15).
- [ ] **8.4 Stage + scan + commit** the ledger/backlog edits via §5 (explicit paths,
      secret-scan, no `-A`). This is the final commit of the tick.
- [ ] **8.5 Open a PR** for any code branch (`propose` rung). **Never self-merge.**

---

## 9. ABORT path (any gate failure above)

1. Stop work; leave the branch untouched (no partial commit).
2. Append a ledger note: `gate-failed: <which> @ <WINDOW>` + the observed value.
   **Do not** write the `tick-window` stamp on an aborted tick that did no shippable
   work, *unless* the abort itself is the unit of work for this window (so a retry
   re-attempts) — prefer leaving the window unstamped so the next fire can retry once
   the blocker clears.
3. If the abort is the 3rd consecutive (§1.7) ⇒ flip the dial `OFF` + notify human.
4. Never escalate privilege, never weaken a gate, never retry a *budget* breach
   smaller. Human-blocked work goes to `BACKLOG.md` → "Blocked on the human owner".
