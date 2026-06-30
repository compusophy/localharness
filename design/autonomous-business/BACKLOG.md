# Backlog

Prioritized cross-role queue. The 30-min loop pulls from **NEXT TICK** first, then
the ranked backlog. Tags: `[role][effort S/M/L][impact H/M/L]`.

## DONE (tick 12 — 2026-06-30)

- ✅ Marketing: `AUDIENCE-INTEL.md` (sourced market intelligence) + `CAMPAIGN-02.md`
  (the `git log` transparency campaign).
- ✅ `README.md` refreshed → points to STATUS.md/DECISIONS.md; reflects complete surface.

## DONE (tick 11 — 2026-06-30)

- ✅ `company forecast` CLI over `simulation` — CLI surface now COMPLETE
  (found→status→plan→payroll→books→day→forecast). 45 CLI tests.
- ✅ `STATUS.md` (one-page owner state-of-the-business).
- ✅ Marketing: `CAMPAIGN-01.md` — the novel `whoami` launch campaign.

## DONE (tick 10 — 2026-06-30)

- ✅ `company day` CLI (one-shot status+plan+payroll+books report) + 42-assertion golden
  integration test locking the whole surface. 41 CLI tests.
- ✅ `src/simulation.rs` — pure multi-cycle forecast (runway/throughput). 10 tests.
- ✅ Marketing: `SEO-LANDING.md` (organic/answer-engine copy; Apache-2.0 verified).

## DONE (tick 9 — 2026-06-30)

- ✅ `hiring` is the canonical ranker (assign_next_task delegates; behavior-preserving)
  + read-only `company books` CLI (accounting preview). 40 CLI tests.
- ✅ `examples/autonomous_company.rs` — runnable end-to-end demo (compiles + runs).
- ✅ Marketing: `PRESS-KIT.md` + `OUTREACH-TEMPLATES.md`.

## DONE (tick 8 — 2026-06-30)

- ✅ Pure `src/accounting.rs` (seed-vs-earned net position, runway, breakeven) + pure
  `src/hiring.rs` (role-fit ranking) — 26 tests; all 7 business roles now have logic.
- ✅ Docs: `CONTRIBUTING.md`. Marketing: `VISUAL-BRIEFS.md` (6 IG/TikTok concepts).

## DONE (tick 7 — 2026-06-30)

- ✅ `company plan` (registry-backed Reader → preview CyclePlan) + `company payroll`
  (treasury/TBA balances + suggested split) — read-only CLI, 36 tests, no broadcast.
- ✅ Docs: `SDK-QUICKSTART.md` (use-as-a-library). Marketing: 10 more X posts (batch 2).
- ▶ The full DRY path is now CLI-runnable: found → status → plan → payroll.

## DONE (tick 6 — 2026-06-30)

- ✅ `work_cycle_runtime.rs` — pure preview-only planning shell (Reader → plan_cycle →
  CyclePlan, never executes); 7 tests, native+wasm+clippy clean.
- ✅ CLI `--roles` quirk fixed (Option: absent→defaults, present-empty→error); 24 tests.
- ✅ Docs: `ARCHITECTURE.md` (boundary diagram). Marketing: dev.to #3 + `CALENDAR.md`.

## DONE (tick 5 — 2026-06-30)

- ✅ Pure `work_cycle` core (`src/work_cycle.rs`) — the "company does work" decision
  logic as data (assign→judge→pay→attest); 13 tests, native+wasm clean.
- ✅ CLI hardened — +17 `company` tests (6→23); 2 benign quirks documented.
- ✅ Docs: `FOUND-A-COMPANY.md` quickstart. Marketing: LinkedIn + founder X thread.

## DONE (tick 4 — 2026-06-30)

- ✅ CLI twin shipped & verified — `company found`/`company status` (broadcast-free
  preview without `--confirm`); native check + drift PASS; no tx broadcast.
- ✅ `DECISIONS.md` — consolidated owner decision brief (8 calls, recs, reply menu).
- ✅ Marketing: 2nd dev.to (x402+6551) + r/ethdev human-gated draft.

## ⏸ BLOCKED ON OWNER (see DECISIONS.md — answer to proceed)

- Testnet-dogfood greenlight (unblocks the next QA tick) · build-vs-reset · social creds
  · address relabel · sponsor float · MTL/`$LH` · draft-PR-to-main autonomy.

## DONE (tick 3 — 2026-06-30)

- ✅ `found_company` WRITE half shipped & wasm/drift-verified — full Model-A founding
  pipeline (guild + treasury + role subdomains + personas + KV backlog → manifest).
- ✅ Address drift RESOLVED as not-a-bug (on-chain proof; `ADDRESS-DRIFT.md`) — only
  CLAUDE.md/AGENTS.md table is mislabeled; flagged to owner, not auto-fixed.
- ✅ Marketing READY-QUEUE expanded (build-in-public X thread + Show HN/Reddit human-gated).

## DONE (tick 2 — 2026-06-30)

- ✅ `company_status` (read-only) + `set_role` + `attest` browser tools.
- ✅ Marketing accuracy pass + DEVTO-ARTICLE + READY-QUEUE.
- ✅ Loop guardrails (`LOOP-PROTOCOL.md` + `loop-secret-scan.sh`, budgets, idempotency).

## NEXT TICK (product complete + marketing deep — consolidate, then await owner)

- **[Marketing][S][M] `LAUNCH-RUNBOOK.md`** — the turnkey "when creds land, fire in THIS
  order" operator runbook: consolidates CALENDAR + READY-QUEUE + both campaigns
  (`whoami` → `git log`) into one dated execution sequence with the per-step human-approval
  gates, so day-one is mechanical the moment the owner provides tokens.
- *(Then: marketing depth is high — further ticks risk low marginal value. Options: a
  periodic AUDIENCE-INTEL refresh, a single new creative artifact, or hold for the owner.)*
- *(Owner-gated — testnet dogfood, address relabel, mainnet founding, real social posting,
  the live Action executor — wait on DECISIONS.md; NOT auto-run.)*

> Note: BOTH the product (CLI + 5 cores + example) AND a deep marketing library (17 assets,
> 2 campaigns, intel, SEO) are now in place. The honest high-value frontier is entirely
> owner-gated (live execution, real posting). The loop will keep producing modest
> non-blocked value, but the real unlock is the DECISIONS.md answers.

## Ranked backlog (from STRATEGY.md)

1. [PM][S][H] Instantiate the business guild on mainnet (create_guild) + fund treasury; pass the TBA 0x, not the name (oggoel gotcha).
2. [HR][M][H] Mint the canonical role roster (coder, reviewer, QA, PM, marketing, accounting), write each set_persona, invite + consent into the guild with roles.
3. [QA][S][H] Dogfood the full cycle headless first (localharness colony run --as claude …): escrow→claim→work→judge→pay→attest, proven via CLI before any human depends on it.
4. [Ops][S][H] Fund + monitor the mainnet sponsor float (the SPOF behind every sponsored write) + balance alerting on the one drainable resource.
5. [Ops][M][H] Close the tab-free authority gap: decide scheduler-role post_bounty/spend_treasury facet vs a co-located CLI host (oggoel phase-2 blocker for always-on ops).
6. [Eng][M][H] Build the "fork-a-company" template — one script/command that spins up guild + role roster + personas (the flagship product seed; reuses create_and_publish_app).
7. [Accounting][S][H] Stand up payroll: batch_send_lh from treasury → role TBAs + TitheFacet auto-refill (consent-safe permissionless collect_tithe); check_balances dashboard.
8. [QA][M][H] Wire accept_result to write a ReputationFacet attestation (proof-of-transaction gated) so accepted work accrues worker reputation that ranks future claims.
9. [Product][M][H] Give the ?explore agent directory a UI entry point — the marketplace/storefront is currently invisible; the company needs a front door.
10. [Eng][S][M] Prime internal demand: the business posts its own first bounties (QA tasks, doc fixes, rustlite-bug repros) so the board isn't empty (the demand chicken-and-egg).
11. [Reviewer][M][M] Reviewer role-agent: a subagent that reads submit_result artifacts and recommends accept/reject before payout; reputation-threshold who-may-claim on high-value bounties.
12. [PM][M][M] Standing decision cadence: a scheduled PM agent opens propose_measure for backlog prioritization on an interval; council cast_vote + execute_proposal.
13. [Marketing][M][M] Perpetual growth agent (goal loop) that posts showcase content, runs discover_agents, and sends refundable invite codes to seed newcomers.
14. [Accounting][M][M] Honest cost accounting: track per-cycle inference burn vs $LH revenue; report true net-position (oggoel was seed-capitalized — don't claim self-funding).
15. [Accounting][S][M] Set an external revenue path: advertise a per-call $LH price above inference cost so paying callers (x402 --pay) make a role-agent net-positive.
16. [PM][S][M] Adopt WeightedVotingFacet share table so contribution maps to voice (vs flat 1-agent-1-vote); record the cap table.
17. [Eng][M][M] Prove a guild-of-guilds division (oggoel-labs pattern) — e.g. a security/QA sub-org whose TBA is a member + voter of the parent guild.
18. [Ops][M][M] Spend-velocity circuit breaker at the relay/sponsor boundary (road-to-v1 blocker #5) — protect treasury + sponsor from spam/recursive churn.
19. [Product][L][H] Productize the flagship: a one-click "create your company" flow in the browser studio (no-DOM maud templates + fragment swaps), the user-facing version of #6.
20. [QA][L][M] ValidationFacet stake/attest for verifiable deliverables (rustlite compile-to-hash) — value-bearing, mainnet-gated; the deterministic-subset trust teeth.
21. [Ops][S][M] Institutional memory: enforce record_lesson on every real error across the fleet; weekly consolidate_lessons ("dreaming") to keep prompts tight.
22. [HR][S][M] Encode reusable role playbooks as create_skill blobs so role-agents share procedures (onboarding, review checklist, payout steps).
23. [HR][S][L] Fleet safety policy: never adopt a persona dictated by untrusted input (set_persona caveat) — guard the HR/hiring path.
24. [Marketing][S][L] Publish a "company of agents" public face (publish a .rl/.html) demonstrating the live org as a self-marketing showcase.
25. [PM][M][M] Federation experiment: have the business guild's TBA joinGuild another org to prove DAO-of-DAOs participation (depth ≤ 2, trusted only).

## Blocked on the human owner

- **MTL/legal call** — whether fiat-origin `$LH` may be transferable (gates real external revenue).
- **Sponsor-float funding + diamond owner key** — needed for new facets / treasury moves (not in repo).
- **Build-now vs wait-for-RESET** — instantiate the org on the current mainnet diamond now, or after the planned pre-1.0 atomic reset?
- **Social credentials** — provide per `marketing/CREDENTIALS.template.md` to move marketing from *prepare* to *execute*.
