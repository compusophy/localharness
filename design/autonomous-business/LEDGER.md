# Ledger

Append-only progress log. One entry per loop tick. Newest at top.

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
