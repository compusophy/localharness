// stripe-webhook.ts — the fiat → $LH money valve's off-chain half.
//
// C3 (LAUNCH-BLOCKING): this MUST run on the Vercel NODE runtime with the RAW
// request body. On Edge / with a JSON body-parser the bytes are mutated and
// Stripe's HMAC verification silently fails — the copy-paste "fix" of skipping
// verify turns a forged POST into a free mint. So: runtime nodejs, bodyParser
// OFF, raw-body read, `constructEvent` over the exact bytes, and a boot guard
// that refuses to run anywhere `Buffer` is absent (i.e. Edge).
//
// Flow: verify HMAC → derive receiptId from the immutable PaymentIntent id →
// on `checkout.session.completed` sign an EIP-712 FiatMint (FIAT_ISSUER_KEY) for
// the NET settled amount and submit `mintFromFiat`; on `charge.refunded` /
// `charge.dispute.created` submit `clawbackFiatMint`. The on-chain receipt is
// the idempotency backstop: we read it first so a Stripe RETRY is a clean 200.

export const config = { runtime: 'nodejs', api: { bodyParser: false } };

import type { IncomingMessage, ServerResponse } from 'node:http';
import {
  stripe,
  receiptIdFor,
  signFiatMint,
  submitMintFromFiat,
  submitClawback,
  readReceipt,
  isHexAddress,
  centsToWei,
} from './_stripe';

async function readRawBody(req: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : (chunk as Buffer));
  }
  return Buffer.concat(chunks);
}

function paymentIntentId(obj: { payment_intent?: unknown }): string | null {
  const pi = obj.payment_intent;
  if (typeof pi === 'string') return pi;
  if (pi && typeof pi === 'object' && typeof (pi as { id?: unknown }).id === 'string') {
    return (pi as { id: string }).id;
  }
  return null;
}

export default async function handler(
  req: IncomingMessage & { method?: string },
  res: ServerResponse,
) {
  // C3 boot guard: never run on Edge (no Buffer ⇒ no reliable raw body).
  if (typeof Buffer === 'undefined') {
    res.statusCode = 500;
    res.end('stripe-webhook must run on the Node runtime');
    return;
  }
  if (req.method !== 'POST') {
    res.statusCode = 405;
    res.end('method not allowed');
    return;
  }

  const secret = process.env.STRIPE_WEBHOOK_SECRET;
  if (!secret) {
    res.statusCode = 500;
    res.end('missing STRIPE_WEBHOOK_SECRET');
    return;
  }
  const sig = req.headers['stripe-signature'];
  if (!sig || Array.isArray(sig)) {
    res.statusCode = 400;
    res.end('missing stripe-signature');
    return;
  }

  let event: import('stripe').Stripe.Event;
  try {
    const raw = await readRawBody(req);
    event = stripe().webhooks.constructEvent(raw, sig, secret); // HMAC over RAW bytes
  } catch (e) {
    // Bad signature OR a body that was mutated upstream — reject, never mint.
    res.statusCode = 400;
    res.end('signature verification failed: ' + (e as Error).message);
    return;
  }

  try {
    if (event.type === 'checkout.session.completed') {
      const session = event.data.object as import('stripe').Stripe.Checkout.Session;
      const lhAddress = session.metadata?.lh_address ?? '';
      const piId = paymentIntentId(session);
      if (!isHexAddress(lhAddress) || !piId) {
        // Nothing actionable (not a $LH purchase / missing binding) — ack so
        // Stripe stops retrying.
        res.statusCode = 200;
        res.end(JSON.stringify({ received: true, skipped: 'no lh_address/payment_intent' }));
        return;
      }
      const receiptId = receiptIdFor(piId);
      // Idempotency backstop: already minted ⇒ clean 200.
      const r = await readReceipt(receiptId);
      if (r.used) {
        res.statusCode = 200;
        res.end(JSON.stringify({ received: true, idempotent: true }));
        return;
      }
      // Mint against NET settled USD (fees out) so circulating ≤ usd_held/peg.
      // FAIL-CLOSED: if net isn't known yet, netSettledCents THROWS → 500 → the
      // one-shot receipt makes Stripe's retry idempotent. Never mint gross.
      const netCents = await netSettledCents(piId);
      const amountWei = centsToWei(netCents);
      const validBefore = BigInt(Math.floor(Date.now() / 1000) + 3600);
      const signature = await signFiatMint(lhAddress, amountWei, receiptId, validBefore);
      await submitMintFromFiat(lhAddress, amountWei, receiptId, validBefore, signature);
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
          res.statusCode = 500;
          res.end('mint not yet landed for this receipt; retry');
          return;
        }
        if (event.type === 'charge.dispute.created') {
          // Full chargeback → claw the whole receipt (maxWei=0).
          if (r.clawedWei < r.amount) await submitClawback(receiptId, 0n);
        } else {
          // Refund (possibly PARTIAL): claw only the cumulative refunded amount.
          // amount_refunded is Stripe's CUMULATIVE total refunded on the charge.
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
    res.statusCode = 500;
    res.end('on-chain submit failed: ' + (e as Error).message);
    return;
  }

  res.statusCode = 200;
  res.end(JSON.stringify({ received: true }));
}

// NET settled amount in cents: expand the PaymentIntent → latest charge →
// balance transaction `net` (gross minus Stripe fees). FAIL-CLOSED (red-team
// #4): if net isn't available yet (async settlement / transient API error) we
// THROW so the handler 500s and Stripe retries — minting GROSS would over-issue
// by the Stripe fee and permanently breach circulating ≤ usd_held/peg. The
// one-shot receiptId makes the eventual retry idempotent. Checkout restricts to
// card so net is normally settled by webhook time. USD settlement assumed
// (multi-currency FX is a reconciliation concern, not a mint-time one).
async function netSettledCents(piId: string): Promise<number> {
  const pi = await stripe().paymentIntents.retrieve(piId, {
    expand: ['latest_charge.balance_transaction'],
  });
  const charge = pi.latest_charge as import('stripe').Stripe.Charge | null;
  const bt = charge?.balance_transaction as import('stripe').Stripe.BalanceTransaction | null;
  if (bt && typeof bt.net === 'number' && bt.net > 0) return bt.net;
  throw new Error(`net settled amount not yet available for ${piId}; retry`);
}
