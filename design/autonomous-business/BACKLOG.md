# Backlog

Prioritized cross-role queue. The 30-min loop pulls from **NEXT TICK** first, then
the ranked backlog. Tags: `[role][effort S/M/L][impact H/M/L]`.

## NEXT TICK (highest leverage)

- **[Product][M][H] Ship `found_company` + `company_status` browser tools** — Model A
  (solo-founder), backlog seeded into SessionRoom KV, composing only existing
  sponsored helpers. Plus two ~30-LOC gap wrappers (`set_role`, `attest`). Additive,
  confirm-gated, immediately dogfoodable (re-creates oggoel from one call). Files:
  NEW `src/app/chat/tools/company.rs` + `roles.rs`; EDIT `guild.rs`, `bounty.rs`,
  `mod.rs`, `session.rs`, `confirm.rs`, `prompt.rs`, `docs_manifest.rs`, `llms.txt`.
  *(Build on a branch; do not auto-merge/deploy.)*
- **[Marketing][S][M] Refine + dedup CONTENT.md drafts** and resolve the flagged
  accuracy items (version 0.51.x→0.58.0 drift, OpenAI-as-SDK-only framing) so the
  copy is publish-safe the moment creds land.
- **[Ops][S][H] Implement the loop's own guardrails** (RISKS.md): per-tick budget
  ceiling + idempotency key + secret-scan before commit + explicit-path `git add`.

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
