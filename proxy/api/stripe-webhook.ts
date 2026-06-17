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
  mintSettledPayment,
  submitClawback,
  readReceipt,
  isHexAddress,
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
      // Idempotent NET mint (fees out); THROWS if net isn't settled yet → outer
      // catch 500s → Stripe retries; the one-shot receipt keeps it idempotent.
      await mintSettledPayment(piId, lhAddress);
    } else if (event.type === 'payment_intent.succeeded') {
      // The browser Stripe Elements path drives a BARE PaymentIntent (no
      // Checkout Session), so this is its mint trigger. The hosted Checkout
      // path ALSO fires this — the on-chain one-shot receipt keeps the
      // double-fire idempotent. `lh_address` is bound server-side at PI-create
      // (never a buyer field), so the mint can only ever credit the buyer.
      const pi = event.data.object as import('stripe').Stripe.PaymentIntent;
      const lhAddress = (pi.metadata?.lh_address as string | undefined) ?? '';
      if (isHexAddress(lhAddress)) {
        await mintSettledPayment(pi.id, lhAddress);
      }
    } else if (event.type === 'charge.refunded' || event.type === 'charge.dispute.created') {
      const obj = event.data.object as {
        payment_intent?: unknown;
        amount_refunded?: unknown;
        amount?: unknown;
      };
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
          // Refund (possibly PARTIAL). The mint was NET of Stripe fees
          // (r.amount = net), but Stripe's amount_refunded is GROSS, so clawing
          // centsToWei(grossRefunded) over-burns the buyer by the fee share of
          // the refund. Claw the PROPORTIONAL net amount instead: the cumulative
          // net-refunded = r.amount × (cumulative gross refunded ÷ gross charge).
          // amount_refunded + amount are both cumulative/gross on the Charge.
          const refundedCents = Number(obj.amount_refunded ?? 0);
          const grossCents = Number(obj.amount ?? 0);
          if (refundedCents > 0 && grossCents > 0) {
            const proportional = (r.amount * BigInt(refundedCents)) / BigInt(grossCents);
            // Cap at the full net mint; submit the SAME value we compare against
            // (avoids redundant reverting resubmits once the cumulative is met).
            const targetWei = proportional > r.amount ? r.amount : proportional;
            if (targetWei > r.clawedWei) await submitClawback(receiptId, targetWei);
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
