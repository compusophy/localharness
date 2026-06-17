# Security + quality audit — 2026-06-17

> Multi-agent audit (14 dimensions × adversarial verify) of the money/on-chain
> surface. 6 findings confirmed, 11 refuted. Scope: logic + money-safety only
> (no UI). Live on Tempo MAINNET (chain 4217), real fiat→$LH.

## Confirmed (action) — ALL FIXED + DEPLOYED 2026-06-17

| ID | Sev | Where | Status |
|----|-----|-------|--------|
| CONFIRM-1 | high | `spend_treasury` not in `CONFIRM_GATED` | ✅ fixed → web bundle `879bf42592cc` LIVE |
| VOTE-1 | high | `VotingFacet` 1m1v Officer sybil-flood → treasury drain | ✅ fixed → mainnet diamondCut LIVE (new facet `0x056Fc5e62Ca37cc57b20C000842E68B2c26A271F`) |
| METER-1 | high | `gemini.ts` burst race → free LLM inference (platform loss) | ✅ fixed → proxy redeployed LIVE |
| METER-2 | med | `gemini.ts` token-metering debit only in flush → disconnect = free call | ✅ fixed → proxy redeployed (closed before `LH_TOKEN_METERING` flip) |
| X402-1 | info | `settleUpto` implemented+tested but not cut into diamond | ✅ precondition note added; still MUST cut `settleUpto` before flipping `LH_TOKEN_METERING` |
| TEMPO-3 | info | sponsor fee-per-gas verbatim from node, no ceiling | ✅ fixed → web bundle `879bf42592cc` LIVE |

Deploys: proxy `proxy-tau-ten-15.vercel.app`, web `localharness.xyz` bundle
`879bf42592cc`, mainnet VotingFacet recut tx via
`ReplaceVotingFacetAdminGate.s.sol` (owner `0x313b…EF1e`, simulated then
broadcast; 626/626 tests green). Commits `3344c36`..`1a5f176`.

### CONFIRM-1 (FIXED) — `spend_treasury` bypassed the typed-confirm gate
`CONFIRM_GATED` listed only release/send_lh tools. `spend_treasury(guild_id, to,
amount_lh)` pays a model-supplied amount to a model-supplied recipient via an
**unconditional, non-refundable** on-chain transfer — same risk class as the
gated `send_lh`. The on-chain Admin check restricts WHO may spend, not WHETHER
the owner approved this payout, and the agent's own key IS the founding Admin.
A prompt-injected / poisoned-persona agent could drain a funded guild treasury
with no owner confirmation. The design doc's "escrow/refundable" rationale is
inaccurate for this path (it's a direct outbound transfer).
**Fix:** added `"spend_treasury"` to `CONFIRM_GATED` + a `confirmation` schema
field (mirrors `send_lh`). Escrow tools (`fund_*`/`post_bounty`) stay ungated
(refundable); `execute_proposal` stays ungated (quorum-gated — but see VOTE-1).

### VOTE-1 — Officer can sybil-flood members and drain the guild treasury
`VotingFacet.vote()` admits any **current** member; `propose()` snapshots only
the member **count** (the quorum denominator), not the eligible-voter **set**.
Membership growth is Officer-controlled (`inviteToGuild` is Officer+, accept is
free, no per-Officer cap, only `MAX_MEMBERS=128`). A malicious/compromised
Officer invites + self-accepts N controlled sybils, then `propose`s a
self-spend and votes Officer+N FOR (7 honest members + 6 sybils → snapshot 13,
quorum 7, exactly 7 FOR → passes → `_spendCore` transfers the treasury out).
This bypasses the Admin-only `spendTreasury` gate (proven by
`test_officer_cannot_spend_treasury`) — a privilege escalation that breaks the
Rung-4 member-governance promise. The snapshot-churn fix does NOT cover it (the
sybils are members at both snapshot and vote time). **`WeightedVotingFacet` is
immune** (share-gate: default-0-share sybils revert `NoVotingPower`).
Precondition: requires an Admin-granted Officer seat → high, not critical
(insider/compromised, not an arbitrary outsider). Value at risk = full treasury
of any 1m1v-governed guild with a non-Admin Officer (e.g. the live oggoel guild).
**Fix options:** (a) bind vote eligibility to the snapshot (record a per-member
join epoch/block; require `joinBlock < proposal.createBlock`); (b) per-Officer
invite cap; (c) make `WeightedVotingFacet` the recommended board for untrusted
membership + document Officer == de-facto treasury controller under 1m1v. (a) is
the real fix and is a money-critical `diamondCut` (owner-gated).

### METER-1 — burst race buys unbounded free LLM inference (platform loss)
`gemini.ts` gates on a lock-free `creditOf(addr) >= cost` read, then debits via
`meterDebit(addr, cost, false)` — `confirm=false` broadcasts the `meter()` tx
and returns **without awaiting the receipt**, so an on-chain revert is never
observed. N concurrent requests from one address read the same snapshot, all
pass, all stream, but only ~1 debit lands (the rest revert silently on-chain).
Net: fund `creditOf` with one call's price, fire a concurrent burst for the
most expensive model → N-1 free calls per burst, repeatable. User balance never
goes negative (contract reverts) ⇒ **direct platform loss**. The `confirm=true`
receipt-await path that would close this is **dead code** (both call sites pass
`false`). Worsened by `LH_MAX_OUTPUT_TOKENS` defaulting to 0 (uncapped output).
This route is the live primary usage path and has no rate limit (meter is the
sole defense). **Fix options:** await the receipt + 402 on revert; OR serialize
per-address debits; OR an in-isolate in-flight reservation per address; at
minimum cap per-address concurrency + default-enable `LH_MAX_OUTPUT_TOKENS`.

### METER-2 — token-metering debit lives in `flush()`; disconnect = free call
Latent (default-off `LH_TOKEN_METERING`, but the imminent go-live). When on,
the flat up-front debit branch is skipped and the ONLY debit is inside
`meteredBody`'s `flush()`, which does NOT run if the client disconnects before
stream end (`TransformStream.flush` runs on close, not on reader-abort). Abort
just before the terminal frame → full/partial response received, zero debit.
The `creditOf >= cost` gate is read-only and never consumed. Stronger than
METER-1 (no debit even attempted). The doc's "disconnect falls back to the flat
floor" is wrong — that fallback also runs inside `flush()`. **Fix BEFORE
flipping the flag:** broadcast a non-refundable floor debit up front and
reconcile usage in flush, OR use `waitUntil`/abort-signal to debit the floor on
disconnect. Never leave the sole debit on a client-skippable `flush()`.

### X402-1 (info) — `settleUpto` not cut into the diamond
`settleUpto` is implemented + tested but the only X402 cut script registers
just `settle`/`authorizationState`/`x402DomainSeparator`; calling `settleUpto`
reverts `FunctionNotFound`. Benign (revert, no loss), documented as a pending
owner-gated recut. Only matters if metering go-live relies on the Upto rail —
keep settlement on the exact `settle` path until the recut lands, and verify the
proxy doesn't submit `settleUpto` against the live diamond.

### TEMPO-3 (info) — no gas-price ceiling on sponsored txs
`run_sponsored_tempo_call` sets `max_fee == priority == eth_gasPrice()` with no
upper bound. A hostile/MITM'd RPC could inflate the sponsor's per-tx fee up to
`gas_limit * inflated_price`. Bounded (sponsor's small fee-token float; requires
RPC compromise). Hardening: clamp `max_fee_per_gas` to a sane multiple of a
known-good baseline and refuse to sign above it.

## Refuted (verified NOT exploitable — recorded so they aren't re-raised)

- **MINTGATE-1** — a verifier queried the LIVE diamond: all three mint caps are
  ARMED (token C1 = 1e23/day, fiat window = 5e22, per-receipt = 1e21). The
  script's 0-default is fail-open tooling, but the runbook sets finite caps and
  the credits script warns loudly on 0. Not a live exposure.
- **STRIPE-CLIENT-1** — swallowed client-side net-settle wait is cosmetic; the
  webhook `payment_intent.succeeded` backstop + one-shot on-chain receipt make
  the mint idempotent and eventually-consistent. Recipient/amount server-bound.
- **X402 (off-chain) replay / broadcast-only settle / unlimited allowance** —
  payer charged exactly once (on-chain one-shot nonce); serve-anyway leaks only
  sub-cent platform model spend; the unlimited allowance is to the diamond
  ITSELF and is payer-signature-gated. Accepted, documented.
- **ESCROW-1** (`withdrawTreasury` drains escrows) — owner-only, and the owner
  already has strictly greater power via `diamondCut`. EIP-2535 trust root, not
  a new bug.
- **TEMPO-1/2** (sponsor no-allowlist / no expiry) — bounded by the sponsor's
  small float (fee-only signature, can't move inner value); replay blocked by
  the sequential one-shot nonce.
- **CONFIRM-2** (escrow fund tools ungated) — refundable + owner-key-only;
  documented owner decision.
- **METER-3** (auth recover allows high-s) — harmless: malleation recovers the
  SAME address, and the token is already replayable within the 300s window with
  no nonce, so a second valid sig adds nothing.

## Clean (no findings)
Stripe proxy routes, MintGateFacet logic, X402Facet on-chain, Bounty/Party,
ERC-6551 TBAs, Schedule/Invite/Validation/Redeem, Credits/Meter/Tithe,
wallet crypto + encoding codecs, Tempo tx encoder.
