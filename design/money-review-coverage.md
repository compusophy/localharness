# Money-code adversarial review coverage (2026-06-16 autonomous loop)

> Provenance for the overnight hardening pass: every money-touching surface that
> was adversarially reviewed, the verdict, and the bugs found + fixed. Use this to
> avoid redundant re-review and to know what is/ isn't yet audited.

## Reviewed surfaces + verdicts

| Surface | Files | Verdict | Bugs fixed |
|---|---|---|---|
| x402 per-call metering (proxy) | `proxy/api/_x402.ts`, `gemini.ts` gate + caps | clean core; 2 MED | `a5edabe` |
| x402 client + SDK | `call.rs`, `backends/{gemini,anthropic}`, `agent.rs` | 1 MED | `a5edabe` |
| Redeem / invites / registration | `RedeemFacet.sol`, `invite.rs`, `credits.rs` | 1 MED | `a5edabe` |
| Fiat-mint core | `MintGateFacet.sol`, `_stripe.ts`, `stripe-webhook.ts`, `LocalharnessCredits.sol`, `CreditMeterFacet.sol` | 1 MED (partial-refund over-claw) | `bbe4ed5` |
| Buy / onboarding flows | `buy.rs`, `stripe-checkout.ts`, app buy-to-claim | clean | — |
| Scheduler | `ScheduleFacet.sol`, `proxy/api/scheduler.ts` | **1 HIGH** (budget hot-loop) | `4a372fa` |
| Guild treasury + DAO governance | `GuildFacet.sol`, `VotingFacet.sol`, `WeightedVotingFacet.sol` | **1 HIGH** (weighted snapshot-quorum bypass, un-deployed) | `334f41b` + test `5b0bc29` |
| Agent-to-agent x402 (`ask_agent`) | `proxy/api/mcp.ts` `handleAskAgent` + helpers | clean | — |
| Bounty + Party escrow/payout | `BountyFacet.sol`, `PartyFacet.sol` | clean | — |
| Validation staking escrow/payout | `ValidationFacet.sol` | clean core; 1 hardening applied | `(this tick)` |

**Totals: 10 surfaces reviewed; 6 bugs fixed (4 MED + 2 HIGH) + 1 defense-in-depth
hardening; 4 surfaces (buy/onboarding, ask_agent, bounty/party) and the 1m1v
voting path clean.**

## Notable findings (detail)
- **Scheduler hot-loop (HIGH, fixed live):** per-run debit capped to the stale
  start-of-run budget; a mid-run `scheduleChildJob` shrank the live budget →
  `recordRun` reverted `SpendExceedsBudget` → 'stale' → `nextRun` never advanced →
  the job re-fired every tick burning real upstream spend without debiting. Fixed
  by capping to the LIVE budget (re-read before `recordRun`).
- **Weighted-voting bypass (HIGH, un-deployed):** quorum denominator snapshotted
  at propose but ballot weight read live + `setShares` unguarded → an Admin could
  re-weight a voter mid-vote past quorum. Fixed by freezing the cap table for the
  voting window (`SharesLockedDuringVote`); regression-tested.
- **Fiat partial-refund over-claw (MED, fixed live):** clawed gross refunded cents
  vs a net-of-fees mint → over-burned the buyer. Fixed to proportional net.
- **ask_agent serve-then-dodge:** bounded to ONE model-cost per drained auth
  (funds pre-flight + one-shot nonce + `validBefore`), documented testnet policy —
  the payer's money is never wrongly taken; no amplification.
- **Bounty + Party (clean):** lifecycle states disjoint + terminal-guarded under
  CEI; escrow conserved (every wei refunded or paid); party split is exact
  (10000-bps, no dust/over-distribute); payouts bind to the deterministic claimed
  TBA (claim-squatting just pays the squatter); no reentrancy ($LH has no transfer
  callback). No bugs.
- **Validation self-deal (defense-in-depth, applied; recut deferred):** the resolve
  trust model is poster-is-oracle (intentional, same as BountyFacet) and the
  resolver is DISCLOSED via `validationResolverOf` — not a hidden vuln. Hardened
  anyway: a resolver who is also a disputant (`msg.sender == validator || ==
  challenger`) now reverts `ResolverIsDisputant`, forcing the owner-arbiter / draw
  path; the diamond owner (trusted platform arbiter) stays exempt. Source + 4
  regression tests landed (`ValidationResolverDisputant.t.sol`); the live facet
  still carries the documented boundary until the next ValidationFacet `diamondCut`
  (low-urgency — no honest party can be drained, the resolver is on-chain visible).

## NOT yet reviewed (mature, lower-probability; chain-independent logic)
- ReputationFacet (free attestations — no escrow, lowest money-risk).
- SessionRoom (#22) encrypted-KV append + the CRDT/AES cores.
- The rustlite compiler + cartridge runtime (chain-independent; agent-app-breaking
  if buggy, but not money).

## Cross-cutting OPEN items (owner decisions — gate further money work)
- **Chain coherence** (`design/chain-coherence.md`): CLI = testnet, web+proxy =
  mainnet → CLI agents can't transact on the live platform. THE top blocker.
- **Metering Option A** (`design/metering.md`): flat-per-request loses money;
  foundation built + tested (`_usage.ts`), wiring needs the margin + a supervised
  live tee. Option B caps shipped flag-off (enable via `LH_MAX_OUTPUT_TOKENS`).
