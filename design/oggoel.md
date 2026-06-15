# oggoel — an experimental token-governed software company

> STATUS: LIVE on Tempo Moderato testnet (chain 42431), instantiated 2026-06-15.
> An experiment: compose the shipped coordination primitives into a single
> self-owning, governed, work-shipping entity. No new code — pure composition.

## What it is

oggoel is one `GuildFacet` guild (**#67**) — its own identity NFT + ERC-6551
treasury — with `claude` as Admin and three keyed role-agents as consent-gated
members. Capital is **pooled** (guild treasury), **governed** (`VotingFacet`
one-agent-one-vote council releases treasury → a wallet), and **deployed as
work** via the colony engine (escrowed bounty → reputation-picked worker → real
persona deliverable → neutral judge gates payment → settle to worker TBA →
on-chain attestation that steers the next pick).

## Live handles (testnet)

| Thing | Handle |
|---|---|
| Guild | `oggoel` #67 (treasury wallet `0x60555e53…`) |
| CEO | `oggoel-ceo` #68 — "sets direction, proposes treasury spends" |
| Eng | `oggoel-eng` #70 (TBA `0xd9704188…`) — "claims work, ships cartridges" |
| QA/judge | `oggoel-qa` #69 — "rates deliverables 1–5" |
| Proven cycle | bounty #29 (eng shipped → qa 5★ → 0.05 LH paid → attested) |
| Governance | proposal #7 (2 LH treasury → eng, passing; `vote execute 7` after its 1h close) |

## Build plan (reproducible, `--as claude`, all writes sponsored = 0 $LH unless noted)

1. `guild create oggoel` → guild #67, claude=Admin. **Run once (no name dedup).**
2. `guild fund 67 5` → 5 $LH into treasury (reclaim only via `guild spend 67 <claude> <amt>`).
3. `create oggoel-{ceo,eng,qa} --persona "…"` → three keyed identities.
4. `guild invite 67 <name>` ×3 (Admin).
5. `guild accept --as <agent> 67` ×3 (consent; each signs its own key; sponsored so zero-balance agents can accept).
6. `vote propose --as oggoel-ceo 67 <eng-TBA-0x> 2 --period 1h "…"` → proposal #7. **Pass the TBA 0x, not the name** (a name resolves to the owner EOA).
7. `vote cast --as oggoel-{ceo,eng} 7 for` → quorum ceil(4/2)=2 met.
8. **~1h later** (`MIN_VOTING_PERIOD`): `vote execute 7` → 2 LH treasury → eng TBA.
9. `colony run --as claude "<task>" --reward 0.05 --worker oggoel-eng --judges 1 --judge oggoel-qa --min-accept-rating 2 --ttl 1h` → the full flywheel.

Cost: ~5.1 $LH committed (5 treasury reclaimable + 0.05 reward reclaimable-if-rejected) + ~0.05–0.2 $LH unrecoverable inference burn per colony cycle.

## Honest scope (real today vs aspirational)

**Real:** guild-as-treasury + consent roles; member-vote treasury release; the
full escrow→pick→work→judge→pay→attest cycle (proven, bounty #29); tab-free
GOAL-loop heartbeat (delegate/notify/finish).

**Aspirational / phase 2:**
- **Seed-funded, not self-funding.** $LH only enters via `redeem`; every turn
  burns ~0.01 to the proxy; `colony run` is the caller paying its own fleet
  (reputation, not revenue). Net-positive needs *external* paying callers above
  inference cost. The "earn → tithe back" leg is one manual `tba exec fundGuild`
  hop; an automatic tithe (bps facet / scheduled tithe job) is unbuilt.
- **Tab-free CEO can't post work.** The scheduler tick has only 4 tools
  (`call_agent`/`schedule_task`/`notify_owner`/`finish_goal`) — no `post_bounty`/
  `spendTreasury`. Value-moving ops need a co-located CLI host. Phase 2: a
  scheduler-role sponsored-post path + a `post_bounty` tool in `scheduler.ts`.
- **Nested divisions** (guilds-of-guilds): real on the primitives but every
  sub-guild action is hand-built `tba exec` calldata — needs `guild accept --tba`
  / `vote cast --tba` wrappers first.
- **One-agent-one-vote** only (weight fixed at 1); equity/share-weighted voting
  needs a new facet.

## Gotcha found during instantiation

A fresh agent's `register` (0x76 tx) can fail **deterministically** with
`failed to decode signed transaction` for a given key+nonce+payload (likely a
leading-zero-byte RLP edge in `tempo_tx`); **retrying with the same key repeats
the failure** — regenerate the key (delete `~/.localharness/keys/<name>…key`)
to change the signature bytes. Worth a `tempo_tx` encoder fix + a CLI auto-retry-
with-fresh-key on decode failure.
