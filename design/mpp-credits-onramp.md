# MPP Credits as a $LH funding path — decision prep (2026-07-05)

**Decision:** should localharness accept Tempo's MPP Credits (card-funded,
Coinflow-issued agent credits, live since 2026-06-17) alongside the Stripe
on-ramp?

## Context — what we already have

Two money-in valves, ONE mint path (MintGateFacet `mintFromFiat`, issuer-signed,
one-shot receipt):

- **Stripe fiat on-ramp** — `proxy/api/stripe-{checkout,finalize,webhook}.ts` +
  `_stripe.ts`. Card → Elements → webhook → mint. $1..$500 band, $1 = 100 $LH.
  Human + browser required; iOS embedded checkout was reverted (OOM spiral).
- **MPP onramp (ALREADY LIVE)** — `proxy/api/mpp-onramp.ts` + `_mpp.ts`: an
  MPP-shaped 402 charge endpoint selling $LH for USDC.e at web parity
  (1 USDC.e = 100 $LH). Emits `WWW-Authenticate: Payment` (method `tempo`,
  intent `charge`), self-verifies the on-chain USDC.e transfer to
  `ONRAMP_TREASURY`, mints via the same MintGateFacet valve. Live-proven
  2026-07-02: 0.5 USDC.e → 50 $LH (settlement `0x710ad2e0…24ed`).

## Findings (sources fetched 2026-07-05)

- **MPP Credits** = "a new way for developers and agents to pay for MPP
  services using a credit or debit card." Bought at `wallet.tempo.xyz/agent`
  or `tempo wallet fund`; Coinflow ("a licensed payment service provider")
  processes cards (170+ countries, Apple/Google Pay, saved cards);
  "merchants receive settlement in USDC.e on Tempo within seconds."
  — https://tempo.xyz/blog/mpp-credits/
- **Merchant side needs no credits-specific API**: the blog indicates standard
  MPP 402 compliance suffices; mpp.dev's protocol spec lists only `tempo`
  (stablecoin) and `stripe` (card) production methods — **no distinct
  "credits"/"coinflow" method exists in the spec**. Credits are a BUYER-side
  funding rail; settlement arrives as ordinary USDC.e on Tempo.
  — https://mpp.dev/protocol/ , https://mpp.dev/quickstart/server
- **Coinflow posture**: "full merchant indemnification against fraud and
  chargebacks"; "One top-up, one wallet, accepted across every Tempo-proxied
  MPP service." — https://coinflow.cash/blog/tempo-coinflow-partnership/
- Merchant integration in the docs = mppx middleware (`tempo.charge({currency:
  USDC.e, recipient})` + a server-side `MPP_SECRET_KEY`); our onramp is a
  hand-rolled but wire-compatible equivalent, deliberately built so "the full
  mppx facilitator verify can be swapped in later" (`_mpp.ts` header).

## (a) What "accept MPP Credits" architecturally means

It reduces to: **make our EXISTING MPP-402 $LH endpoint payable by a
credits-funded Tempo Wallet agent.** No new facet, no new Vercel project, no
second mint valve. Two concrete gaps:

1. **Payer attribution.** `_mpp.ts::mintFromSettlement` credits the PROVEN
   on-chain payer (`from` of the USDC.e Transfer log) — the deliberate
   anti-replay fix. If Coinflow's settlement wallet (not the buyer) sends the
   on-chain transfer when credits are spent, the mint would go to Coinflow's
   address. Fix = adopt the official mppx verify (facilitator binds
   payment→challenge→caller) or bind our challenge id into the credential and
   attribute the mint to the challenge's authenticated caller.
2. **Challenge compatibility (UNVERIFIED).** Whether Tempo Wallet's credits
   spend accepts our hand-rolled challenge, or requires the exact mppx-served
   shape / registration as a "Tempo-proxied MPP service" (Coinflow's phrase —
   possibly a discovery/proxy prerequisite). Only a live test answers this.

## (b) Effort

- **S — spike (recommended first):** buy ~$5 of credits in wallet.tempo.xyz,
  `tempo request POST https://<proxy>/api/mpp-onramp` against the LIVE
  endpoint, observe the settlement tx + payer address. Zero code.
- **M — integration (if spike shows gaps):** swap `_mpp.ts` verify to mppx (or
  add challenge-bound attribution), update `mpp-onramp.ts` +
  `proxy/test/mpp-onramp.mjs`, redeploy proxy, optionally list the endpoint on
  MPPScan / the mpp.dev directory for discovery. Files: `proxy/api/_mpp.ts`,
  `proxy/api/mpp-onramp.ts`, `proxy/test/mpp-onramp.mjs`, `proxy/package.json`
  (mppx dep — Edge-compatible, proven on venture-mpp), docs. No contract
  changes; MintGateFacet untouched.
- **L is NOT on the table** — nothing requires a new facet or custody change.

## (c) What it buys vs Stripe

- **No card form, no browser**: an agent funds itself headlessly (card lives in
  Tempo Wallet). Kills the iOS-checkout class of pain for agent users.
- **New buyer pool**: any credits-holding agent in the Tempo ecosystem can buy
  $LH without ever holding stablecoins — agent-native demand channel, plus
  MPPScan/mpp.dev directory discovery of the onramp itself.
- **Chargeback posture improves**: Coinflow claims full merchant
  indemnification; on Stripe we bear disputes (`clawbackFiatMint` exists for
  refunds).
- Complements, not replaces, Stripe: Stripe stays the human/browser path (and
  is itself an MPP method for future use).

## (d) Risks / unknowns

- Credits settlement mechanics are publicly UNDOCUMENTED (who signs the
  on-chain transfer; no credits method in the spec) — empirical test required.
- "Tempo-proxied MPP service" may imply registration/proxying via Tempo infra;
  terms unknown. Coinflow merchant-side terms/fees/KYC not published — a
  Coinflow merchant account MIGHT be needed (nothing fetched says so, but
  nothing rules it out).
- Refund of a credits top-up after we minted $LH: indemnification suggests
  settlement is final, but unverified — worst case mirrors the Stripe
  `clawbackFiatMint` story.
- USDC.e custody unchanged: settles to `ONRAMP_TREASURY`, same as today.

## Options

1. **(Recommended) Spike, then close the gaps** — run the S spike against the
   live endpoint; if credits pay it, ship only the attribution fix (M);
   decide listing/discovery after. Low cost, answers every unknown with real
   money on the real rail.
2. **Full mppx adoption up front** — rewrite the onramp on mppx middleware for
   spec-exactness + future methods (Stripe method, sessions). More churn on a
   proven money valve before knowing it's needed.
3. **Do nothing** — Stripe + raw-USDC.e onramp already cover humans and
   funded agents; revisit when credits docs mature. Cedes the card-funded
   agent pool.

## Open questions (the spike answers 1–2)

1. Does a credits-funded `tempo request` pay our hand-rolled 402 challenge?
2. Whose address is `from` on the settlement USDC.e transfer?
3. Is "Tempo-proxied" registration required, and on what terms?
