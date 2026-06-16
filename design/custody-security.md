# Fiat on-ramp custody & trust model

> STATUS: on-chain money core + test-mode proxy shipped; mainnet deploy + legal gated.
>
> Companion to `design/stripe-mainnet.md` (the SoT). That doc is the build plan;
> this one is the custody/trust contract — the invariant, the loss table, and the
> open legal decision. Read §6 (money safety) + §7 (red-team) of the SoT first.

The on-ramp sells USD on Stripe and mints `$LH` on Tempo. Everything below exists
to keep minted `$LH` USD-backed and to bound loss when something is compromised.

## Backing invariant

The single property the whole system protects, in USD cents:

```
circulating_$LH (totalSupply − diamond escrow)  ≤  usd_held_at_stripe / peg
```

- **On-chain side:** `MintGateFacet.circulatingSupply()` = `totalSupply −
  balanceOf(diamond)`. The diamond holds every fiat-mint as mint-to-self escrow,
  so `circulatingSupply()` is exactly the `$LH` that has escaped escrow into real
  buyer balances — the side of the invariant the chain can prove.
- **Off-chain side:** Stripe's settled-USD balance (NET of Stripe fees — mint
  against net, never gross), divided by the peg (`LH_PEG_WEI_PER_USD_CENT`,
  default `1e16` = 1 `$LH` per USD = `1e16` wei per cent).
- **Reconciliation alarm:** a periodic read-only job compares `circulatingSupply()`
  to the Stripe settled-USD balance and ALARMS on drift. Expected drift sources
  are bounded and explainable — rounding, Stripe fees, FX, partial refunds, an
  in-flight clawback. Unexplained drift = a leak; treat as an incident, not a
  rounding note. The alarm is read-only; it does not pause minting (that's the
  circuit-breaker, below).

## Loss enumeration

Each failure mode, what it can do, and what bounds it. "Residual" is the
accepted, post-mitigation exposure.

| Failure mode | Mechanism | Blast radius | Mitigation | Residual |
|---|---|---|---|---|
| **Leaked `fiatIssuerSigner`** | Attacker signs arbitrary `FiatMint` and submits `mintFromFiat` | Bounded by the **token global rolling-window cap** (C1) + the fiat-specific window + per-receipt cap + the lock. NOT `supplyCap` — the C1 fix moved the ceiling into `LocalharnessCredits._mint`, so every mint path (incl. a rogue cut facet) is bounded. | Key distinct from `PROXY_METER_KEY`, ideally KMS/HSM; issuer key only SIGNS, never mints directly; tight per-receipt + window caps; reconciliation alarm flags any `FiatMinted` with no matching settled payment | The cap is a FIXED (tumbling) window, so the worst case across a boundary is **≤2× cap per `windowSecs`** — size the cap at HALF the tolerable per-interval loss. All of it lands LOCKED (clawbackable until `unlockAt`); rotate via `setFiatIssuerSigner` |
| **Owner-key compromise** | Owner cuts a new mint path or loosens caps | Without the timelock, a same-block `setCap(∞)` + drain → `supplyCap`. With it, the attacker cannot widen the ceiling for **2 days** (`CAP_LOOSEN_TIMELOCK`). | Loosening (raise cap / uncap / shorten window) is timelocked via `proposeLoosenMintWindow` → wait 2d → `applyLoosenMintWindow`, with `cancelLoosenMintWindow`. **Tightening** (lower cap / longer window) is immediate via `tightenMintWindow`. | Whatever the *current* (un-loosened) window cap permits during the 2-day window in which defenders can `tighten`, cancel, and rotate the owner key |
| **Forged / replayed webhook** | Fake `checkout.session.completed` POST, or Stripe's legit retries | A forged body fails HMAC; a replay re-fires a real event | Raw-body HMAC (`stripe.webhooks.constructEvent`) + 5-min skew; one-shot `receiptId` derived ONLY from the immutable Stripe PaymentIntent/event id → idempotent mint, replay hits `ReceiptUsed` revert | None for forged (HMAC) / replayed (one-shot receipt); see C3 for the runtime trap that makes HMAC real |
| **Chargeback after spend/withdraw** | Buyer mints, spends-on-compute or (post-unlock) withdraws, THEN disputes the card | `clawbackFiatMint` can only burn what is **still locked**; already-spent / already-withdrawn `$LH` is gone | Long lock (≥ dispute window, below); metered spend drains the UNLOCKED portion first then locked, shrinking what a chargeback can recover; Radar + 3DS + first-buyer cap reduce fraud rate | The lock-window-bounded leak (H1): `$LH` spent-on-compute or withdrawn before a dispute lands. Accepted, bounded, treasury-reserve-covered |
| **Stripe freezes the USD reserve** | Stripe locks the settled-USD balance | Breaks backing for **all circulating fiat-`$LH` at once**; the on-chain side cannot detect or repair it | Sweep settled USD to a separate reserve (shrink the freezable float); circuit-breaker the webhook can check to pause `mintFromFiat` on a detected freeze; carry the circulating-fiat exposure as a documented liability | The full circulating-fiat exposure, until Stripe releases or the reserve covers redemption. Operational/legal, not on-chain-fixable |
| **Webhook on Edge runtime** | Edge parses/reframes the body → Stripe HMAC silently fails → a dev "fixes" it by skipping verify → open money-printer (C3) | Unbounded free mint | Webhook is a Vercel **NODE** function (raw body + `constructEvent`); deploy-time assert it is NOT Edge; test a byte-mutated body returns 400 | None if the Node-runtime assertion holds |

## Lock-window rationale

`mintFromFiat` records a `fiatLocked{amount, unlockAt}`. Until `unlockAt`, the
`$LH` is **spendable on compute** (via `meter`) but cannot be withdrawn to wallet,
transferred, or x402'd (`CreditMeterFacet.withdrawCredits` refuses the still-locked
portion; `withdrawableOf(user)` reports the movable amount).

- **Default `FIAT_LOCK_SECS` = 90 days.** Stripe card disputes can arrive up to
  ~120 days after the charge. A 90-day default narrows but does NOT close the gap —
  it is the build-time floor, owner-tunable up.
- **The gap is the H1 residual.** Any `$LH` spent-on-compute or withdrawn inside
  the lock-vs-dispute gap is unrecoverable by clawback. This is the accepted,
  bounded chargeback-fraud cost.
- **Closing it fully is a launch gate**, not a tweak. The launch posture is: a
  long lock (≥ dispute window, 100% for new/unverified buyers; shorten only after
  clean history) + a **funded treasury reserve sized to the 120-day worst case** +
  a required first-buyer mint cap + 3DS-over-threshold + the reconciliation alarm.
  Until the reserve is funded and sized, the gap is live risk.

These are owner-set on-chain (`setFiatLockSecs`, `setFiatMintWindow`,
`setPerReceiptMaxWei`) and proxy env (`LH_PEG_WEI_PER_USD_CENT`) values — NOT
hardcoded constants.

## Webhook enforcement (post-red-team)

The off-chain webhook is the money valve's other half; three invariants are
enforced in code (`proxy/api/stripe-webhook.ts`), each closing a confirmed
red-team finding:

- **Fail-closed NET (never gross).** Mint uses the charge's settled `net`
  (gross − Stripe fees). If `net` isn't known yet (async settlement / transient
  API error) the handler 500s and Stripe retries — it never falls back to gross,
  which would over-issue by the fee and breach the backing invariant. Checkout is
  card-only so net is normally settled by webhook time.
- **Durable clawback.** A refund/dispute that arrives BEFORE the mint tx lands
  (`!receipt.used`) returns 500 (retry) instead of `200`-ing the clawback away —
  Stripe re-delivers until the mint confirms, then the clawback fires. Dropping it
  would leave a minted-but-refunded receipt permanently unbacked.
- **Amount-aware, cumulative clawback.** `charge.refunded` fires on PARTIAL
  refunds too; the webhook passes the cumulative `amount_refunded` (peg-converted)
  to `clawbackFiatMint(receiptId, maxWei)`, which claws only the delta (capped at
  the receipt). A $10 refund of a $500 purchase burns ~$10 of credit, not all of
  it; disputes claw the full receipt (`maxWei=0`). The one-shot `receiptId` makes
  every retry idempotent.

## Key custody

- **`FIAT_ISSUER_KEY` is a dedicated hot EOA, distinct from `PROXY_METER_KEY`** (and
  from the sponsor key). Assert distinctness at proxy boot — sharing them means one
  leak compromises both minting and metering.
- **The issuer key only SIGNS** EIP-712 `FiatMint` payloads; it is not the
  `ISSUER_ROLE` holder and cannot mint directly. A proxy RCE leaks a
  cap-bounded signing oracle, not the raw mint authority. Ideally KMS/HSM so the
  raw key never sits in process memory.
- **Cap-raise is timelocked** (`CAP_LOOSEN_TIMELOCK = 2 days`). An owner-key
  compromise cannot do a same-block `setCap(∞)` + drain; defenders have a 2-day
  window to `tightenMintWindow` (immediate), cancel the pending loosen, and rotate.
- **EIP-712 binding.** Domain name `"localharness-mintgate"`, version `"1"`,
  `chainId`, `verifyingContract = diamond`; typehash `FiatMint(address to,uint256
  amount,bytes32 receiptId,uint256 validBefore)`. The chainId binding means a
  signature is invalid on the wrong chain — the seam MUST switch chainId atomically
  (testnet `42431` ↔ mainnet `4217`). Verify against the live
  `fiatMintDomainSeparator()` getter, never a hand-pinned hash.

## KYC / legal decision log — OPEN, gates go-live, maintainer-owned

This is **H2** from the SoT red-team — existential, unresolved, and the hard
go-live gate.

- **The problem:** `$LH` is currently **OPEN-loop** — `x402.settle`,
  `withdrawCredits`, and `send_lh` let it move to third parties and back to wallet.
  So the "closed-loop prepaid credits" framing is **false**, which puts fiat-origin
  `$LH` in stored-value / money-transmitter territory (MTL/MSB), with
  Stripe-termination and frozen-funds risk.
- **What the lock buys:** the fiat lock makes fiat-origin `$LH` closed-loop
  (spend-on-compute ONLY) **while locked**. After `unlockAt` it becomes
  transferable like any other `$LH` — so the lock defers the legal question, it
  does not answer it.
- **Two unresolved go-live options (pick one, maintainer-owned):**
  - **(a) Legal route** — obtain an MTL/MSB/stored-value legal analysis for the
    *actual* (transferable, third-party-payable) mechanics, plus confirm Stripe's
    TOS permits selling these credits at all, plus OFAC/sanctions screening.
  - **(b) Technical route** — enforce on-chain that fiat-origin `$LH` is a
    **permanently non-transferable, spend-on-compute-only balance class** that never
    reaches transfer / x402 / withdraw. This is much deeper than a time-lock (a new
    balance class threaded through every spend path), but it keeps the closed-loop
    framing true and shrinks regulatory surface.
- **Status:** unresolved. This decision GATES go-live regardless of how much of the
  on-chain/proxy plumbing is shipped.
