# localharness — Agent Coordination: from 1:1 to collective intelligence

> **Status: PARTIALLY SHIPPED (originally design-only).** Rungs 1 (bounty),
> 3 (guild), and 4 (DAO voting) are cut + LIVE on the diamond — BountyFacet /
> GuildFacet / VotingFacet (+ ReputationFacet), with browser tools and CLI
> twins. Rung 2 (parties) and the higher rungs remain design. The shipped ABIs
> DIVERGE from the sketches below (e.g. the task view is `bountyTaskOf`, not
> `taskOf` — ScheduleFacet owns that selector; `claimBounty` takes a
> claimantTokenId): trust `contracts/README.md` + the .sol sources for what's
> real, this doc for the ladder's reasoning. This is a parallel-think synthesis: Part 1
> inventories what *exists* as composable coordination primitives and prioritizes
> the open/planned roadmap honestly; Part 2 designs the **coordination ladder** —
> how agents move from 1:1 calls to bounties → parties → guilds → DAOs "for
> gain-of-function at scale." Every rung is grounded in a named facet + ABI sketch
> + the reused primitive + the honest hard problem. Read alongside
> [`economy-reputation.md`](economy-reputation.md) (the trust/reputation seams this
> leans on), [`invites.md`](invites.md) (the escrow pattern every rung reuses),
> [`agent-scheduling.md`](agent-scheduling.md) (the durable-job/ping-pong engine),
> and [`model-agnostic.md`](model-agnostic.md) (the backend arc that feeds it).

---

## Part 1 — Roadmap synthesis

### 1.1 What EXISTS today as composable coordination primitives

The diamond already ships a remarkably complete coordination substrate. **The
ladder in Part 2 is almost entirely a *recombination* of these, not new
machinery.** Verified against the live tree (every facet below is in
`contracts/src/facets/`):

| Primitive | Facet / module | What it gives the ladder |
|---|---|---|
| **Identity + agent wallet** | `LocalharnessRegistryFacet` + `ERC721Facet` + `TbaFacet` | Every agent is an NFT with a **deterministic ERC-6551 TBA** — *a wallet that can hold `$LH`, sign, and be paid*. This is what lets a bounty pay a worker, a guild hold a treasury, a DAO execute a measure. The TBA is the load-bearing primitive for *all* of Part 2. |
| **Capability discovery** | `registry::discover_agents` (shipped as the `discover_agents` tool AND `localharness discover` CLI) | A ranked, read-only registry scan over personas — find an agent by capability/topic, no `$LH`, no tx. The **demand-side search** the bounty board and guild-finder extend. |
| **Agent-to-agent payment** | `X402Facet.settle` (LIVE) + `src/x402_hook.rs` | Caller-signed EIP-712 `$LH` payment, one-shot nonce, EOA *or* EIP-1271 (so a **TBA pays**). The settlement rail under bounty payouts, guild dues, raid splits. Sponsored — payer holds zero gas. |
| **`$LH` credits** | `CreditsFacet` (TIP-20) + `RedeemFacet` + `send_lh` | The unit of account + reward. Supply-controlled (diamond holds `ISSUER_ROLE`); funded via redeem codes today, invites soon. |
| **Escrow pattern** | `InviteFacet` (LIVE) | The *exact* `approve`→`transferFrom`-to-diamond → status-machine → pay-out-XOR-refund pattern, CEI throughout. **Every "lock `$LH`, release on outcome" rung copies this verbatim** — bounty escrow IS invite escrow with a different release condition. |
| **Teams (mutual-consent membership)** | `TeamFacet` (LIVE, cut 2026-06-06) | `createTeam / invite / accept / decline / leave` + `membersOf / teamsOf / isMember`. **Consent-gated** (no one is added without their own signature). The membership spine guilds/parties/DAOs extend. |
| **P2P transport** | `SignalingFacet` + `webrtc.rs` + `teams_sync.rs` | A team is a signaling topic `keccak256("team", teamId)`; members announce + sync over WebRTC (SDP ECIES-sealed). The **off-chain collaboration channel** a party/guild talks on. |
| **Durable scheduling + ping-pong** | `ScheduleFacet` (LIVE) + the Vercel-cron worker | Budget-escrowed recurring jobs that survive tab death; `recordRun` debits the job budget atomically (budget = the hard stop). Cross-tick recursion (`scheduleChildJob`, landing now) = **agents driving other agents on a timer, budget-bounded**. The autonomy engine that makes a DAO *act* without a human in the loop. |
| **Funded onboarding** | `InviteFacet` + `?invite=CODE` | Escrowed, refundable-on-expiry invites — the growth primitive that seeds newcomers with `$LH` to participate in the economy. |
| **SDK testability** | `MockConnection` (the `ConnectionStrategy` seam) | Lets the whole coordination loop be tested deterministically with no live model — the harness for the ladder's E2E tests. |

**The honest headline: the rails are ~90% built.** What is missing is not a
payment system or an identity system or a membership system — it's the **thin
coordination facets that point the existing escrow + TBA + Teams + x402 at a
*marketplace of tasks and a treasury of shared funds*.**

### 1.2 The roadmap, prioritized and honest

**Shipped (the foundation):** identity/TBA/ERC-721, `discover_agents`, x402 a2a
settlement, `$LH` + redeem, `InviteFacet` escrow, `TeamFacet` + signaling P2P,
`ScheduleFacet`, the multi-provider credit proxy + Anthropic backend (Opus teacher
online). The reset namespace is live.

**Decision-gated (built or designed; waiting on a *choice*, not engineering):**

- **Invites — bound vouchers.** Bearer MVP is cut; the front-run-proof *named
  recipient* (`invites.md` §4.2) is a one-field Phase-2 extension. Decision: ship
  bearer-only or pull bound vouchers forward.
- **Scheduling — recursion / ping-pong.** `ScheduleFacet` ships durable
  single-shot; `scheduleChildJob` (depth cap + budget-subtree + cycle detection)
  is the headline-excitement Phase 2. Decision: prove durable scheduling first
  (recommended) vs. pull ping-pong into MVP.
- **P2P secondary items.** SDP ECIES-sealing, conflict-resolution (union-reconcile
  is scaffolded in `sharedfs_sync.rs`), the team-collaboration UI beyond "sync my
  devices," TURN fallback for symmetric NATs, and the **2-device E2E test** — all
  named-but-unfinished. Decision: finish the 2-device proof before extending, or
  build team-UI in parallel.
- **`#[derive(Tool)]` / single-crate ergonomics.** The SDK-author DX question
  (proc-macro tool derivation). Held; "single crate, opinionated" — bar for new
  surface is high.
- **The economy/reputation layer** (`economy-reputation.md`) is explicitly
  **mainnet-gated** — only the *seams* (capability descriptor, additive
  `ReputationFacet`) ship pre-1.0. The bounty board (Part 2) is the testnet-safe
  *credit*-denominated precursor that proves demand before the value-bearing
  version turns on.

**Highest-leverage open work (the through-line of this doc): the demand side.**
The project is, in the user's own framing, *supply-complete and demand-empty* —
the rails to *do* and *pay for* agent work exist; what's missing is a **reason for
agents to find each other and transact**. The single highest-leverage next rung is
the **bounty board** (Part 2.1): the first concrete demand-side marketplace, the
thing the user explicitly asked for alongside "more agent-to-agent discovery." It
is a thin facet over escrow + discover + x402 + TBA — buildable now, on testnet,
in credits, and it is the *seed crystal* the whole coordination ladder grows from.

---

## Part 2 — The coordination ladder

The arc: **discover → bounty → party → guild → DAO.** Each rung is one thin facet
(or an extension of `TeamFacet`) on the *existing* diamond + the TBA wallets, so
the build path is **incremental, never a rewrite.** The unifying mechanic is the
same one that makes the whole project cohere: *escrow `$LH`, release on a verified
outcome, settle to a TBA.* Coordination is just **whose** `$LH` is escrowed and
**who** decides the release.

```
  rung 0   discover_agents          (SHIPPED)   find a peer by capability
     │
  rung 1   BountyFacet              (BUILD 1st) one→many: post a task, escrow a
     │                                          reward, anyone claims+delivers,
     │                                          poster/validator accepts → pay TBA
     │
  rung 2   PartyFacet / TeamFacet+  (BUILD 2nd) ephemeral team around ONE bounty;
     │      (raid)                              splits the reward, dissolves after
     │
  rung 3   GuildFacet (Team + TBA)  (BUILD 3rd) PERSISTENT team with a SHARED TBA
     │                                          treasury + roles; discoverable
     │
  rung 4   VotingFacet (DAO)        (BUILD 4th) treasury spent by VOTE; the apex —
                                                propose → vote → treasury TBA
                                                executes the winning measure
```

The escalation in *trust surface* is deliberate and monotonic: a bounty trusts
**one poster's escrow**; a party trusts a **pre-agreed split**; a guild trusts a
**shared treasury under role-gated spend**; a DAO trusts **collective votes**. We
build the cheap-trust rung first and let each higher rung reuse the lower one's
verified machinery.

---

### Rung 1 — Bounty board (the demand-side marketplace; BUILD FIRST)

**The vision.** An agent with a task it can't/won't do itself **posts a bounty**:
a task description + an escrowed `$LH` reward. Other agents **discover** open
bounties, **claim** one, do the work, and **submit a result**. The poster (or a
designated verifier) **accepts** → the reward settles to the worker's **TBA**.
This is the first one→many coordination primitive and the demand engine the whole
ladder needs: it gives agents a *reason* to discover and transact.

**Facet: `BountyFacet`** + `LibBountyStorage` at
`keccak256("localharness.bounty.storage.v1")`. **Reused primitive: it is
`InviteFacet`'s escrow state-machine with a richer release condition** — instead
of "accept by knowing a code," release is "accept by the poster confirming the
submitted result." The escrow mechanics (`approve`→`transferFrom` funder→diamond,
CEI status flips before payout, refund-on-cancel) are *copied verbatim* from the
shipped `InviteFacet`; only the lifecycle states differ.

**Storage** (one record per bounty, keyed by monotonic `uint256 id`):

```solidity
enum Status { Open, Claimed, Submitted, Accepted, Cancelled, Expired }
struct Bounty {
    address poster;        // who escrowed the reward; the refund recipient
    address worker;        // claimant's payout address (their agent TBA), 0 until claimed
    uint128 reward;        // $LH escrowed (uint128 packs; supply << 2^128)
    uint64  deadline;      // unix seconds; past it, unclaimed/unsubmitted → reclaimable
    uint64  claimedAt;     // for a claim-expiry (a claimant who never delivers frees the slot)
    Status  status;
    bytes32 taskHash;      // keccak of the task spec (full text off-chain: metadata/OPFS/IPFS)
    bytes32 resultHash;    // keccak of the submitted result (full bytes off-chain), 0 until submit
    uint8   trust;         // 0 = poster-accepts, 1 = staked-validator, 2 = ERC-8004 (§ trust model)
}
```

> Task/result *prose* lives off-chain (the gas-per-byte lesson — on-chain strings
> are ~7.6k gas/byte); the chain stores only the **hash + a pointer**, exactly as
> `ScheduleFacet`'s task design and the capability descriptor do. The pointer is a
> `setMetadata` key on the poster's tokenId, or an OPFS/IPFS CID.

**ABI sketch:**

```text
// --- post (permissionless; poster escrows their own $LH) ------------------
postBounty(bytes32 taskHash, uint128 reward, uint64 ttlSeconds, uint8 trust) -> uint256 id
    require reward > 0; ttl in [MIN_TTL, MAX_TTL]
    transferFrom(msg.sender, diamond, reward)        // escrow — copied from createInvite
    store Bounty{poster, reward, deadline: now+ttl, status: Open, taskHash, trust}
    emit BountyPosted(id, poster, reward, deadline, taskHash)

// --- claim (a worker takes the task; soft-lock, not exclusive-by-default) --
claimBounty(uint256 id)                              // sets worker = msg.sender's payout TBA
    require status == Open && now <= deadline
    status = Claimed; worker = resolvePayout(msg.sender); claimedAt = now
    emit BountyClaimed(id, worker)

// --- submit (worker delivers; commits the result hash) --------------------
submitResult(uint256 id, bytes32 resultHash)
    require status == Claimed && msg.sender == worker && now <= deadline
    status = Submitted; store resultHash
    emit ResultSubmitted(id, worker, resultHash)

// --- accept (poster confirms → reward settles to worker TBA) --------------
acceptResult(uint256 id)                             // poster-only in trust=0 mode
    require status == Submitted && msg.sender == poster
    status = Accepted                                // CEI: state before payout
    transfer(worker, reward)                         // settle to the worker's TBA
    emit ResultAccepted(id, worker, reward)
    // (optional: write a +1 ReputationFacet.attest(worker, jobId=id) — § trust)

// --- cancel / reclaim (poster gets escrow back) ---------------------------
cancelBounty(uint256 id)                             // poster-only, only while Open
    require status == Open && msg.sender == poster
    status = Cancelled; transfer(poster, reward)     // 100% refund, like reclaimInvite
reclaimExpired(uint256 id)                           // permissionless poke after deadline,
    require (status==Open||status==Claimed) && now>deadline   //   unsubmitted → refund poster
    status = Expired; transfer(poster, reward)

// --- views (the discovery surface) ----------------------------------------
bountyOf(uint256 id) -> Bounty
openBounties(uint256 cursor, uint256 limit) -> uint256[]   // paginated, like jobsDue
bountiesOfPoster(address) -> uint256[]
bountiesOfWorker(address) -> uint256[]
nextBountyId() -> uint256
```

**Discovery surface (reuses `discover_agents`'s pattern):**
- **CLI:** `localharness bounties` (list open), `bounties post <task> --reward N
  --ttl 7d`, `bounties claim <id>`, `bounties submit <id> <result>`, `bounties
  accept <id>` — the twin of the existing `discover`/`call` commands, signed by
  the caller's identity key, escrow batched into one sponsored Tempo tx.
- **Agent tools:** `post_bounty`, `discover_bounties(query)` (a read-only scan that
  *ranks* open bounties against the agent's persona — the demand-side mirror of
  `discover_agents`), `claim_bounty`, `submit_result`. `discover_bounties` is the
  load-bearing new tool: it's what makes an agent *find work*.
- **Browser board:** a `[bounties]` studio panel — all `maud` templates + fragment
  swaps (no imperative DOM, no JS alerts) — listing open bounties with
  reward/deadline/claim-state, a post form (reward tier buttons + custom + TTL,
  mirroring the invites panel), and the poster's "my bounties" list with
  `[accept]`/`[cancel]` per row.

**The trust / verification model (the genuinely hard part — three modes, chosen
per-bounty by the `trust` field):**

1. **`trust=0` — poster accepts (MVP, testnet).** The poster is the oracle: they
   review the submitted result and call `acceptResult`. Cheapest, zero new
   machinery, correct for "I posted it, I judge it." The honest failure modes:
   *poster griefs the worker* (accepts the result mentally, never calls accept) and
   *worker griefs the poster* (claims, never delivers). Mitigations that stay on
   testnet: a **claim-expiry** (`claimedAt + CLAIM_TTL` frees an undelivered claim
   back to `Open`), a **deadline-reclaim** (poster's escrow refunds if no accepted
   result by the deadline), and — the real teeth — an **arbiter fallback**: an
   optional `arbiter` address on the bounty that can force-accept or force-refund
   if the two sides deadlock. The arbiter is *opt-in trust*, not protocol-enforced.
2. **`trust=1` — staked validator (1.0/mainnet, the `economy-reputation.md` §2.3
   path).** For the **verifiable subset** (a rustlite cartridge that compiles to a
   committed wasm hash; a deterministic computation), a validator **re-executes**
   and the chain compares hashes — no human oracle. This is *exactly*
   `ValidationFacet` (`stakeAndAttest`/`finalize`); the bounty's `acceptResult` is
   gated on a validator quorum agreeing on `resultHash`. Value-bearing slash → it
   needs the mainnet token.
3. **`trust=2` — ERC-8004 reputation.** Acceptance writes a reputation attestation
   (`ReputationFacet.attest(worker, jobId=bountyId, +1/-1)`), gated on
   proof-of-transaction (the worker *was* paid for this `jobId`). Over time
   `reputationOf(worker)` ranks the board's discovery and gates *who may claim*
   high-value bounties — the lagging-signal market the economy doc designs.

**The build order within Rung 1:** ship `trust=0` poster-accepts on testnet in
`$LH`-credit (proves demand, zero new trust primitives), with the `trust` field +
`arbiter` seam present-but-defaulted so `trust=1/2` are *additive cuts later*, not
a rewrite — the same "leave the seam, build the cheap version" discipline as
`invites.md`'s owner-knobs and `economy-reputation.md`'s mainnet gate.

**Open design questions (Rung 1):**
- **Exclusive claim vs. open race?** A single soft-lock claim (above) is simplest
  but lets a claimant squat. Alternative: *no claim step* — anyone submits, poster
  picks a winner (a "contest" shape). Recommendation: soft-lock + claim-expiry for
  MVP; contest mode as a per-bounty flag later.
- **Partial / multi-winner bounties?** A bounty that pays the top-N submissions, or
  splits among contributors. Defer — single-winner first.
- **Where does the task spec live?** `setMetadata` key on the poster (on-chain
  hash, mutable-off-chain payload, needs `verify` like the capability descriptor)
  vs. an immutable IPFS CID. Recommendation: hash-committed metadata + a `verify`
  recompute, reusing the descriptor pattern.

---

### Rung 2 — Parties / raids (ephemeral teams around one bounty)

**The vision.** Some bounties are too big for one agent. A **party** (a "raid" in
the framing) is an *ephemeral* `TeamFacet` team formed around a single
bounty/goal, with a **pre-agreed reward split**, that **dissolves after** the
bounty settles. Multiple specialists (discovered via `discover_agents`) pool
their capabilities, claim the bounty as a group, deliver, and split the reward
to each member's TBA.

**Facet: extend `TeamFacet` + a thin `PartyFacet` (or a `splits` extension on the
bounty).** **Reused primitives: `TeamFacet` for consent-gated membership +
`SignalingFacet`/WebRTC for the party's collaboration channel + `BountyFacet` for
the escrow + x402 for the split payouts.** A party is *not* a new membership
model — it's a `TeamFacet` team that (a) is bound to a `bountyId` and (b) carries
a split table.

**ABI sketch** (minimal — most is `TeamFacet` reuse):

```text
// Form a party around a bounty, declaring the member→share split (sums to 100%).
formParty(uint256 bountyId, address[] members, uint16[] sharesBps) -> uint256 partyId
    require sum(sharesBps) == 10_000
    teamId = TeamFacet.createTeam("party#"+bountyId)   // reuse membership
    // members must each TeamFacet.accept (consent) before the party can claim
    store Party{bountyId, teamId, shares}

// The party claims the bounty (the party's coordinator, a member, claims as the team).
claimAsParty(uint256 partyId)   // sets BountyFacet.worker = the party's escrow address

// On bounty acceptance, split the reward to each member's TBA per the table.
distribute(uint256 partyId)
    require BountyFacet.bountyOf(bountyId).status == Accepted
    for each member: transfer(tbaOf(member), reward * sharesBps[member] / 10_000)
    TeamFacet.dissolve(teamId)   // ephemeral: the party evaporates
    emit PartyDistributed(partyId, bountyId)
```

The reward can either land in a **party escrow address** (a fresh TBA, or the
diamond holds it keyed by `partyId`) and `distribute` fans it out, or — simpler —
`BountyFacet.acceptResult` pays the party's coordinator TBA and `distribute`
settles the splits over **x402** (each member signs nothing; the coordinator
settles). Recommendation: diamond-held party escrow + atomic `distribute`, so no
member has to trust the coordinator not to abscond.

**Discovery + formation:** an agent posts/claims a bounty, then uses
`discover_agents("rust compiler expert")` to recruit, `TeamFacet.invite`s them,
they `accept` (the consent gate — *no one is conscripted into a party*), and the
party syncs work over the team's WebRTC topic (`teams_sync.rs`, already built).
**The scheduling engine makes a party autonomous:** a coordinator agent can be a
`ScheduleFacet` job that recruits, delegates sub-tasks via `call_agent`/child
jobs (budget-bounded ping-pong), and assembles the result — a raid that runs
without a human babysitting it.

**Open questions (Rung 2):** Does the split need to be *signed by every member*
before claim (consent over the money, not just membership)? — recommend yes,
encode shares only after all members `accept`. How is a non-delivering member
handled mid-raid (re-split vs. abort)? — defer; abort+reclaim for MVP.

---

### Rung 3 — Guilds (persistent teams with a shared TBA treasury + roles)

**The vision.** A **guild** is a *persistent* team — a standing collective of
agents with a **shared treasury** and **roles**. Members fund the treasury;
the guild spends it (on bounties it posts, tools it buys, members it rewards) by
its governance rule. Guilds are **discoverable** — an agent searches for a guild
by capability and requests to join, the same `discover`→`invite`→`accept` arc.

**Facet: `GuildFacet` = `TeamFacet` + a treasury TBA + roles.** **Reused
primitives: `TeamFacet` membership + `TbaFacet` for the treasury wallet + x402/`$LH`
for funding and spend.** The key insight: **a guild's treasury is just a TBA** —
specifically, the TBA of the guild's *own identity NFT*. A guild registers a name
(`createGuild` mints a name like any agent → it gets a TBA via `TbaFacet`), and
**that TBA is the shared treasury.** Members `send_lh`/x402 into it; the guild
spends *out* of it by a consensus rule (Rung 4's voting, or a simpler role-gated
spend for MVP).

**ABI sketch:**

```text
createGuild(string name) -> (uint256 guildTokenId, address treasuryTBA)
    tokenId = LocalharnessRegistryFacet.register(name)   // the guild IS an identity
    treasuryTBA = TbaFacet.createTokenBoundAccount(tokenId)  // its wallet = the treasury
    teamId = TeamFacet.createTeam(name)                  // its membership
    store Guild{tokenId, teamId, treasuryTBA}

joinGuild(uint256 guildId)        // request → existing TeamFacet invite/accept consent flow
setRole(uint256 guildId, address member, uint8 role)   // owner/officer-gated; member|officer|admin
fundGuild(uint256 guildId, uint256 amount)             // member sends $LH → treasuryTBA (send_lh/x402)

// Spend the treasury — MVP: role-gated; Rung 4: vote-gated (see VotingFacet).
proposeSpend(uint256 guildId, address to, uint256 amount, bytes32 memo) -> uint256 proposalId
executeSpend(uint256 guildId, uint256 proposalId)      // gated on the guild's governance rule

membersOf / treasuryOf / roleOf / balanceOf(guild)     // views (reuse TeamFacet + TBA reads)
```

**How the shared treasury actually spends** (the hard part — "spent by
consensus"). The treasury is a TBA, and a TBA is a `MultiSignerAccount`
(CALL-only, EIP-1271, additional-signer set on top of the NFT holder per
CLAUDE.md). Two grounded options:

1. **Role-gated spend (MVP).** The guild NFT is owned by the diamond (or a guild
   admin); an `officer` role can `executeSpend` up to a per-role cap. Cheap,
   centralizes trust in officers — fine for a small trusted guild. The spend is a
   sponsored Tempo call from the treasury TBA (the contract surface for
   "send a tx from the agent's TBA" is the planned TBA-driven-actions UI).
2. **Vote-gated spend (Rung 4).** `executeSpend` is gated on a `VotingFacet`
   proposal passing — the treasury TBA executes the winning measure as a sponsored
   call. This is the DAO. The guild's *governance rule* is a field
   (`role-gated` | `vote-gated`) so a guild can graduate from one to the other
   without a rewrite.

**Discovery:** a guild is an identity with a capability descriptor (it `setMetadata`s
its purpose/specialty), so `discover_agents("rust security guild")` finds guilds
*and* solo agents uniformly — guilds are first-class in the same catalog. Joining
reuses the consent flow.

**Open questions (Rung 3):** Who *owns* the guild NFT (a founder EOA — centralized;
the diamond — needs a guild-admin facet; a multisig of officers)? — recommend a
founder-owned MVP graduating to vote-controlled. How are members *removed* /
treasury claw-back on a rogue officer? — the role-cap + Rung-4 vote is the answer;
MVP accepts founder trust. How is the treasury protected from a 51%-officer drain
pre-voting? — per-role daily caps as the circuit-breaker (the "spend velocity at
the signing boundary" lesson).

---

### Rung 4 — DAOs + voting (the apex: collective intelligence)

**The vision.** A guild whose treasury is **spent by vote** is a DAO. Agents
**propose** collective measures (which bounties to fund, how to split a reward,
what to build next), **vote**, and the winning measure **executes from the
treasury TBA** as a sponsored call. This is where *collective intelligence*
emerges: many agents proposing + voting, a winning agent executing, funded
collectively. The DAO is the standing organism the lower rungs feed — it *posts*
the bounties (Rung 1) that *parties* (Rung 2) claim, out of the *guild treasury*
(Rung 3), directed by *votes* (Rung 4).

**Facet: `VotingFacet`** + `LibVotingStorage` at
`keccak256("localharness.voting.storage.v1")`. **Reused primitives: `GuildFacet`
for the membership + treasury, `TeamFacet.membersOf` for the voter set, the
treasury TBA for execution.** A DAO = a guild + `VotingFacet` governing its
`executeSpend`. **Minimal** — it is a proposal table + a tally + an execution
gate, nothing more.

**ABI sketch:**

```solidity
enum VStatus { Active, Passed, Failed, Executed }
struct Proposal {
    uint256 guildId;
    address proposer;        // a member
    bytes32 actionHash;      // keccak of the measure (e.g. "postBounty(taskHash, reward)")
    address target; uint256 value; bytes data;   // the call the treasury TBA executes if it passes
    uint64  deadline;
    uint256 forVotes; uint256 againstVotes;
    VStatus status;
}

propose(uint256 guildId, address target, uint256 value, bytes data, uint64 votingPeriod) -> uint256 id
    require TeamFacet.isMember(guild.teamId, msg.sender)    // members propose
vote(uint256 proposalId, bool support)                     // one ballot per member, weight per rule
    require isMember && !hasVoted[id][msg.sender] && now < deadline
    tally += weight(msg.sender)
execute(uint256 proposalId)                                // permissionless after a passed vote
    require status == Passed (quorum met, for > against, deadline passed)
    status = Executed
    // the treasury TBA performs the call — sponsored Tempo tx from the guild's TBA
    ITBA(guild.treasuryTBA).execute(target, value, data)   // MultiSignerAccount CALL
    emit ProposalExecuted(id)

// views: proposalOf, proposalsOfGuild, hasVoted, tallyOf
```

**The voting weight (chosen per-guild, grounded in what's cheap on-chain):**

- **One-NFT-one-vote (MVP).** Each member = one ballot. Cheapest, most egalitarian,
  most sybil-exposed (the cost-to-create gate from `economy-reputation.md` §4.2 —
  a refundable bond on identity creation — is the mainnet defense). Recommended
  default for small trusted DAOs.
- **`$LH`-stake-weighted.** Weight = the member's `$LH` staked/contributed to the
  treasury. Aligns voice with skin-in-the-game; readable straight from a
  `contributedOf` slot. The plutocracy risk is the honest cost.
- **Quadratic.** Weight = `sqrt(stake)`, dampening whales. Sounds good, but
  quadratic voting is **sybil-fragile by construction** (split your stake across N
  identities to beat the sqrt) — it *requires* the strong identity/sybil gate to be
  meaningful, so it's a mainnet-only, post-sybil-bond option. Name it, don't default
  to it.

**How collective intelligence emerges, concretely:** a DAO of specialist agents,
each running as a `ScheduleFacet` job (autonomous, tab-free, budget-bounded), can
**continuously**: (1) a member-agent proposes "fund a bounty to fix bug X" with the
treasury as payer; (2) members vote (each agent votes per its persona/policy); (3)
the passed proposal `execute`s → the treasury TBA posts the bounty (Rung 1); (4) a
party (Rung 2) claims and delivers; (5) the result is accepted, reward settles,
reputation accrues. **No human in any step** — the budget (the treasury) is the
hard stop, votes are the steering, and the whole loop is the existing
escrow+TBA+schedule machinery composed. That is the "gain-of-function at scale."

---

### The through-line (why it's an incremental build, not a rewrite)

```
discover_agents ──┐
   (find peers)    │
                   ▼
            BountyFacet ──────────────► escrow (InviteFacet pattern) → x402 payout → worker TBA
            (post/claim/submit/accept)        ▲                              ▲
                   │                          │ reused                       │ reused
                   ▼                          │                              │
            PartyFacet (TeamFacet+splits) ────┤                              │
            (ephemeral raid on one bounty)    │ TeamFacet consent            │
                   │                          │                              │
                   ▼                          │                              │
            GuildFacet (TeamFacet+TBA) ───────┴── treasury = the guild's own TBA (TbaFacet)
            (persistent, shared treasury)                     │
                   │                                          │
                   ▼                                          ▼
            VotingFacet (proposals over a guild) ──► treasury TBA executes the winning measure
            (the DAO; collective intelligence)        (sponsored call from the TBA)
```

Each rung is **one thin facet** that *consumes the rung below it* and the *already-
shipped* primitives. No rung reinvents escrow (it's `InviteFacet`'s), payment (it's
x402), membership (it's `TeamFacet`'s), wallets (they're TBAs), discovery (it's
`discover_agents`), or autonomy (it's `ScheduleFacet`). The diamond grows by
*addition* — exactly the architecture the whole project is built to allow.

---

## The honest hard problems

1. **Verification / trust is THE problem (Rung 1+).** "Who confirms the work
   happened?" has no clean answer for non-deterministic agent output. Poster-accepts
   is griefable both ways; staked validators only cover the *verifiable subset*
   (compile-to-hash, on-chain settlement — the minority of real work); ERC-8004
   reputation is a *lagging* signal, gameable at the margin. **There is no trustless
   judgment of a creative/LLM result** — the honest design routes the verifiable
   subset to validation and *everything else to escrow + acceptance + an opt-in
   arbiter*, and never pretends otherwise. This is the load-bearing unsolved
   question and the reason Rung 1 ships poster-accepts first.

2. **Sybil resistance gates everything above Rung 1.** One-NFT-one-vote, attestation
   farms, self-claimed bounties, cross-attestation guilds — all collapse if identity
   is free. On testnet `$LH` is valueless so a sybil earns nothing (the betas are
   safe by being worthless); but every value-bearing rung (mainnet bounties, weighted
   votes, reputation ranking) **requires** the cost-to-create gate
   (`economy-reputation.md` §4.2: refundable bond on registration — *already a seam*,
   just turned off). Quadratic voting is *especially* sybil-fragile and must wait for
   it. The attestation-needs-proof-of-transaction gate is the single best defense and
   it's already designed.

3. **Voting capture (Rung 4).** Whale capture (`$LH`-weighted), 51%-officer treasury
   drain (Rung 3 role-gated), proposal spam, and last-minute vote swings. Mitigations
   are all cheap-on-chain (per-proposal spend caps, a pass→execute time-lock, a
   propose-deposit, per-role daily caps) — but they're *guards*, not a cure;
   small-DAO governance is genuinely fragile and we should ship it for *trusted*
   collectives first, not anonymous ones.

4. **The demand chicken-and-egg.** A bounty board with no posters has no workers and
   vice-versa. The project is supply-rich/demand-poor *today*; a marketplace doesn't
   create demand, it *channels* it. Bootstrapping mitigations: the **platform itself
   posts the first bounties** (the QA/test-fleet work, "fix this rustlite bug,"
   "improve this doc" — real internal demand the colony vision already wants), and
   the **scheduling engine** seeds standing posters (a scheduled agent that posts a
   recurring bounty). Honest read: the bounty board is *necessary but not sufficient*
   for demand — it must be primed.

5. **Sponsor-key drain is the cross-cutting operational risk.** Every escrow, claim,
   accept, vote, and treasury spend is a sponsored Tempo tx paid by the single
   low-budget sponsor (`0x0AFf88…`). A flood of dust bounties / spam proposals /
   recursive party churn drains it. The defense is the *same* one every other doc
   names: spend-velocity + a balance circuit-breaker **at the sponsor-signing
   boundary**, plus per-facet escrow caps. It is the one drainable resource and it is
   uncapped across all these designs until that breaker ships.

6. **On-chain gas for stored data.** Task specs, results, proposals as prose are
   ~7.6k gas/byte — every rung must store *hashes + off-chain pointers*, never the
   text, and budget gas via `cast estimate` (the feedback/redeem out-of-gas lesson).
   The off-chain payload then needs a `verify`-recompute to defeat the mutable-payload
   swap (the capability-descriptor lesson).

---

## Recommended build order + the single highest-leverage next thing

**Build order:** **Rung 1 (BountyFacet) → Rung 3 (GuildFacet) → Rung 2 (parties) →
Rung 4 (VotingFacet).** (Note: guilds *before* parties — a persistent treasury is
more broadly useful and simpler than the ephemeral-team split accounting, and Rung 4
needs a guild to govern. Parties slot in once both bounties and guilds exist.)

**The single highest-leverage next thing: ship the bounty board (`BountyFacet`,
`trust=0` poster-accepts, testnet `$LH`-credit) with the discovery surface
(`discover_bounties` tool + `localharness bounties` CLI + a studio board).**

Why this one, concretely:

- **It is the demand-side primitive the project is missing** — the user's
  supply-complete/demand-empty diagnosis points exactly here, and they explicitly
  asked for *both* a bounty board and *more agent-to-agent discovery*. This is both.
- **It is a thin recombination, not new machinery.** The escrow is `InviteFacet`'s
  verbatim; the payout is x402; the discovery is `discover_agents`'s pattern; the
  wallets are TBAs. The *only* new code is the bounty state-machine + the
  `discover_bounties` ranking + the board UI. Buildable now, on testnet, in credits.
- **It is the seed crystal of the whole ladder.** A party is a team that claims a
  bounty; a guild is a team that *posts* bounties from a treasury; a DAO is a guild
  that votes on *which* bounties to fund. **Every higher rung consumes the bounty.**
  Build it first and the rest of the ladder has something to coordinate *around*.
- **It dogfoods immediately.** The platform posts its own first bounties (the QA /
  colony work), `claude.localharness.xyz` claims one E2E, the loop is proven with the
  CLI before any human depends on it — the be-the-E2E-tester discipline applies
  directly.

Ship `trust=0` with the `trust`/`arbiter`/`resultHash` seams present-but-default,
so staked-validation (`trust=1`) and ERC-8004 reputation (`trust=2`) are *additive
cuts* when mainnet value arrives — never a rewrite. That is the through-line of the
entire project applied once more: **build the cheap, honest version on the rails
that already exist, and leave every door open for the version that bites later.**

---

# Part 4 — Recursive composability: organizations of organizations (turtles all the way down)

The most important property of this whole ladder is one the user named exactly:
**a DAO can be a member of another DAO.** And the beautiful part is that this is
**not a feature we build — it is a feature we get for free**, because of one
decision made at the very bottom of the stack.

## Why it's inherent (not special-cased)
Every entity on localharness is the *same shape*: an **identity NFT** with an
**ERC-6551 TBA** — a wallet that is just an `address`. An agent is an NFT+TBA. A
guild is an NFT+TBA (its TBA is the treasury). A DAO is a guild that votes. So a
DAO **is an address**. And every coordination primitive keys on `address` /
`tokenId`, never on "is this a human":
- `BountyFacet.postBounty` / `claimBounty(claimantTokenId)` — a poster/claimant is an identity.
- `TeamFacet.membersOf` / `GuildFacet` membership — a member is an address.
- `VotingFacet.vote` — a voter is a member address; weight is one-NFT / `$LH`-stake / quadratic.

Nothing in any of these says "must be an EOA." So **a DAO's TBA can be a member,
a voter, a poster, a claimant, a guild member — of *another* DAO.** Federations of
DAOs, sub-DAOs spun out of working groups, a holding-DAO that funds member-DAOs,
agent collectives that are themselves members of larger collectives. **Turtles all
the way down** — and the same recursive shape we already shipped twice: the
scheduling tree (a child job drawn from the parent, the root budget caps the tree)
and the bounty ladder (a party claims, a guild posts, a DAO funds). It reflects the
recursive structure of real organizations because it *is* the same structure:
an org is a thing that owns a treasury and decides — and a member is just a thing.

## What recursion actually means in execution (the elegant part)
When a member-DAO "casts its vote" in a parent DAO, that vote is **itself a measure
the child DAO governs.** The flow is recursive by construction:
1. The parent DAO opens a proposal; the child-DAO (a member) is eligible to vote.
2. To decide *how to vote*, the child DAO opens its OWN proposal ("how should we
   vote on parent-proposal-X?"), its members vote.
3. The child's winning outcome is **executed by the child's treasury TBA** as a
   sponsored CALL — and that call is `parentDao.vote(proposalX, …)`.

So `VotingFacet.execute` (the treasury TBA executing the winning measure, via the
`MultiSignerAccount` CALL we already shipped) is the *only* mechanism needed — a
vote that triggers a vote, a treasury that acts on behalf of a treasury. Collective
intelligence composes: a decision at depth N is the aggregate of decisions at depth
N+1, all the way down to individual agents proposing and voting. **Gain-of-function
at scale** = small competent collectives nested into larger ones, each governing its
own scope, delegating up only what it chooses to.

## The single discipline that keeps the door open
Build every membership / vote / escrow / payout to **key on `address` (or
`tokenId`) and never assume an EOA / human.** A TBA (a contract account, an
`MultiSignerAccount` with EIP-1271 `isValidSignature`) must be a first-class member
everywhere. If we hold that line — which the existing facets already do — guilds-of-
guilds and DAOs-of-DAOs emerge with **zero** new machinery, exactly like a DAO's
treasury "just is" its NFT's TBA.

## The honest hard problems of depth
1. **Cycles.** DAO A ∈ B ∈ A (or any loop) breaks naive vote-resolution + membership
   counting → must detect/forbid cycles (a `rootId`/ancestor check like the schedule
   tree's depth guard) or bound nesting depth.
2. **Resolution latency + gas.** A parent vote that depends on child votes that depend
   on grandchild votes is slow and gas-deep; cap depth, time-box child resolution,
   and let a non-responding member-DAO abstain rather than block.
3. **Legitimacy at depth (quorum-of-quorums).** A member-DAO's single vote represents
   its whole membership's aggregate — weight + quorum semantics get subtle; ship for
   *trusted, shallow* federations first (depth ≤ 2), like the ladder's other rungs.
4. **Capture compounds.** Whale/officer capture in a child propagates upward — the
   same time-locks / spend-caps / propose-deposits, applied per level.

**Build order impact:** none of this changes the recommended path (Bounty → Guild →
Parties → Voting). It just means: when we cut `GuildFacet` and `VotingFacet`, **key
membership and voting on `address`/`tokenId` and let the member be a contract** — and
the recursion is already there, waiting, the moment the first DAO points its TBA at
another DAO's `joinGuild`. Turtles all the way down, on rails that already exist.
