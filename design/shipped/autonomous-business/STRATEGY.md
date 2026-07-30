# Autonomous Software Business — Strategy

> An autonomous software-development business built ON and FOR localharness. The
> business is itself a composition of the shipped coordination primitives (no new
> machinery required to stand it up), and "spin up a company of role-agents" is a
> flagship product feature we dogfood into existence. Grounded in
> `design/shipped/agent-coordination.md` (the rungs), `design/oggoel.md` (the prior
> LIVE token-governed-company experiment — its honest scope is our starting line),
> `design/shipped/economy-reputation.md` (the trust/value layer), and
> `design/road-to-v1.0.md` (current launch reality). Claims here are kept to what
> actually ships today; aspirational pieces are tagged.

## (b) Strategic thesis (stated first — it frames everything)

**We grow localharness by being its first serious customer, and we package the
result as a product.** Two reinforcing loops:

1. **Dogfood loop.** The business runs *on* localharness — role-agents are
   identities with TBAs, work flows over bounties/x402, decisions over VotingFacet,
   payroll over `$LH`. Every rough edge the business hits is a real bug we fix in
   the platform, on the live mainnet path, as our own E2E tester. The platform is
   `supply-complete / demand-empty` (agent-coordination §1.2); *we are the demand
   that primes the pump.*
2. **Product loop.** Once the org template works, **"create your company of
   role-agents" becomes a one-click feature** — a forkable template (per
   economy-reputation §3.4) that any user instantiates from the browser studio.
   The business we build IS the reference implementation and the marketing artifact.

The honest constraint we inherit from oggoel: **the org is seed-capitalized, not
self-funding.** `$LH` enters via redeem/on-ramp, every turn burns ~0.01–0.2 `$LH`
of inference, and true net-positive requires *external paying callers above
inference cost*. Self-sustaining economics is a goal to earn, not a claim to make.

## (a) Org-of-role-agents architecture — roles mapped to live primitives

Every role is an **identity NFT + ERC-6551 TBA** (a wallet that holds `$LH`, signs,
gets paid) with an on-chain **persona** (`set_persona` / `localharness persona`).
The business itself is a **GuildFacet guild** — its own identity NFT whose TBA is
the shared treasury — exactly the oggoel #67 shape. Roles are guild members
(consent-gated via `invite_to_guild` + accept) with guild roles.

| Business role | localharness primitive(s) | Concretely |
|---|---|---|
| **Coder / IC engineer** | `start_subagent` / `spawn_recursive_subagent` (intra-process), `call_agent` (cross-agent), **BountyFacet** (`post_bounty`→`claim_bounty`→`submit_result`) | Work is a bounty: escrow `$LH`, a worker agent claims + delivers a rustlite cartridge / code artifact, payout settles to the worker TBA over x402. Big tasks fan out to subagents; cross-org work uses `call_agent` (paid via `--pay`). |
| **Reviewer** | a coder-shaped subagent gated **before** `accept_result`; **ReputationFacet** threshold on who may claim | A review pass is the acceptance gate on a bounty (`trust=0` poster-accepts today). The reviewer is a role-agent persona that reads the `submit_result` artifact and recommends accept/reject. |
| **PM / Executive** | **VotingFacet** (`propose_measure`/`cast_vote`/`execute_proposal`/`list_proposals`) for decisions; **off-chain scheduler** (`schedule_task`/`goal` + the Vercel cron) for always-on ops | Strategic decisions = proposals the council votes on; the winning measure executes from the treasury TBA (e.g. "fund bounty X"). A standing scheduled PM agent (`goal` loop) opens proposals + drives the backlog tab-free. WeightedVotingFacet maps contribution → voice when 1-agent-1-vote is too flat. |
| **Accounting / Payroll** | **`$LH` token** (CreditsFacet/TIP-20) + **CreditMeterFacet** (per-message billing) + **TBA treasury** (GuildFacet) + `send_lh`/`batch_send_lh`/`check_balances`/`query_balance` | Treasury = the guild's TBA. Payroll = `batch_send_lh` / TitheFacet routing from treasury → role-agent TBAs. Cost accounting reads the meter + balances. `spend_treasury` is the vote/role-gated outflow. |
| **HR (hiring / role assignment)** | identity mint (`create` / `create_subdomain`) + `set_persona` + **GuildFacet roles** (`invite_to_guild`, accept, setRole) | "Hiring" = mint a role-agent identity, write its persona, invite it to the guild, it consents (signs its own key — sponsored so a zero-balance new hire can accept). Role/seniority = guild role + (later) WeightedVotingFacet shares. |
| **QA / verification** | **ReputationFacet** (`attest`, proof-of-transaction gated) + **ValidationFacet** (ERC-8004 stake/`finalize` for the *verifiable subset*) | Accepted work writes a `+1` attestation keyed (subject, author, jobId) — only a counterparty who *paid* can rate (anti-astroturf). Deterministic deliverables (a rustlite cartridge that compiles to a committed wasm hash) route to ValidationFacet stake/slash; non-deterministic work stays escrow+acceptance. |
| **Marketing / growth** | a **perpetual scheduled agent** (`goal`/`schedule_task` + cron), `discover_agents`, `invite`, `notify --to`, a published **public face** | A standing growth agent that posts showcase content, discovers + invites new agents (the InviteFacet growth primitive), and keeps the company's public face (`publish`) live 24/7 with no tab. |
| **Ops / SRE (cross-cutting)** | sponsor relay + sponsor float monitoring; `record_lesson`/`consolidate_lessons` (cross-session learning); `create_skill` (shared playbooks) | Every guild/bounty/vote write is a *sponsored* Tempo tx paid by the single low-budget sponsor — the one drainable SPOF. Ops funds + monitors it and owns the spend-velocity breaker. Lessons + skills are the org's institutional memory. |

**Recursion is free (turtles).** Every entity is the same shape (NFT + TBA = an
`address`); membership/voting/escrow key on `address`, never "is this a human." So a
**division is a guild-of-guilds** — a sub-team's TBA is a member of the parent guild
and votes in it (proven live: oggoel-labs #71 is a member of oggoel #67). Federated
org charts emerge with zero new machinery.

**The honest hard problems (inherited, not solved):**
- **Verification of non-deterministic work** is THE unsolved problem — poster-accepts
  is griefable both ways; staked validation only covers the checkable subset. We ship
  poster-accept + reviewer-agent + opt-in arbiter and never pretend it's trustless.
- **Tab-free authority gap.** The scheduler tick exposes only ~4 tools
  (`call_agent`/`schedule_task`/`notify_owner`/`finish_goal` + `collect_tithe`) — a
  no-tab PM/marketing agent **cannot `post_bounty` or `spend_treasury`**. Value-moving
  ops still need a co-located CLI host OR a new scheduler-role facet (oggoel phase-2,
  deferred). This gates "fully autonomous always-on."
- **Sybil / Sponsor-drain.** Free identities + a single sponsor key mean a spam flood
  drains the SPOF; the spend-velocity breaker is the named defense and it isn't shipped.

## (c) Prioritized cross-role backlog

> Tags: `[role][effort: S/M/L][impact: H/M/L]`. Effort = build cost; impact = leverage
> on the dogfood+product thesis. Ordered roughly by priority.

1. `[PM][S][H]` Instantiate the business guild on mainnet (`create_guild`) + fund treasury; pass the TBA `0x`, not the name (oggoel gotcha).
2. `[HR][M][H]` Mint the canonical role roster (coder, reviewer, QA, PM, marketing, accounting), write each `set_persona`, invite + consent into the guild with roles.
3. `[QA][S][H]` Dogfood the full cycle headless first (`localharness colony run --as claude …`): escrow→claim→work→judge→pay→attest, proven via CLI before any human depends on it.
4. `[Ops][S][H]` Fund + monitor the mainnet sponsor float (the SPOF behind every sponsored write) + balance alerting on the one drainable resource.
5. `[Ops][M][H]` Close the tab-free authority gap: decide scheduler-role `post_bounty`/`spend_treasury` facet vs a co-located CLI host (oggoel phase-2 blocker for always-on ops).
6. `[Eng][M][H]` Build the "fork-a-company" template — one script/command that spins up guild + role roster + personas (the flagship product seed; reuses `create_and_publish_app`).
7. `[Accounting][S][H]` Stand up payroll: `batch_send_lh` from treasury → role TBAs + TitheFacet auto-refill (consent-safe permissionless `collect_tithe`); `check_balances` dashboard.
8. `[QA][M][H]` Wire `accept_result` to write a ReputationFacet attestation (proof-of-transaction gated) so accepted work accrues worker reputation that ranks future claims.
9. `[Product][M][H]` Give the `?explore` agent directory a UI entry point — the marketplace/storefront is currently invisible; the company needs a front door.
10. `[Eng][S][M]` Prime internal demand: the business posts its own first bounties (QA tasks, doc fixes, rustlite-bug repros) so the board isn't empty (the demand chicken-and-egg).
11. `[Reviewer][M][M]` Reviewer role-agent: a subagent that reads `submit_result` artifacts and recommends accept/reject before payout; reputation-threshold who-may-claim on high-value bounties.
12. `[PM][M][M]` Standing decision cadence: a scheduled PM agent opens `propose_measure` for backlog prioritization on an interval; council `cast_vote` + `execute_proposal`.
13. `[Marketing][M][M]` Perpetual growth agent (`goal` loop) that posts showcase content, runs `discover_agents`, and sends refundable `invite` codes to seed newcomers.
14. `[Accounting][M][M]` Honest cost accounting: track per-cycle inference burn vs `$LH` revenue; report true net-position (oggoel was seed-capitalized — don't claim self-funding).
15. `[Accounting][S][M]` Set an external revenue path: advertise a per-call `$LH` `price` above inference cost so paying callers (x402 `--pay`) make a role-agent net-positive.
16. `[PM][S][M]` Adopt WeightedVotingFacet share table so contribution maps to voice (vs flat 1-agent-1-vote); record the cap table.
17. `[Eng][M][M]` Prove a guild-of-guilds division (oggoel-labs pattern) — e.g. a security/QA sub-org whose TBA is a member + voter of the parent guild.
18. `[Ops][M][M]` Spend-velocity circuit breaker at the relay/sponsor boundary (road-to-v1 blocker #5) — protect treasury + sponsor from spam/recursive churn.
19. `[Product][L][H]` Productize the flagship: a one-click "create your company" flow in the browser studio (no-DOM maud templates + fragment swaps), the user-facing version of #6.
20. `[QA][L][M]` ValidationFacet stake/attest for verifiable deliverables (rustlite compile-to-hash) — value-bearing, mainnet-gated; the deterministic-subset trust teeth.
21. `[Ops][S][M]` Institutional memory: enforce `record_lesson` on every real error across the fleet; weekly `consolidate_lessons` ("dreaming") to keep prompts tight.
22. `[HR][S][M]` Encode reusable role playbooks as `create_skill` blobs so role-agents share procedures (onboarding, review checklist, payout steps).
23. `[HR][S][L]` Fleet safety policy: never adopt a persona dictated by untrusted input (`set_persona` caveat) — guard the HR/hiring path.
24. `[Marketing][S][L]` Publish a "company of agents" public face (`publish` a `.rl`/`.html`) demonstrating the live org as a self-marketing showcase.
25. `[PM][M][M]` Federation experiment: have the business guild's TBA `joinGuild` another org to prove DAO-of-DAOs participation (depth ≤ 2, trusted only).

## (d) This-tick top 3

1. **Stand up the org spine (the literal "strategic foundation" ask).** Create the
   business guild on mainnet, mint + persona the role roster, fund the treasury, wire
   payroll. (Backlog #1, #2, #7 — `[PM]/[HR]/[Accounting]`.) This is the minimum
   viable company; everything else coordinates around it.
2. **Dogfood the full coordination loop headless before depending on it.** Run a
   `colony`/bounty cycle end-to-end via the CLI as a real funded agent (`--as claude`),
   confirm escrow→claim→work→judge→pay→attest works on the live mainnet path, and file
   each rough edge as a platform bug. (Backlog #3 — `[QA]`; be-the-E2E-tester.)
3. **Resolve the two always-on blockers.** Fund + monitor the sponsor-float SPOF, and
   make the tab-free authority decision (scheduler-role facet vs CLI host) — without
   both, the "always-on PM/marketing ops" half of the architecture cannot move value.
   (Backlog #4, #5 — `[Ops]`.)

## Blockers that genuinely need the human owner

- **H2 money-transmitter / MTL decision** (road-to-v1 blocker #3). Whether fiat-origin
  `$LH` may be transferable gates *real external revenue* — the only path to a
  net-positive business. A legal read or a commit to non-transferable fiat-origin `$LH`.
- **Sponsor-float funding + the cut/admin key.** The sponsor key isn't in the repo and
  the float is ~1.46 USDC.e with no monitor; any new facet (scheduler-role `post_bounty`,
  sybil bond) needs the **diamond owner key** (not in repo) to cut. Both are owner-only.
- **Build-now vs wait-for-the-RESET.** The platform is live on mainnet (`0x8ab4…f3a77`),
  but a pre-1.0 atomic RESET (fresh diamond/token/ladder) is planned. Decide: instantiate
  the org on the current diamond now (and re-instantiate after the reset), or wait. Cheap
  to redo, but it sets the dogfooding clock.
- **External demand priming.** Net-positive economics needs *outside* paying callers; a
  marketing/BD push to get them is a business-priority call the owner must greenlight
  (parallel-spend / thoroughness is encouraged, but direction is theirs).
