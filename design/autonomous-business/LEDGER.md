# Ledger

Append-only progress log. One entry per loop tick. Newest at top.

---

## Tick 13 — 2026-06-30T14:00Z
<!-- tick-window: 2026-06-30T1400Z -->

**Goal:** the ONE remaining high-value consolidating deliverable, then downshift.
Minimal tick (1 agent, no code) — per the tick-12 stewardship downshift.

**Shipped (no code; secret-scan clean):**
- **MARKETING — `LAUNCH-RUNBOOK.md`:** the turnkey go-live runbook. Consolidates CALENDAR
  + both campaigns (`whoami`→`git log`) into ONE D0→D42 fire-sequence; an 8-criteria
  GO/NO-GO gate; per-step disclosure-line + native-label matrix; ABORT/rollback triggers;
  a "first 5 actions when creds land" quick-start. De-duped 3 cross-file asset collisions.
  The moment creds land, launch is mechanical.

**State — both halves of the mission are built out and waiting:**
- Product: feature-complete preview surface (CLI found→…→forecast, 5 pure cores, example).
- Marketing: 18 assets — 2 campaigns, audience intel, SEO, calendar, press/outreach,
  visual briefs, + this runbook.
Everything substantive left is OWNER-GATED (live execution, real posting).

**Loop posture: DOWNSHIFTED.** Per the tick-12 note, ticks are now minimal — no more
multi-agent fan-outs manufacturing low-marginal-value docs. Future ticks will do at most a
tiny polish/refresh or effectively idle, until the owner engages (DECISIONS.md) or pauses
(CronDelete ca03e5d2).

**Human-blocked → DECISIONS.md / STATUS.md** (8 decisions await answers).

---

## Tick 12 — 2026-06-30T13:30Z
<!-- tick-window: 2026-06-30T1330Z -->

**Goal:** perpetual marketing (the mission's emphasis) now the product is complete —
market intelligence + a 2nd campaign + README polish. Lean tick, no code/product churn.

**Shipped (no code — pure marketing/docs; secret-scan clean):**
- **MARKETING — `AUDIENCE-INTEL.md` (sourced 2026 web sweep):** real market intelligence.
  Macro reframe: the agent-TOKEN cycle broke (ai16z ~-99.9% + a fraud class action) while
  agent PAYMENTS got legitimized (x402 → Linux Foundation; Tempo mainnet; MCP ubiquity) →
  lead with payments-rail + identity + no-server runtime, NEVER "token". Best channel per
  segment: This Week in Rust / the x402 ecosystem / the MCP ecosystem. Sharpest wedge: the
  same crate is BOTH the SDK and the deployed self-owned agent (no rival collapses those).
- **MARKETING — `CAMPAIGN-02.md` (the `git log` campaign):** the trust second-act to
  `whoami` — radical transparency as the moat ("instead of a demo, here's our git log").
  Hero = the real public branch; the quotable receipt = the guardrail line that held every
  tick (`0 writes · 0 $LH · 0 posts · 0 merges`). Turns AI-disclosure + open-self-funding
  into the creative spine.
- **POLISH — `README.md`:** points to STATUS.md/DECISIONS.md, reflects the complete surface.

**State:** product complete; marketing library = 17 assets incl. 2 full campaigns + market
intel. Loop is now in perpetual-marketing mode (the mission's standing directive) +
awaiting the owner's DECISIONS.md answers for the live leaps.

**Next tick:** more perpetual marketing/creative (intel refresh, a 3rd creative angle, or
asset polish) — or owner answers. Owner-gated leaps still wait on DECISIONS.md.

**Human-blocked → DECISIONS.md / STATUS.md** (8 decisions await answers).

---

## Tick 11 — 2026-06-30T13:00Z
<!-- tick-window: 2026-06-30T1300Z -->

**Goal:** finish the CLI surface (forecast); a one-page owner STATUS; a novel creative
campaign (the mission's marketing emphasis). Leaner, marketing-weighted tick.
(Last tick flagged diminishing returns + recommended owner review/unblock — no answer
yet, so the standing every-30-min directive holds; ticks now lean toward marketing.)

**Shipped (verified — re-ran CLI + drift + check + secret-scan):**
- **CODE — `company forecast` CLI:** read-only — builds the live board (ChainReader) and
  runs `simulation::simulate` forward, printing the per-cycle treasury/throughput
  projection + runway verdict ("survives N" / "RAN OUT at cycle k") + honest self-funding
  read, under a "FORECAST (model/preview only)" banner. 45 CLI tests (4 new). No broadcast.
  The CLI company surface is now COMPLETE: found→status→plan→payroll→books→day→forecast.
- **EXEC — `STATUS.md`:** 11 ticks on one scannable page (capabilities / marketing
  inventory / quality gates / gated frontier → DECISIONS.md). Updated to include forecast.
- **MARKETING — `CAMPAIGN-01.md` (the `whoami` campaign):** a novel coordinated launch
  with a real through-line — *every other agent answers `whoami` with a username on
  someone else's server; a localharness agent answers with a name it owns on-chain* —
  hero origin-film at a claimable `whoami.localharness.xyz`, AI-disclosure-as-punchline,
  a 2-week beat-by-beat rollout re-sequencing the asset library. Accuracy-guarded.

**Next tick:** non-blocked work is now genuinely narrow (the CLI/cores are complete). Lean
to perpetual marketing (campaign assets, audience/competitive notes) + small polish, OR
await owner answers. Owner-gated leaps (live execution, real posting) wait on DECISIONS.md.

**Human-blocked → DECISIONS.md / STATUS.md** (8 decisions await answers).

---

## Tick 10 — 2026-06-30T12:30Z
<!-- tick-window: 2026-06-30T1230Z -->

**Goal:** the operator's daily glance (`company day`) + lock the CLI surface; a forward
forecast core; organic-discovery copy. Three agents parallel — non-blocked.

**Shipped (verified — re-ran CLI + simulation + wasm + drift + license + secret-scan):**
- **CODE — `company day` CLI:** read-only one-shot daily report composing status + plan +
  payroll + books (does every read ONCE; reuses the pure formatters — extracted
  `format_status`/`format_payroll` to avoid duplication). Under a "PREVIEW ONLY" banner;
  no broadcast. Plus a 42-assertion golden integration test locking the whole `company`
  surface against regression. 41 CLI tests.
- **CODE — `src/simulation.rs` (pure forecast):** `simulate(State, SimConfig) -> Forecast`
  runs N cycles forward (deliver→step→book revenue/cost into an accounting::Ledger),
  projecting per-cycle treasury, throughput, and `ran_out_at` runway exhaustion. Honest
  documented `submit_quality` modeling assumption. 10 tests, native+wasm clean.
- **MARKETING — `SEO-LANDING.md`:** FAQPage-structured organic/answer-engine copy
  (answer-first, 5 target-query mappings, JSON-LD note, honest limitations). Verified the
  asserted **Apache-2.0** license against `Cargo.toml` — accurate.

**State:** the autonomous-company is now a complete, inspectable, forward-looking PREVIEW
product — found → status → plan → payroll → books → day (snapshot) + simulate (forecast),
all read-only, plus a runnable example, all 7 roles, and a stocked marketing library.

**Next tick (non-blocked, narrowing):** a `company forecast` CLI over `simulation`; a
"state of the business" capabilities summary for the owner; marketing polish. The high-
value frontier is now the DECISIONS.md answers (live execution / real posting).

**Human-blocked → DECISIONS.md** (unchanged; 8 decisions await answers).

---

## Tick 9 — 2026-06-30T12:00Z
<!-- tick-window: 2026-06-30T1200Z -->

**Goal:** integrate the accounting/hiring cores into the shells; ship a runnable
example; press kit + outreach. Three agents parallel — non-blocked.

**Shipped (verified — re-ran work_cycle + CLI + example run + wasm + secret-scan):**
- **CODE — `hiring` is now the canonical ranker:** `work_cycle::assign_next_task`
  delegates worker selection to `hiring::best_candidate` (behavior-preserving — same
  rule, ONE impl; all 20 work_cycle tests green unchanged). No public API change.
- **CODE — `company books` CLI:** read-only — reads the on-chain treasury, builds an
  `accounting::Ledger` from estimate flags (`--period-cost/--period-revenue/--seed/
  --calls`), prints net position / runway / breakeven / self-funding / relies-on-seed,
  clearly labeled "ESTIMATE — only treasury is on-chain". 40 CLI tests (4 new). No writes.
- **DOCS — `examples/autonomous_company.rs` (RUNNABLE):** a pure end-to-end demo —
  HR staffs the high-rep coder, work_cycle previews 7 actions (accept→payout→attest→
  post→assign), accounting prints the HONEST read (net −23, relies_on_seed, "NOT yet
  self-funding"). `cargo run --example autonomous_company` succeeds.
- **MARKETING — `PRESS-KIT.md`** (boilerplate + 8 facts + unapproved founder-quote
  placeholder) + **`OUTREACH-TEMPLATES.md`** (3 cold-email templates + CAN-SPAM/GDPR
  notes + DO-NOT list).

**Next tick (non-blocked):** wire accounting into a per-cycle report in the planner
output; an end-to-end CLI "dry-run a full company day" demo command; more marketing
(blog/SEO); tests/polish. Owner-gated items wait on DECISIONS.md.

**Human-blocked → DECISIONS.md** (unchanged; 8 decisions await answers).

---

## Tick 8 — 2026-06-30T11:30Z
<!-- tick-window: 2026-06-30T1130Z -->

**Goal:** complete pure-core coverage for the Accounting + HR roles; prep visual
marketing; contributor onboarding. Three agents parallel — non-blocked.

**Shipped (verified — re-ran lib tests + wasm guard + secret-scan):**
- **CODE — `src/accounting.rs` (pure):** company economics, HONEST seed-vs-earned —
  `Ledger{treasury,period_costs,period_revenue,seed_capital}`, `net_position` (seed
  EXCLUDED, signed), `is_self_funding`/`relies_on_seed`, `runway_cycles`,
  `breakeven_price`, `margin_per_call`, saturating math. 15 tests.
- **CODE — `src/hiring.rs` (pure):** role-fit — `score_candidate`/`rank_candidates`/
  `best_candidate` over `work_cycle::Role`, ranked identically to `assign_next_task`
  (rep desc, id asc) so HR + allocation agree by construction. 11 tests.
  - PASS accounting (15) + hiring (11) · native check · wasm guard.
  - With these, all 7 business roles now have dedicated logic: coder/reviewer
    (work_cycle), PM/exec (decisions/voting), accounting + HR (new cores), marketing.
- **DOCS — `CONTRIBUTING.md`:** the pure-core-vs-IO-shell rule, layer map, how to add a
  role/capability, the verify gates, loop guardrails.
- **MARKETING — `VISUAL-BRIEFS.md`:** 6 IG/TikTok short-form video concepts (hook + shot
  list + footage needed + disclosure), Tier-3 prepared; shoot-first = V3 (slither demo).

**Next tick (non-blocked):** wire `hiring`/`accounting` into the planner (hiring →
`work_cycle_runtime` worker ranking; an `company books` CLI preview using accounting);
SDK example file; more marketing. Owner-gated items wait on DECISIONS.md.

**Human-blocked → DECISIONS.md** (unchanged; 8 decisions await answers).

---

## Tick 7 — 2026-06-30T11:00Z
<!-- tick-window: 2026-06-30T1100Z -->

**Goal:** wire the planning shell to a real (read-only) data source via the CLI; add
accounting/HR preview; SDK doc; more marketing. Three agents parallel — non-blocked.

**Shipped (verified — re-ran wallet bin + drift + secret-scan):**
- **CODE — `company plan` + `company payroll` (`bin/company.rs` +main.rs):** READ-ONLY
  previews, no broadcast. `company plan <guild|name>` builds a registry-backed
  `ChainReader` (members→role+rep workers, treasury, open bounties→Posted tasks) and
  dry-runs `work_cycle_runtime::plan_cycle`, printing the planned Actions under a
  "PREVIEW ONLY — nothing executed or broadcast" banner. `company payroll <guild>
  [--fraction] [--by-rep]` prints treasury + per-role TBA balances + a suggested even/
  reputation-weighted split (NO transfers). 36 tests (13 new), clippy + drift clean.
  Honest TODO: on-chain bounties carry no role/quality, so they map to Posted+Coder+
  min-quality-3; claimed/submitted bounties skipped (no fabricated verdict).
- **DOCS — `SDK-QUICKSTART.md`:** "use localharness as a library" — Mock-backend hello
  agent + real-backend swap + `ClosureTool`, every API name source-verified (funnel asset).
- **MARKETING — `X-POSTS-BATCH-2.md`:** 10 new standalone posts (all ≤280), evergreen-pool
  entry in READY-QUEUE + 4 slotted into CALENDAR. Caught+fixed a "no solc" slip.

**Now true: the full DRY path is CLI-runnable** — found → status → plan → payroll, all
read/preview-only. The single remaining gap to a live self-operating company is the
greenlight-gated Action executor.

**Next tick (non-blocked):** accounting cost-accounting core (per-cycle burn vs revenue,
pure+tested); a HR roster/role-assignment preview; SDK example file; more marketing.
Owner-gated items wait on DECISIONS.md.

**Human-blocked → DECISIONS.md** (unchanged; 8 decisions await answers).

---

## Tick 6 — 2026-06-30T10:30Z
<!-- tick-window: 2026-06-30T1030Z -->

**Goal:** connect the work-cycle core to a (preview-only) runtime; fix the CLI quirk;
architecture doc; marketing #3. Four agents parallel — all non-owner-blocked.

**Shipped (verified — re-ran native lib + wasm guard + wallet bin + secret-scan):**
- **CODE — `work_cycle_runtime.rs` (pure planning shell, +lib.rs):** `Reader` trait
  (tasks/workers/treasury_balance — no registry dep) → `plan_cycle(reader, max_steps)
  -> CyclePlan{state_before, actions, state_after, summary}`. Builds State from reads,
  runs `work_cycle::step` to quiescence, returns the planned `Action`s — **PREVIEW ONLY,
  never executes** (summary literally prefixed "PLAN (preview only — nothing executed)";
  treasury debit is a pure projection). MockReader + 7 tests.
  - PASS `cargo test --lib work_cycle` (20: 13 core + 7 runtime) · native + wasm + clippy clean.
- **CODE — CLI `--roles` fix (`bin/company.rs`):** `resolve_roles(Option<&[String]>)` —
  absent flag → 7 defaults; present-but-empty → exit-2 error (no more silent fallback).
  24 tests pass.
- **DOCS — `ARCHITECTURE.md`:** full map w/ a pure-core ↔ I/O-shell boundary diagram +
  honest per-layer status table + the Action→`registry::*_sponsored` executor mapping.
- **MARKETING — `DEVTO-ARTICLE-3.md`** (rustlite compiler, source-pinned) + `CALENDAR.md`
  (3-week day×platform schedule keyed to READY-QUEUE ids, spacing rules honored).

**Next tick (non-blocked):** the deferred executor as a PREVIEW-only wiring (real Reader
impl over registry reads + a `plan`/dry-run CLI subcommand that PRINTS the CyclePlan; no
broadcast until greenlit); accounting/HR role tooling (treasury/payroll preview, role
roster); more docs/marketing. Owner-gated items still wait on DECISIONS.md.

**Human-blocked → DECISIONS.md** (unchanged; 8 decisions await answers).

---

## Tick 5 — 2026-06-30T10:00Z
<!-- tick-window: 2026-06-30T1000Z -->

**Goal:** non-owner-blocked progress while DECISIONS.md awaits answers — the
"company does work" core, CLI hardening, docs, marketing. Four agents parallel.

**Shipped (verified — re-ran native lib + wasm guard + wallet bin + secret-scan):**
- **CODE — pure `work_cycle` core (`src/work_cycle.rs`, +lib.rs):** models one
  claim→work→judge→pay→attest cycle as DATA — pure types + `assign_next_task` /
  `evaluate_result` / `compute_payout` / `step(State)->(State, Vec<Action>)`; `Action`
  variants doc-mapped to real `registry` bounty/reputation calls (I/O stays in a future
  wiring shell, decisions in the pure core). Mirrors keeper.rs/lessons.rs style, zero deps.
  - PASS `cargo test --lib work_cycle` (13) · PASS native check · PASS wasm guard.
- **CODE — CLI hardening (`src/bin/localharness/company.rs`):** +17 tests (6→23) —
  preview golden map + treasury math, amount-parse rejections, malformed `--roles`,
  long-input clamps, `company status` parsing. Found 2 BENIGN quirks (no bug): empty-ish
  `--roles` silently falls back to defaults; lone `.` = 0 wei ("skip"). PASS (23/23).
- **DOCS:** `FOUND-A-COMPANY.md` — user quickstart (browser tool + CLI, preview-vs-
  `--confirm`, `--dev`→testnet, treasury math, honest "not yet" notes).
- **MARKETING:** LinkedIn long-form (#7) + founder-story X thread (#8) in READY-QUEUE,
  accuracy-guarded (found_company framed shipped; autonomous operation in-progress;
  x402 "built/design-level" not mainnet-live).

**Next tick (still non-blocked):** wire `work_cycle` to a runtime shell that builds
`State` from on-chain reads + maps `Action`s to sponsored calls (no auto-broadcast —
preview/dry-run only until owner greenlights); stricter `--roles` validation; more docs/
marketing. Owner-gated items still wait on DECISIONS.md.

**Human-blocked → DECISIONS.md** (unchanged; 8 decisions await answers).

---

## Tick 4 — 2026-06-30T09:30Z
<!-- tick-window: 2026-06-30T0930Z -->

**Goal:** CLI twin of `found_company` (enable headless dogfood); consolidate owner
decisions; expand marketing. Three role-agents (coder, executive, marketing) parallel.

**Shipped (verified — re-ran native check + drift + secret-scan):**
- **CODE (real, compiles, no tx broadcast):** `company` CLI command family in
  `src/bin/localharness/company.rs` — `company found …` (same sponsored-helper pipeline
  as the browser tool; LH_CHAIN routing inherited; **`--confirm`-less = broadcast-free
  PREVIEW**, no signer/RPC) + `company status <guild|name>` (read-only). Wired dispatch +
  USAGE + `CLI_COMMANDS`; gen-docs regenerated (skill.md only). 6 new unit tests pass.
  - PASS `cargo check --features wallet` · PASS `no_doc_drift` · NO on-chain tx run.
  - Honest deferral vs browser: CLI prints the mission in the manifest but doesn't
    create the SessionRoom KV backlog room (createRoom ~1.3M gas) — follow-up.
- **EXEC:** `DECISIONS.md` — consolidated owner-blocked decision brief (TL;DR table +
  reply menu). Key coupling: answering "wait for the reset" resolves build-vs-reset +
  testnet-dogfood + sponsor-float + transferable-`$LH` together. Recs: wait-for-reset;
  YES to a scoped testnet-only dogfood exception; seed Tier1-2 creds; fund+monitor float;
  apply address relabel; loop opens DRAFT PRs to main (never auto-merge).
- **MARKETING:** 2nd dev.to (`DEVTO-ARTICLE-2.md`, x402+EIP-6551 angle, every figure
  source-pinned) + r/ethdev human-gated draft (distinct body). Caught + softened: x402
  settlement is NOT mainnet-live (testnet-only per `x402.rs`).

**Next tick:** depends on owner answers in DECISIONS.md. If testnet-dogfood greenlit →
run `company found … --dev --confirm` on Moderato + `company status` it back. Else →
CLI SessionRoom backlog room, or Phase-2 `run_work_cycle` (hoist colony.rs cores).

**Human-blocked → see `DECISIONS.md`** (one brief, 8 decisions, recommendations).

---

## Tick 3 — 2026-06-30T09:00Z
<!-- tick-window: 2026-06-30T0900Z -->

**Goal:** ship the `found_company` WRITE half; settle the address drift; expand
marketing. Three role-agents (coder, address-investigator, marketing) ran in parallel.

**Shipped (verified — re-ran wasm check + drift test + secret-scan myself):**
- **CODE (real, compiles):** `found_company(name, mission, roles?, seed_treasury_lh?,
  prefund_each_lh?, confirmation)` — Model-A solo-founder pipeline composing existing
  sponsored helpers (zero new on-chain surface): create_guild → optional treasury seed →
  batch-create N role subdomains in one tx → per-role on-chain persona (+ optional
  prefund) in one sponsored tx → seed mission/backlog into SessionRoom KV. Returns a
  manifest `company_status` reads back. CONFIRM_GATED + allowlist-gated; both backends.
  NEW helper `room::set_shared_state`. Files: `company.rs`, `room.rs`, `session.rs`,
  `confirm_guard.rs`, `prompt.rs`, `docs_manifest.rs`, `web/{llms.txt,skill.md}`.
  - PASS wasm guard · PASS `no_doc_drift` · gen-docs changed NO chain address.
  - Honest design note: Model A skips an invite step (founder is already sole guild
    Admin → inviteToGuild reverts AlreadyMember); manifest records each role TBA so a
    later Model-B (distinct voters) cut can seat them.
- **ADDRESS DRIFT — RESOLVED (not a bug):** on-chain proof — `0x8ab4f3a5…f3a77` is the
  live MAINNET diamond (chain 4217; owner()=0x313b…EF1e, 36 facets); `0x6c31c01e…` is
  the Moderato TESTNET diamond (chain 42431). `registry::chain.rs`, `docs_manifest`,
  `llms.txt`, `skill.md`, `README` all already correct. Only `CLAUDE.md`/`AGENTS.md`
  "Canonical addresses" table is the *testnet* set under an unqualified header.
  See `ADDRESS-DRIFT.md`. NOT auto-fixed — user-curated core spec + ties to the pending
  mainnet reset; flagged to owner.
- **MARKETING:** READY-QUEUE expanded — 6 AUTO assets (incl. a new build-in-public X
  thread, `found_company` honestly framed as in-progress) + 2 HUMAN-GATED (Show HN +
  Reddit r/rust) with their exact ToS caveats. No address pinned; accuracy re-verified.

**Next tick:** CLI twin of `found_company` (headless founding) + dogfood the full
create→read cycle on testnet; or Phase-2 `run_work_cycle` (hoist colony.rs cores).

**Human-blocked (unchanged):** social credentials; build-now-vs-RESET (now sharper —
see ADDRESS-DRIFT.md); MTL/legal on transferable `$LH`; sponsor float + owner key.

---

## Tick 2 — 2026-06-30T08:55Z
<!-- tick-window: 2026-06-30T0830Z -->

**Goal:** ship the first *compiling* code slice of `found_company`; harden the loop;
advance marketing. Three role-agents (coder, marketing, ops) ran in parallel.

**Shipped (verified — re-ran the gates myself):**
- **CODE (real, compiles):** `company_status` (read-only: members + roles + treasury
  `$LH` balance), `set_role`, `attest` browser agent tools — composing only existing
  `registry::*_sponsored` helpers, zero new on-chain surface. Write tools are
  confirm-gated; registered in both backend branches; `AGENT_TOOLS` + GEN docs updated.
  NEW `src/app/chat/tools/company.rs`; edits to `mod/guild/bounty/session/confirm_guard/
  prompt.rs` + `docs_manifest.rs` + `web/{llms.txt,skill.md}`.
  - PASS `cargo check --no-default-features --features browser-app --target wasm32...`
  - PASS `cargo test --features wallet no_doc_drift`
- **OPS:** `LOOP-PROTOCOL.md` (enforceable per-tick checklist) + `loop-secret-scan.sh`
  (commit-gate scanner). Budget ceilings: 0 on-chain writes / 0 `$LH` / 0 live posts,
  <=6 agents, $5/tick + $40/day. Idempotency = UTC half-hour `tick-window` stamp (this
  entry carries one) written last, so a double-fire no-ops but a crash stays re-runnable.
- **MARKETING:** CONTENT.md accuracy pass (version -> **0.58.0** verified; OpenAI/Gemma
  locked to SDK-only, never live in-app models per `src/app/model.rs`) + new
  `DEVTO-ARTICLE.md` (Tier-1 long-form) + `READY-QUEUE.md` (5 publish-safe first-party
  assets w/ FTC+EU-AI-Act disclosure).

**Findings carried forward:**
- **Address drift:** root `CLAUDE.md` diamond `0x6c31c01e...` vs `web/llms.txt`
  `0x8ab4f3a5...` DISAGREE — backlogged (no marketing asset pins an address).
- Adopted the ops idempotency window-stamp into the ledger format as of this tick.

**Next tick:** `found_company` WRITE half (compose create_guild + role subdomains +
invites + backlog seed, confirm+allowlist gated); dogfood `company_status` headless;
fix the diamond-address drift.

**Human-blocked (unchanged):** MTL/legal on transferable `$LH`; sponsor float + owner
key; build-now-vs-RESET; social credentials.

---

## Tick 1 — 2026-06-30 (bootstrap)

**Goal:** stand up the autonomous-business workspace + first deliverables across all
roles. Six role-agents ran in parallel.

**Shipped:**
- `STRATEGY.md` — org = a GuildFacet guild (identity NFT + TBA treasury) with
  role-agents as consent-gated members (the proven oggoel shape). Full role→primitive
  mapping, the two-loop dogfood thesis, a 25-item backlog, honest blockers.
- `COMPANY-FEATURE.md` + `roles/{coder,reviewer,pm,executive,accounting,hr,marketing}.md`
  — design for `found_company` (one call replaces oggoel's 9 manual steps; **zero new
  on-chain surface for Phase 1**) + 7 `set_persona`-ready role personas.
- `marketing/BRAND.md` — positioning, voice (technical/cypherpunk/anti-bloat), 3
  audiences, 5 fair differentiators vs LangGraph/CrewAI/Vercel/OpenAI/Eliza, 8 taglines.
- `marketing/CONTENT.md` — 10 X posts, a 10-post thread, r/rust + r/ethdev posts, a
  Show HN, LinkedIn, a 2-week calendar; parked features excluded; claims flagged.
- `marketing/GROWTH.md` + `CREDENTIALS.template.md` — channel playbook
  (`[AUTO]/[APPROVE]/[NEVER]`), north-star KPI = on-chain identity claims, exact creds
  checklist + secure (gitignored) delivery, honest automatable/not breakdown.
- `RISKS.md` — guardrails: social posting never a closed loop; no auto-merge/deploy/
  release/cut; confirm-gate intact; no `git add -A`; budget ceilings + idempotency;
  AI-disclosure on every draft (FTC + EU AI Act Art. 50).

**Findings worth carrying forward:**
- Economy ladder (guild/bounty/voting/reputation) is confirmed **cut & live on
  mainnet** — the org can be stood up on the real chain today (corrects a stale note).
- Only **2 code gaps** block the `found_company` slice: no browser `set_role` / `attest`
  tool (each ~30 LOC over shipped `registry::*_sponsored` helpers).
- **Doc drift:** root `CLAUDE.md` header says `0.51.x`; generated source-of-truth is
  `0.58.0`. Backlog a fix.

**Next tick:** see `BACKLOG.md` → NEXT TICK (ship `found_company`/`company_status`
design-to-code slice on a branch; refine CONTENT accuracy; implement loop guardrails).

**Human-blocked:** MTL/legal on transferable `$LH`; sponsor float + owner key;
build-now-vs-RESET; social credentials.
