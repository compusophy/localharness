// stripe-webhook.ts — the fiat → $LH money valve's off-chain half.
//
// C3 (LAUNCH-BLOCKING) — HMAC MUST be verified over the RAW request bytes, and
// the verify must NEVER be skipped. On Edge the raw body is `await req.text()`
// (a Web Request — NO body parser to mutate the bytes, unlike a JSON-parsing
// framework), and Stripe verifies it with `constructEventAsync` +
// `createSubtleCryptoProvider` (WebCrypto). Do NOT switch to `req.json()` (that
// reframes the bytes → HMAC fails) and do NOT skip verification.
//
// Flow: verify HMAC → derive receiptId from the immutable PaymentIntent id →
// on `checkout.session.completed` sign an EIP-712 FiatMint (FIAT_ISSUER_KEY) for
// the NET settled amount and submit `mintFromFiat`; on `charge.refunded` /
// `charge.dispute.created` submit an amount-aware `clawbackFiatMint`. The
// on-chain receipt is the idempotency backstop: we read it first so a Stripe
// retry is a clean 200.

export const config = { runtime: 'edge' };

import {
  stripe,
  stripeCryptoProvider,
  receiptIdFor,
  signFiatMint,
  submitMintFromFiat,
  submitClawback,
  readReceipt,
  isHexAddress,
  centsToWei,
  tagCustomerLhAddress,
} from './_stripe';

function paymentIntentId(obj: { payment_intent?: unknown }): string | null {
  const pi = obj.payment_intent;
  if (typeof pi === 'string') return pi;
  if (pi && typeof pi === 'object' && typeof (pi as { id?: unknown }).id === 'string') {
    return (pi as { id: string }).id;
  }
  return null;
}

export default async function handler(req: Request): Promise<Response> {
  if (req.method !== 'POST') return new Response('method not allowed', { status: 405 });

  const secret = process.env.STRIPE_WEBHOOK_SECRET;
  if (!secret) return new Response('missing STRIPE_WEBHOOK_SECRET', { status: 500 });
  const sig = req.headers.get('stripe-signature');
  if (!sig) return new Response('missing stripe-signature', { status: 400 });

  let event: import('stripe').Stripe.Event;
  try {
    const raw = await req.text(); // RAW bytes — no parsing
    event = await stripe().webhooks.constructEventAsync(raw, sig, secret, undefined, stripeCryptoProvider);
  } catch (e) {
    // Bad signature OR a body that was mutated upstream — reject, never mint.
    return new Response('signature verification failed: ' + (e as Error).message, { status: 400 });
  }

  try {
    if (
      event.type === 'checkout.session.completed' ||
      event.type === 'checkout.session.async_payment_succeeded'
    ) {
      const session = event.data.object as import('stripe').Stripe.Checkout.Session;
      // A delayed-funding method (some bank-backed paths) completes 'unpaid' then
      // fires async_payment_succeeded on settlement — mint ONLY once settled-paid
      // (the one-shot receipt keeps both events idempotent). card + Link settle
      // synchronously, so this is normally 'paid' on the completed event.
      if (session.payment_status !== 'paid') {
        return json({ received: true, pending: true }, 200);
      }
      const lhAddress = session.metadata?.lh_address ?? '';
      const piId = paymentIntentId(session);
      if (!isHexAddress(lhAddress) || !piId) {
        return json({ received: true, skipped: 'no lh_address/payment_intent' }, 200);
      }
      await mintFiatForPi(piId, lhAddress);

      // Tier 2 enrolment: tag the Checkout-created Customer with lh_address so a
      // later off-session top-up can find its saved card. Best-effort — a tag
      // failure must NOT fail the webhook (the mint already succeeded); it only
      // means the buyer re-enters a card next time. Only present when off-session
      // is enabled (customer_creation:'always' in stripe-checkout.ts).
      const custId =
        typeof session.customer === 'string' ? session.customer : session.customer?.id;
      if (custId) {
        try {
          await tagCustomerLhAddress(custId, lhAddress);
        } catch {
          /* best-effort: the mint already landed */
        }
      }
    } else if (event.type === 'payment_intent.succeeded') {
      // Tier 2 OFF-SESSION top-up mint. CRITICAL anti-double-mint: a browser
      // Checkout ALSO fires payment_intent.succeeded, but its PI carries NO
      // lh_flow tag (Checkout mints via checkout.session.completed above) — so we
      // mint here ONLY for PIs explicitly tagged by /api/stripe-topup. This tag
      // gate is LOAD-BEARING; do not rely on receiptId alone (the two paths share
      // receiptIdFor(piId), so the tag is what keeps a Checkout PI out of here).
      const pi = event.data.object as import('stripe').Stripe.PaymentIntent;
      if (pi.metadata?.lh_flow !== 'offsession_topup') {
        return json({ received: true, skipped: 'not an off-session topup' }, 200);
      }
      const lhAddress = pi.metadata?.lh_address ?? '';
      if (!isHexAddress(lhAddress)) {
        return json({ received: true, skipped: 'no lh_address' }, 200);
      }
      await mintFiatForPi(pi.id, lhAddress);
    } else if (event.type === 'charge.refunded' || event.type === 'charge.dispute.created') {
      const obj = event.data.object as { payment_intent?: unknown; amount_refunded?: unknown };
      const piId = paymentIntentId(obj);
      if (piId) {
        const receiptId = receiptIdFor(piId);
        const r = await readReceipt(receiptId);
        if (!r.used) {
          // DURABILITY (red-team C2/HIGH): the mint tx hasn't landed yet (out-of-
          // order webhook / RPC lag). Do NOT 200 — that would silently DROP the
          // clawback forever. 500 → Stripe retries until the mint lands, then we
          // claw. (clawbackFiatMint reverts UnknownReceipt pre-mint, so retry.)
          return new Response('mint not yet landed for this receipt; retry', { status: 500 });
        }
        if (event.type === 'charge.dispute.created') {
          if (r.clawedWei < r.amount) await submitClawback(receiptId, 0n); // full
        } else {
          // Refund (possibly PARTIAL): claw only the cumulative refunded amount.
          const refundedCents = Number(obj.amount_refunded ?? 0);
          if (refundedCents > 0) {
            const targetWei = centsToWei(refundedCents);
            const capped = targetWei > r.amount ? r.amount : targetWei;
            if (capped > r.clawedWei) await submitClawback(receiptId, targetWei);
          }
        }
      }
    }
  } catch (e) {
    // On-chain submit / RPC failure: 500 so Stripe RETRIES. The receipt one-shot
    // makes the retried mint idempotent (we re-check `used` above first).
    return new Response('on-chain submit failed: ' + (e as Error).message, { status: 500 });
  }

  return json({ received: true }, 200);
}

function json(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

// Mint NET-settled $LH for a PaymentIntent against its idempotent on-chain
// receipt. Shared by the Checkout (checkout.session.completed) and the
// off-session top-up (payment_intent.succeeded) paths — both bind to the same
// receiptIdFor(piId) one-shot, so a replay (or a stray re-delivery) is a no-op.
// FAIL-CLOSED: netSettledCents THROWS if net isn't known yet → the caller 500s
// and Stripe retries; the receipt one-shot keeps the eventual retry idempotent.
// Never mints gross.
async function mintFiatForPi(piId: string, lhAddress: string): Promise<void> {
  const receiptId = receiptIdFor(piId);
  const r = await readReceipt(receiptId);
  if (r.used) return; // already minted — idempotent
  const netCents = await netSettledCents(piId);
  const amountWei = centsToWei(netCents);
  const validBefore = BigInt(Math.floor(Date.now() / 1000) + 3600);
  const signature = await signFiatMint(lhAddress, amountWei, receiptId, validBefore);
  await submitMintFromFiat(lhAddress, amountWei, receiptId, validBefore, signature);
}

// NET settled amount in cents: expand the PaymentIntent → latest charge →
// balance transaction `net` (gross minus Stripe fees). FAIL-CLOSED (red-team
// #4): if net isn't available yet (async settlement / transient API error) we
// THROW so the handler 500s and Stripe retries — minting GROSS would over-issue
// by the Stripe fee and permanently breach circulating ≤ usd_held/peg. The
// one-shot receiptId makes the eventual retry idempotent. Checkout uses card +
// Link (both settle synchronously), and the handler only mints once
// payment_status=='paid', so net is normally available by webhook time.
async function netSettledCents(piId: string): Promise<number> {
  const pi = await stripe().paymentIntents.retrieve(piId, {
    expand: ['latest_charge.balance_transaction'],
  });
  const charge = pi.latest_charge as import('stripe').Stripe.Charge | null;
  const bt = charge?.balance_transaction as import('stripe').Stripe.BalanceTransaction | null;
  if (bt && typeof bt.net === 'number' && bt.net > 0) return bt.net;
  throw new Error(`net settled amount not yet available for ${piId}; retry`);
}
