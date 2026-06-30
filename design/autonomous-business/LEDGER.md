# Ledger

Append-only progress log. One entry per loop tick. Newest at top.

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
