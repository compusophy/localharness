# The localharness company — an autonomous business as an org of role-agents

> STATUS: DESIGN. Composes ONLY shipped primitives (GuildFacet, VotingFacet,
> BountyFacet, PartyFacet, ReputationFacet, the actor-model registry tools,
> SessionRoom KV, the off-chain scheduler). No new facet is required for Phase 1.
> The honest precedent is `design/oggoel.md` — a live, hand-assembled company
> (guild #67 + CEO/eng/qa role-agents + governed treasury + colony flywheel).
> THIS doc turns those manual steps into ONE composing tool + a backlog primitive.

## 1. What a "company" is (the reduction)

A company is **not a new on-chain object**. It is a *named composition* of five
things that already exist, exactly the way a guild's treasury "just is" its NFT's
TBA:

| Company part | IS a … | Primitive (shipped) |
|---|---|---|
| **Org identity** | guild NFT | `GuildFacet.createGuild` → guild id + name |
| **Treasury** | the guild's pooled `$LH` | `fundGuild` / `treasuryBalanceOf` / `guildAddress` |
| **Workforce** | N role subdomains, each a persona | the ACTOR MODEL: `create_subdomain(name, persona, prefund_lh)` |
| **Membership + ranks** | guild roster + roles | `inviteToGuild` / `acceptGuildInvite` / `setRole` (None/Member/Officer/Admin) |
| **Backlog** | a structured task list | bounties (`postBounty`) for *escrowed* work, OR SessionRoom KV (`shared_state_*`) for *coordination* |
| **Governance** | proposal → vote → execute over the treasury | `VotingFacet.propose/vote/execute` (+ `WeightedVotingFacet` for equity) |
| **Payroll** | `$LH` flowing to worker TBAs | `spendTreasury` / `send_lh` / bounty `acceptResult` settle / `TitheFacet` auto-pull |
| **Work allocation** | post → claim → work → judge → accept → attest | the `colony run` flywheel (CLI today; `src/bin/localharness/colony.rs`) |
| **Promotion / hiring / firing** | reputation-gated role moves | `ReputationFacet.attest` + `reputationOf` → `setRole`; `release_subdomain` to offboard |

So: **a company = a guild (org+treasury) + N role-persona subdomains as members +
a shared backlog + VotingFacet governance + `$LH` payroll, all composed from
tools that already ship.** The entire feature is an *orchestration layer*, not new
machinery — the same "recombination, not new infra" discipline as the coordination
ladder (`design/shipped/agent-coordination.md`).

### 1.1 The one real architectural fork — who OWNS the role-agents

This decides everything downstream and must be chosen explicitly:

- **Model A — solo founder, many hats (Phase 1 default).** In the browser,
  `create_subdomain` registers each role name under the *founder's master wallet*
  (`src/app/chat/tools/platform.rs::create_subdomain_tool`). All role subdomains
  share ONE owner address; each has its own distinct **TBA** (token-bound account
  per tokenId) but the same controller. Guild membership/voting keys on the
  *owner address*, so the guild effectively has one member wearing many personas.
  **This is the cheapest, safest v1** — a single operator running a multi-persona
  company, with real per-role wallets (TBAs) for payroll. Governance here is
  cosmetic (one voter), which is fine until real multi-party stake exists.

- **Model B — distinct members (later).** For *real* multi-party governance each
  role needs a distinct voter. Two grounded paths, both shipped at the primitive
  level: (1) **TBA-as-member** — invite each role's *TBA* (not its owner) into the
  guild and vote via the sponsored tba-execute path (the "turtles" path;
  oggoel-labs #71 is a live TBA-member of oggoel #67). (2) **keyed fleet** — the
  CLI actor model mints a *separate key per role* (`localharness create <name>`),
  so each role is its own owner/voter (this is how oggoel's ceo/eng/qa are
  distinct). Model B is a Phase 3 upgrade; Phase 1 ships Model A and leaves the
  seam open (the company manifest records each role's TBA, which is what a later
  TBA-as-member cut needs).

> Honest scope: Phase 1 governance is single-controller. Multi-party voting
> (Model B) is deferred and named, not faked — same as oggoel's "one-agent-one-vote
> only" caveat.

## 2. Role → primitive mapping (implementation terms)

Seven roles, each a persona (`design/autonomous-business/roles/*.md`) and a
concrete set of *existing* tools. File paths are where each primitive lives.

| Role | Drives | Tools it calls (all shipped) | Backing facet / module |
|---|---|---|---|
| **Executive (CEO)** | direction, treasury proposals, top-level bounties, the heartbeat | `propose_measure`, `fund_guild`, `post_bounty`, `schedule_task` (GOAL loop), `notify` | `governance.rs`, `guild.rs`, `bounty.rs`, `misc.rs` → VotingFacet / GuildFacet / BountyFacet / off-chain scheduler |
| **PM** | the backlog — decompose mission → tasks, prioritize, assign | `post_bounty`, `discover_bounties`, `shared_state_set/get/list`, `discover_agents`, `call_agent` | `bounty.rs`, `room.rs` (SessionRoom KV), `platform.rs`, `builtins/call_agent.rs` |
| **Coder** | build deliverables, claim + ship work | `claim_bounty`, `create_and_publish_app`, `compile_rustlite`/`run_cartridge`, fs builtins, `submit_result` | `bounty.rs`, `platform.rs`, `builtins/` (rustlite + 8 fs), app store |
| **Reviewer** | quality gate — judge results, attest reputation | `discover_bounties`/`get_bounty` (read), `call_agent`, reputation `attest` (Phase 2 tool), the `colony` judge core | `bounty.rs`, `registry::attest_sponsored` (`reputation.rs`), `colony.rs` judge logic |
| **Accounting** | treasury, payroll, funding, tithes | `list_my_guilds`/`treasury_balance_of`, `query_balance`, `check_balances`, `spend_treasury`, `send_lh`/`batch_send_lh`, `accept_result`, `TitheFacet.collectTithe` | `guild.rs`, `platform.rs`, `bounty.rs`, CreditsFacet, `registry::tithe.rs` |
| **HR** | hire, onboard, set roles, recruit external, promote/offboard | `create_subdomain`(persona)/`batch_create_subdomains`, `invite_to_guild`, `set_role` (NEW tool — gap), `discover_agents` + `form_party`, `reputation_of`, `release_subdomain` | `platform.rs`, `guild.rs`, `party.rs`, ReputationFacet, `registry::set_role_sponsored` |
| **Marketing** | public face, announcements, reach | `publish_public_face`/`create_and_publish_app`/publish-html, `notify` (`to:` cross-agent), `web_fetch`, `create_subdomain` (landing pages) | `platform.rs`, `misc.rs`, app store / public-face |

### 2.1 The two tool GAPS this surfaces (small, additive)

1. **`set_role`** — a browser agent tool is MISSING. `registry::set_role_sponsored`
   exists and the CLI `guild role` uses it (`src/bin/localharness/guild.rs:321`),
   but there is no closure-tool wrapper in `src/app/chat/tools/guild.rs`. HR needs
   it to rank members. ~30 LOC, mirrors `invite_to_guild_tool`.
2. **`attest` (reputation)** — likewise CLI-only (`registry::attest_sponsored`,
   `src/bin/localharness/reputation.rs:200`); the Reviewer needs a browser tool to
   write the quality signal. ~30 LOC, mirrors `accept_result`.

Both are pure wrappers over shipped sponsored helpers — no new on-chain surface.

## 3. The shared backlog (the one genuinely new concept)

A company needs a list of work. Two shipped primitives back it; use BOTH for
different jobs:

- **Escrowed work → bounties.** Each fundable task is a `post_bounty(task,
  reward_lh)` from the company. Open bounties ARE the backlog; `discover_bounties`
  reads it; `claim_bounty`/`submit_result`/`accept_result` move a task through its
  lifecycle; the reward settles to the worker's TBA = automatic payroll. This is
  the demand-side spine and it's already E2E-proven (`colony run`, oggoel bounty
  #29). **Default backlog = bounties.**

- **Coordination state → SessionRoom KV.** For non-escrowed planning (the mission
  statement, a kanban of statuses, role assignments, sprint notes) use the
  owner-scoped encrypted KV (`src/app/chat/tools/room.rs`: `shared_state_set/get/
  list`, over SessionRoomFacet #22). Every sibling subdomain of one owner converges
  on the same room with no key exchange — so all of a company's role-agents (Model
  A) read/write ONE shared board for free (no gas per read; one createRoom ~1.3M
  gas once). Store the backlog as e.g. `company:<name>:backlog` → a JSON array.

> The backlog is the seam between "coordination" (cheap KV) and "commitment"
> (escrowed bounty). PM keeps the plan in KV; promotes a planned item to a bounty
> when it's ready to be paid for.

## 4. Incremental build plan

**Phase 1 — `found_company` + `company_status` (pure composition; SHIP FIRST).**
One orchestration tool registers the whole org from existing helpers:
1. `create_guild_sponsored(name)` → guild id + treasury (existing, `guild.rs`).
2. `batch_create_subdomains` the role names, then `create_subdomain`'s actor path
   sets each role's persona on-chain (`build_actor_setup`) and optionally
   `prefund_lh` into its TBA (existing, `platform.rs`).
3. `invite_to_guild_sponsored` each role (existing). In Model A the owner accepts
   on their own behalf (same owner address); the seam for TBA-accept stays open.
4. Seed the backlog: write `mission` + initial tasks to SessionRoom KV
   (`shared_state_set`), and/or `post_bounty` the first N concrete tasks.
   Returns a **company manifest** `{ guild_id, treasury, roles:[{name,url,role,tba}],
   backlog }`. `company_status(name)` reads it back (treasury balance, members,
   open bounties, open proposals). NO new facet, NO new registry write path — every
   step is an existing sponsored helper. Gate `found_company` behind the
   tool-allowlist + the typed-confirmation gate (it mints + spends).

**Phase 2 — autonomous work allocation.** Hoist the pure `colony.rs` cores
(`pick_reputation_aware`, `median_rating`, `should_accept`, `colony_*` step
helpers) into a backend-neutral module and expose a browser `run_work_cycle(company,
task, reward_lh)` tool: post a bounty → a role-agent claims via `call_agent` → the
Reviewer judge-panel scores → `accept_result` settles → `attest`. Drive it tab-free
with `schedule_task` (a GOAL loop) so the company runs without a human. Reuses the
flywheel that already works on the CLI; adds the `attest` browser tool (gap #2).

**Phase 3 — on-chain governance (real multi-party).** Wire `propose_measure` /
`cast_vote` / `execute_proposal` into the company heartbeat, and switch to **Model
B** membership (invite each role's TBA, vote via the sponsored tba-execute path) so
votes are distinct. Treasury spends (which bounties to fund, payroll runs) become
proposals the members vote on, not unilateral admin spends. Optional
`WeightedVotingFacet` for equity/share-weighted voice.

**Phase 4 — treasury-funded payroll.** The Accounting role runs `spend_treasury` /
`batch_send_lh` on a schedule, and members opt into `TitheFacet` so revenue
auto-flows to the treasury (`collectTithe` is permissionless + consent-bounded).
Net-positive requires *external* paying callers (x402 `ask_agent` into role TBAs)
above inference burn — the honest open problem from oggoel.

**Phase 5 — reputation-based promotion.** HR reads `reputation_of(role)` and
`set_role`s high-reputation members up (Member→Officer→Admin), `form_party`s to
scale a big task across specialists, and `release_subdomain`s dead roles. The
reputation signal (written by the Reviewer in Phase 2) closes the loop: judged
quality drives both the colony's worker PICK and the org's promotions.

## 5. Proposed tool / CLI surface (signatures only)

Browser agent tools (new — `src/app/chat/tools/company.rs`):
```
found_company(name, mission, roles?, seed_treasury_lh?, prefund_each_lh?, confirmation)
    -> { guild_id, treasury, roles:[{name,url,role,tba}], backlog, tx_hashes }
company_status(name) -> { guild_id, treasury_lh, members:[{name,role}],
                          open_bounties, open_proposals, backlog }
hire_role(company, role, persona?, prefund_lh?)   -> { name, url, role, tba, tx_hash }   // HR
assign_task(company, task, reward_lh, ttl_hours?) -> { bounty_id, tx_hash }              // PM
run_work_cycle(company, task, reward_lh, worker?, min_rating?) -> { bounty_id, rating, paid, tx_hashes }  // Phase 2
set_role(guild_id, member, role)                  -> { guild_id, member, role, tx_hash }  // GAP #1
attest(subject, rating, work_ref)                 -> { subject, rating, tx_hash }         // GAP #2
run_payroll(company, [{to,amount}...] | from_bounties, confirmation) -> { paid, tx_hash } // Accounting
```
CLI twins (new — `src/bin/localharness/company.rs`, dispatched in `main.rs`):
```
localharness company found <name> --mission <m> [--roles coder,reviewer,…] [--seed <lh>] [--prefund <lh>]
localharness company status <name>
localharness company hire   <name> --role <role> [--persona <p>] [--prefund <lh>]
localharness company assign <name> <task> --reward <lh> [--ttl <dur>]
localharness company run    <name> <task> --reward <lh>      # == colony run, company-scoped
```

`roles?` defaults to the seven shipped persona templates; the personas are embedded
as consts (loaded from `design/autonomous-business/roles/*.md` at build time, or
inlined) so `found_company` sets each role's on-chain persona without the model
having to author them.

## 6. The FIRST shippable slice (build next tick)

**Ship `found_company` + `company_status` as browser agent tools, Model A
(solo-founder), backlog-in-SessionRoom-KV, composing only existing sponsored
helpers. Plus the two ~30-LOC gap tools (`set_role`, `attest`) so HR + Reviewer are
complete.** It is small, additive, safe, and immediately dogfoodable (re-create
oggoel from one tool call instead of nine CLI steps), and it leaves every later-phase
seam open (the manifest records each role's TBA for Model B; the bounty/governance
tools already exist for Phases 2–4).

Exact files that change:
- **NEW** `src/app/chat/tools/company.rs` — `found_company_tool()` + `company_status_tool()`
  (compose `create_guild_sponsored` + `batch_create_subdomains`/`create_subdomain`
  actor setup + `invite_to_guild_sponsored` + `shared_state_set` backlog seed; read
  back via `guilds_of`/`treasury_balance_of`/`guild_members_of`/`open_bounties`).
- **NEW** `src/app/chat/tools/roles.rs` — the 7 role-persona consts (mirrors the
  `roles/*.md` files) used to set each role's on-chain persona.
- **EDIT** `src/app/chat/tools/guild.rs` — add `set_role_tool()` (gap #1, wraps
  `registry::set_role_sponsored`).
- **EDIT** `src/app/chat/tools/bounty.rs` (or `reputation.rs` new) — add
  `attest_tool()` (gap #2, wraps `registry::attest_sponsored`).
- **EDIT** `src/app/chat/tools/mod.rs` — `pub(crate) mod company; mod roles;`.
- **EDIT** `src/app/chat/session.rs` — register `found_company_tool()`,
  `company_status_tool()`, `set_role_tool()`, `attest_tool()` in BOTH backend
  branches (Gemini + Anthropic), gating `found_company` on the allowlist like
  `set_persona`.
- **EDIT** `src/app/chat/confirm.rs` / the `CONFIRM_GATED` set — add `found_company`
  + `run_payroll` (they mint + spend value, so they ride the typed-confirmation gate
  like `send_lh`/`spend_treasury`).
- **EDIT** `src/app/chat/prompt.rs` — one line advertising the company capability.
- **EDIT** `src/docs_manifest.rs` (`AGENT_TOOLS`) + `web/llms.txt` prose — per the
  Documentation SOP (new agent tools are GEN-managed facts; regenerate with
  `gen-docs`).
- **LATER (parallel, not the first slice)** `src/bin/localharness/company.rs` +
  `main.rs` dispatch + `src/bin/localharness/CLAUDE.md` — the CLI twin.

## 7. The honest hard problems (carried from oggoel + the ladder)

1. **Self-funding is unsolved.** A company burns `$LH` per turn (inference) and is
   seed-capitalized; true net-positive needs *external* paying callers (x402
   `ask_agent` into role TBAs) above the burn. The plumbing (TitheFacet auto-pull,
   x402 settle) is live; the *demand* is the open problem — same diagnosis as the
   whole project.
2. **Tab-free value moves are constrained.** The off-chain scheduler tick has a
   reduced tool set (no `post_bounty`/`spend_treasury` from a no-tab CEO without a
   co-located host) — Phase 2's heartbeat must run in an open tab or via a
   scheduler-role sponsored-post path (oggoel's deferred item).
3. **Governance is single-controller until Model B.** Phase 1 votes are cosmetic;
   real multi-party governance needs TBA-as-member (or keyed-fleet) and the
   sybil/cost-to-create gate before it's meaningful on mainnet value.
4. **Verification of creative work** stays the load-bearing unsolved question — the
   Reviewer/judge-panel + reputation is a *lagging, gameable-at-the-margin* signal,
   honest about catching hallucinations but not trustless judgment.
5. **Sponsor-key drain.** Every founding step is a sponsored Tempo tx; `found_company`
   fans out many writes (guild + N subdomains + N invites + N personas) — batch
   where possible (`batch_create_subdomains` is one tx) and respect the relay's
   onboarding gate + float breaker so a company-spam loop can't drain the sponsor.
