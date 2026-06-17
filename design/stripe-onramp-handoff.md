# Stripe fiat on-ramp — handoff report

> STATUS: open · written 2026-06-17 · for whoever picks this up next (CLI or agent).
> The on-ramp UX is rebuilt and live; the **payment→mint reliability** is fixed in
> code and deployed but **NOT yet confirmed end-to-end with a real payment**.

## TL;DR

- The browser "buy $LH" modal was rebuilt from Stripe **Embedded Checkout** (a full
  hosted page in a nested iframe — the "modal-in-modal", unscrollable, slow thing)
  to custom **Stripe Elements**. Shipped as **0.45.0**.
- A real bug surfaced in testing: **the card is charged but no `$LH` is minted.**
  Root cause = the "reactivity loop" never fired (details below). **Fixed + deployed**
  (commit `2f34187`, live bundle `be56d1ca9518`), **but the mint has not been
  observed working E2E yet** — a verification payment could not be completed via
  browser automation (synthetic clicks don't register inside Stripe's PCI iframe;
  manual clicks do). The owner reported "the first [payment] worked, the second
  didn't" — consistent with the pre-fix inconsistency.
- **Next concrete step: make ONE manual $1 payment on `*.localharness.xyz` and
  confirm the `$LH` balance credits.** If it does, the on-ramp is done. If not, see
  "If it still doesn't credit".

## Architecture (how a buy works now)

```
browser (admin → "buy $LH", or apex "buy $2 to claim")
  └─ events/credits.rs::buy_lh_pressed
       ├─ POST proxy /stripe/checkout {usd_cents, embedded:true}      (auth: personal-sign token)
       │     proxy api/stripe-checkout.ts → creates a BARE PaymentIntent
       │       (payment_method_types ['card','link'] — synchronous settlement only),
       │       metadata.lh_address bound from the AUTHENTICATED caller (never client input).
       │       returns { client_secret, payment_intent, lh_wei }
       ├─ open_buy_modal + call_js("lhBuyLh", {clientSecret})         → web/stripe-embed.js
       │     mounts NATIVE Stripe Elements: #lh-express (Link/Apple/Google Pay, one-click)
       │       + #lh-payment (Payment Element: inline Link "use this card" + card).
       │       NO custom pay button, NO "or pay with card" divider (owner's explicit ask).
       │       Express button confirm event → stripe.confirmPayment.
       │       Inline "use this card" / Link popup self-confirm WITHOUT our code.
       └─ poll_and_finalize(payment_intent)                           ← THE REACTIVITY LOOP
             phase 1: poll window.lhPaymentStatus() (client-side retrievePaymentIntent)
                      every 3s until status === 'succeeded' (catches EVERY confirm path).
             phase 2: POST proxy /stripe/finalize {payment_intent} with a FRESHLY signed
                      token (retries while NET not settled). On {minted:true} →
                      call_js("lhBuySuccess") + refresh balance.

proxy /stripe/finalize  (api/stripe-finalize.ts)
  auth + rate-limit → retrieve PI from Stripe (source of truth) → require status==succeeded
  AND metadata.lh_address == caller → mintSettledPayment() → on-chain mintFromFiat to the
  bound address for the NET-of-fees USD. Idempotent via the on-chain one-shot receipt.

proxy /stripe/webhook   (api/stripe-webhook.ts)  ← BACKSTOP
  payment_intent.succeeded → mintSettledPayment() (same idempotent mint).
  ALSO checkout.session.completed (hosted CLI path) + charge.refunded/dispute (clawback).

MintGateFacet.mintFromFiat (mainnet diamond)  → mints $LH, one-shot receipt per PI id.
```

## The bug + the fix (the important part)

**Symptom:** card charged, `$LH` never credited; modal stuck on "✓ payment received —
minting shortly".

**Root causes (all three):**
1. When the user pays via Stripe's **native** buttons (the Link popup, or the Payment
   Element's inline "use this card"), Stripe confirms the PaymentIntent **directly** —
   our `confirmPayment().then()` never resolves, so `finalize` was never called.
2. The instant-mint `finalize` call used the **modal-open auth token**. Stripe payment
   can take minutes (reading, retrying); past the proxy's **300s** freshness window the
   token 401s. The JS swallowed the error (`.catch(()=>{})`), so it failed **silently**.
3. There is **no webhook backstop** unless the Stripe endpoint is subscribed to
   **`payment_intent.succeeded`** (the bare-PI event). The old Embedded Checkout used
   `checkout.session.completed`; the new bare PaymentIntent never fires that. **This
   subscription state is UNCONFIRMED** (see open items).

**The fix (commit `2f34187`, live):** a Rust **reactivity loop**
(`events/credits.rs::poll_and_finalize`) that polls the PaymentIntent status
(client-side, cheap, path-independent) and, on `succeeded`, mints via `/stripe/finalize`
with a **freshly signed token** — eliminating both #1 and #2. #3 (webhook) remains a
should-do backstop for tab-close / 3DS-redirect.

## What is verified vs NOT

| Thing | State |
|---|---|
| Modal UX (Link-first, no divider, no custom button, compact, scrolls, fast) | ✅ verified live in Chrome |
| Proxy returns a PaymentIntent; finalize/webhook routes respond (204/401/405) | ✅ verified (curl) |
| Money-safety of the mint path (NET-only, fail-closed, idempotent, finalize auth/rate-limit) | ✅ adversarial review, no critical/high |
| Core app unaffected (chat streams, admin renders) | ✅ verified live |
| **Payment → on-chain mint actually credits `$LH` E2E** | ❌ **NOT confirmed** (automation can't click the PCI iframe; needs a manual payment) |
| Stripe webhook subscribed to `payment_intent.succeeded` | ❌ **unknown — check the Stripe dashboard** |

## Open items / next steps (ordered)

1. **Confirm the mint E2E.** Make ONE manual $1 buy on a `*.localharness.xyz` admin
   panel and watch the balance go from 0 → ~0.67 `$LH`. (Owner is `krafto`, wallet
   `0x8f731b4e6879879ee91b29b8a715ffea8b203e07`.) If it credits, the on-ramp is done.
2. **Subscribe the Stripe webhook to `payment_intent.succeeded`** (Dashboard →
   Developers → Webhooks → the localharness endpoint → add the event). This is the
   durable backstop for slow payers / closed tabs / 3DS redirects. To recover any
   already-stuck payment, "Resend" its `payment_intent.succeeded` event after
   subscribing (the on-chain receipt makes it idempotent).
3. **Cut 0.46.0** — the reactivity fix is committed to `main` but the published crate
   is still 0.45.0 (its source predates the fix). `./scripts/release.sh 0.46.0` after a
   CHANGELOG entry. (Pre-flight gotcha that bit 0.45.0: clippy 1.94's
   `items-after-test-module` — already fixed.)
4. **3DS-redirect return handler (not built).** `confirmPayment(redirect:'if_required')`
   sends 3DS cards to `?bought=1`; nothing reads that on load to finalize/confirm. Low
   priority (most Link/saved-card payments stay in-page), webhook covers minting.
5. **Owner-gated activation queue** (`design/README.md` "Awaiting maintainer
   activation") — all approved by the owner this session, none executed yet:
   metering go-live (`LH_TOKEN_METERING=1` + `LH_MARGIN_BPS=13000`, proxy redeploy +
   per-provider verify), x402 `settleUpto` recut (mainnet diamondCut — owner key IS
   the `.env EVM_PRIVATE_KEY = 0x313b…EF1e`), chain coherence, confirm-gate coverage.

## If it still doesn't credit (debug order)

1. In the buy modal, open devtools → Network. Does `/stripe/finalize` get POSTed after
   payment? What does it return? `{minted:true}` = good; `{minted:false}` = PI not
   succeeded/bound or net not settled (retries); 401 = stale token; 429 = rate-limited.
2. `window.lhPaymentStatus()` in the console — does it return `'succeeded'` after paying?
   If not, the payment didn't actually confirm.
3. Check the on-chain receipt / balance: `cast call 0x8ab4f3a5…f3a77 "balanceOf(address)"
   <addr> --rpc-url https://rpc.tempo.xyz`.
4. Proxy logs: `cd proxy && vercel logs <deployment>` — look for finalize/webhook errors
   (e.g. `FIAT_ISSUER_KEY` distinct-from-submitter assertion, RPC failures, no usage).

## Key facts

- **Live web**: `localharness.xyz` (apex) + `*.localharness.xyz`. Bundle `be56d1ca9518`.
  Deploy: `LH_MAINNET_SPONSOR_KEY=$(cat ~/.lh_sponsor_mainnet.key) ./scripts/build-web.sh`
  then `vercel deploy --prod --yes` (root).
- **Proxy** (separate Vercel project `proxy`): `proxy-tau-ten-15.vercel.app`. Deploy:
  `cd proxy && vercel --prod`. Routes: `/stripe/checkout`, `/stripe/finalize`,
  `/stripe/webhook`. Stripe `pk_live_` is in `web/stripe-embed.js` (public by design);
  `STRIPE_SECRET_KEY` / `FIAT_ISSUER_KEY` / `ONRAMP_SUBMITTER_KEY` /
  `STRIPE_WEBHOOK_SECRET` are proxy Vercel envs.
- **Mainnet** (chain 4217, `https://rpc.tempo.xyz`): diamond
  `0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77`, `$LH`
  `0x7ba3c9a39596e438b05c56dfc779700b58aea814`, peg 1 `$LH` = $1.
  Keys local at `~/.lh_*_mainnet.key` (issuer/submitter/sponsor), NOT committed.
  Diamond owner = `.env EVM_PRIVATE_KEY` = `0x313b1659…F348EF1e` (verified on-chain).
- **Files**: `web/stripe-embed.js` (Elements shim), `src/app/events/credits.rs`
  (`buy_lh_pressed`, `poll_and_finalize`, `finalize_mint`), `src/app/templates.rs`
  (`buy_modal`), `proxy/api/{stripe-checkout,stripe-finalize,stripe-webhook,_stripe}.ts`.
- **Commits**: `1875607` (Elements rebuild) · `09278fc` (release 0.45.0) · `2f34187`
  (reactivity-loop fix, current `main` HEAD).
- **Manual mint** (if ever needed to credit a stuck payment without re-charging):
  `MintGateFacet.mintFromFiat(to, amountWei, receiptId, validBefore, issuerSig)` —
  EIP-712 domain `{name:'localharness-mintgate', version:'1', chainId:4217,
  verifyingContract: diamond}`, sign with `~/.lh_fiat_issuer_mainnet.key`, submit with
  `~/.lh_submitter_mainnet.key`. (This is a real-money action — gate it on explicit
  owner authorization.)
