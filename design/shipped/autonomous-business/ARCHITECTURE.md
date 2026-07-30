# Autonomous Business — Architecture Map

> Contributor orientation for the autonomous-business system: how the pieces fit,
> where the pure/impure boundary is, and what is actually built vs. designed. Read
> alongside `STRATEGY.md` (the thesis + role→primitive map), `COMPANY-FEATURE.md`
> (the `found_company` design + build plan), and `src/work_cycle.rs` (the decision
> core). This file is the *map that ties them together* — it does not restate them.

## 1. The reduction (what a "company" actually is)

A company is **not a new on-chain object**. It is a *named composition* of shipped
primitives: an on-chain **GuildFacet guild** (org identity NFT) whose **ERC-6551
TBA** is the pooled `$LH` treasury, staffed by **N role subdomains** (each an
identity NFT + TBA with an on-chain persona), coordinating over a **shared backlog**,
moving value over **BountyFacet** escrow + `$LH` payroll, judging work with
**ReputationFacet** attestations. No new facet is required (`COMPANY-FEATURE.md §1`).

So the whole system is an **orchestration layer over existing tools** — the same
"recombination, not new infra" discipline as the coordination ladder.

## 2. The arc — five layers, setup → execution

```
found_company           role-agents          work_cycle           work_cycle_runtime        (future) executor
(SETUP, one call)   →   (the workers)    →   (PURE decision   →   (PURE planning shell  →   (greenlight-gated
                                              core: Actions)       over the core)            I/O: Actions → tx)
```

1. **`found_company` — SETUP.** One sponsored call stands up the org: create the
   guild (identity + treasury), optionally seed the treasury, register N role
   subdomains in one batch tx, set each role's on-chain persona (+ optional TBA
   prefund) in one more batch tx, and seed the mission + backlog into SessionRoom
   KV. Returns a manifest `{ guild_id, treasury, roles:[{role,subdomain,url,tba}],
   backlog }`. **Model A (solo-founder):** every role NFT is owned by the founder's
   wallet, which is the guild's sole Admin — governance is single-controller for
   now, named-not-faked (`COMPANY-FEATURE.md §1.1`). Browser:
   `src/app/chat/tools/company.rs::found_company_tool`. CLI:
   `src/bin/localharness/company.rs::company_found`.

2. **role-agents — the workers.** Each role subdomain is a persona-bearing agent
   (coder/reviewer/pm/exec/accounting/hr/marketing). They are the entities the work
   cycle allocates tasks to and pays. `company_status` reads the roster + treasury
   back from chain (`company_status_tool` / `company_status`).

3. **`work_cycle` — the PURE decision core** (`src/work_cycle.rs`). Models ONE
   `claim → work → judge → pay → attest` cycle as DATA. Every decision is a pure
   function (`assign_next_task`, `evaluate_result`, `compute_payout`) and every side
   effect is an [`Action`] *descriptor* — `PostBounty`, `AssignTask`, `AcceptResult`,
   `RejectResult`, `Payout`, `Attest`. `step(&State) -> (State, Vec<Action>)`
   advances exactly one transition (judge-in-flight first, then assign, then post).
   Zero I/O, zero chain deps, native + wasm clean, fully unit-tested — the
   `keeper.rs`/`lessons.rs`/`confirm.rs` pattern of a testable core hoisted out of
   the wiring.

4. **`work_cycle_runtime` — the PURE planning shell** (`src/work_cycle_runtime.rs`,
   *added this tick*). A thin layer that turns live reads into a previewed plan
   **without executing anything**. Conceptually: a `Reader` trait abstracts the
   chain reads (treasury balance, guild members, open bounties, worker reputation);
   `plan_cycle(reader, …)` builds a `work_cycle::State` from those reads, runs the
   pure `step` driver, and returns a `CyclePlan` = the `Action`s the cycle *would*
   take. **Preview-only:** it never signs, never broadcasts, never moves `$LH`. It
   exists so a contributor (or a dry-run UI/CLI) can see the proposed
   assign/judge/pay/attest plan before any executor is wired. Like `found_company`'s
   CLI `--confirm` preview, the default is "show the plan, write nothing."

5. **future executor — greenlight-gated I/O (DEFERRED).** The only layer that does
   real writes. It maps each `work_cycle::Action` onto a sponsored edge call — the
   mapping is already documented per-variant in `work_cycle.rs`:
   - `PostBounty`   → `registry::post_bounty_sponsored`
   - `AssignTask`   → `registry::claim_bounty_sponsored`
   - `AcceptResult` → `registry::accept_result_sponsored`
   - `Payout`       → a TBA `$LH` transfer / `send_lh` / x402 settle
   - `Attest`       → `registry::attest_sponsored`
   It rides the typed-confirmation gate (`confirm_guard`) and the relay's
   onboarding/float breaker, exactly like every other value-moving path. **Not
   built yet** — the Action→tx executor is the open seam.

## 3. The design principle — pure core vs. I/O shell

The load-bearing boundary: **decisions are pure + tested; I/O lives in thin shells.**

```
 ┌──────────────────────────── PURE (native+wasm, unit-tested, zero deps) ──────────────────────────┐
 │                                                                                                   │
 │   work_cycle.rs                                  work_cycle_runtime.rs (added this tick)           │
 │   ┌───────────────────────────────┐             ┌─────────────────────────────────────────────┐  │
 │   │ assign_next_task()            │             │ trait Reader  (treasury / members /          │  │
 │   │ evaluate_result()             │  ◀────uses── │                bounties / reputation)        │  │
 │   │ compute_payout()              │             │                                              │  │
 │   │ step(State) -> (State,        │             │ plan_cycle(reader) ─builds─▶ State            │  │
 │   │                 Vec<Action>)  │ ───emits───▶ │                  ─runs step─▶ CyclePlan      │  │
 │   └───────────────────────────────┘   Action     │                  (Vec<Action>, PREVIEW ONLY) │  │
 │            ▲                            (data)    └─────────────────────────────────────────────┘  │
 │            │ State (Backlog/Workers/treasury)                         │                            │
 └────────────┼─────────────────────────────────────────────────────────┼────────────────────────────┘
              │                                                          │
   ═══════════╪════════════ THE BOUNDARY (no chain calls cross upward) ══╪═══════════════════════════════
              │                                                          │
 ┌────────────┴──────────────────── I/O SHELLS (impure: sign, broadcast, read chain) ────────────────┐
 │                                                                       │                            │
 │   found_company / company_status            Reader impl ─reads──▶ registry views                  │
 │   (compose sponsored registry helpers:      (guilds_of / treasury_balance_of /                    │
 │    create_guild · batch subdomains ·         members_of_guild / role_of_guild /                   │
 │    set_persona · fund · KV backlog)          open_bounties / reputation_of)                       │
 │                                                                       │                            │
 │   future executor ◀──────────────────── maps each Action ────────────┘   (greenlight-gated,       │
 │     PostBounty→post_bounty_sponsored · AssignTask→claim_bounty_sponsored ·  confirm + relay)       │
 │     AcceptResult→accept_result_sponsored · Payout→TBA transfer · Attest→attest_sponsored           │
 └───────────────────────────────────────────────────────────────────────────────────────────────────┘
```

Why it matters for contributors: **add new business logic to `work_cycle.rs` with a
unit test — never reach for a chain call there.** A read belongs behind the `Reader`
trait; a write belongs in the executor as an `Action` mapping. The shells stay thin
and the invariants (allocation fairness, payout-clamps-to-treasury,
hallucination-auto-reject, reputation-on-accept) run under `cargo test`.

## 4. Roles → on-chain primitives

Each role is an identity NFT + TBA with an on-chain persona; the work cycle matches a
task's required `Role` to a worker that fills it. Full mapping in `STRATEGY.md §(a)`
and `COMPANY-FEATURE.md §2`; the spine:

| Business role | `work_cycle::Role` | Primary primitive | Where it plugs in |
|---|---|---|---|
| **Coder / IC** | `Coder` | **BountyFacet** (`post_bounty`→`claim_bounty`→`submit_result`) | claims an assigned task, delivers; `Action::AssignTask`/`Payout` settle to its TBA |
| **Reviewer** | `Reviewer` | **ReputationFacet** (`attest`) + the judge core | scores a `Submitted` result; `evaluate_result` → `Action::AcceptResult`/`RejectResult` + `Attest` |
| **PM** | `ProductManager` | backlog: **bounties** (commitment) + **SessionRoom KV** (coordination) | decomposes mission → tasks; promotes a planned item to a posted bounty (`Action::PostBounty`) |
| **Executive (CEO)** | `Executive` | **VotingFacet** + treasury proposals + the heartbeat | direction + funding; treasury spends are (Model B) proposals, single-controller today |
| **Accounting** | `Accounting` | **$LH (CreditsFacet/TIP-20)** + **CreditMeterFacet** + **TBA treasury** | payroll = `Action::Payout` (clamped to treasury by `compute_payout`); watches the float |
| **HR** | `Hr` | identity mint + `set_persona` + **GuildFacet roles** | hires = mint role subdomain + persona + invite; promotes on `reputation_of` |
| **Marketing** | `Marketing` | public face (`publish`) + `notify` + **InviteFacet** | perpetual growth agent; owns the company's public face |

**Treasury / governance backbone:** **GuildFacet** (org identity + pooled `$LH` TBA),
**TbaFacet** (every role's spendable wallet, EIP-6551), **VotingFacet** (Model-B
multi-party governance), **CreditsFacet** (`$LH` payroll). Recursion is free: a
division is a guild-of-guilds (a sub-team's TBA is a member of the parent guild).

## 5. Honest status per layer

| Layer | Module / location | Status |
|---|---|---|
| **`found_company` (browser)** | `src/app/chat/tools/company.rs::found_company_tool` | **SHIPPED** — registered in `session.rs` (both backends), allowlist-gated + `confirm_guard`-gated (`confirm_guard.rs:70`). Model A only. |
| **`company_status` (browser)** | `src/app/chat/tools/company.rs::company_status_tool` | **SHIPPED** — read-only, registered in both backend branches. |
| **`company found/status` (CLI)** | `src/bin/localharness/company.rs` | **SHIPPED** — `found` has a `--confirm` dry-run preview (writes nothing without it); honors `LH_CHAIN`. Pure plan helpers (`company_slug`/`resolve_roles`/`parse_amount_flag`) golden-tested. |
| **`work_cycle` (decision core)** | `src/work_cycle.rs` | **PURE CORE, SHIPPED** — `pub mod work_cycle` in `lib.rs`; full unit suite (assign / evaluate / payout-clamp / accept+reject cycles / multi-task drive-to-terminal). Emits `Action`s; performs no I/O. |
| **`work_cycle_runtime` (planning shell)** | `src/work_cycle_runtime.rs` | **PREVIEW-ONLY, in progress (this tick)** — `Reader` trait → `plan_cycle` → `CyclePlan`. Builds `State` from reads, previews `Action`s. **Never executes.** |
| **executor (Action → sponsored tx)** | — | **DEFERRED** — the per-variant `registry` mapping is documented in `work_cycle.rs`, but no module wires `Action`s to real sponsored calls yet. The greenlight-gated write seam. |
| **Multi-party governance (Model B)** | — | **DEFERRED** — Phase 1 is single-controller (founder owns every role). TBA-as-member voting is named, not faked (`COMPANY-FEATURE.md §1.1`, `§7.3`). |
| **`set_role` / `attest` browser tools** | gap #1 / #2 | **GAP** — CLI-only today (`registry::set_role_sponsored`, `attest_sponsored`); no browser closure-tool wrapper yet (`COMPANY-FEATURE.md §2.1`). |
| **Tab-free always-on heartbeat** | off-chain scheduler | **CONSTRAINED** — the scheduler tick's reduced tool set can't `post_bounty`/`spend_treasury` without a co-located host (`STRATEGY.md` blocker; `COMPANY-FEATURE.md §7.2`). |

## 6. The carried-forward hard problems (not solved here)

Inherited from oggoel + the coordination ladder, stated honestly:

1. **Self-funding is unsolved.** A company burns `$LH` per turn and is
   seed-capitalized; net-positive needs *external* paying callers (x402 into role
   TBAs) above inference burn. The plumbing is live; the demand is the open problem.
2. **Verification of non-deterministic work** is THE load-bearing unsolved question.
   The reviewer/judge-panel + reputation is a lagging, gameable-at-the-margin signal
   (`evaluate_result` catches serverless-impossible hallucinations + a quality bar,
   not trustless judgment). Staked **ValidationFacet** only covers the
   deterministic subset (rustlite compile-to-hash).
3. **Sponsor-key drain.** Every founding/cycle write is a sponsored Tempo tx on the
   one low-budget sponsor SPOF; `found_company` already batches its fan-out (one tx
   for N subdomains, one for N personas) and respects the relay float breaker, but a
   company-spam loop is a real attack surface.
