// stripe-finalize.ts — instant, idempotent mint right after the browser's
// Stripe Elements `confirmPayment` resolves. The client POSTs the PaymentIntent
// id; we re-read the PI from Stripe (the source of truth), verify it SUCCEEDED
// and is bound to the AUTHENTICATED caller, then mint NET via the shared
// one-shot-receipt path. This removes the happy-path dependency on the webhook's
// `payment_intent.succeeded` subscription; the webhook remains the durable
// backstop for tab-close / 3DS-redirect returns. Both paths share the on-chain
// receipt, so whichever lands first wins and the other is a clean no-op.
//
// Money-safety: the mint RECIPIENT and AMOUNT come ONLY from the trusted Stripe
// PI (`metadata.lh_address` set server-side at create + GROSS charged cents) and
// the on-chain receipt — never from client input. The caller can at most
// ACCELERATE a mint the webhook would do anyway, for their OWN payment.

export const config = { runtime: 'edge' };

import {
  stripe,
  verifyAuthToken,
  mintSettledPayment,
  receiptIdFor,
  readReceipt,
} from './_stripe';
import { SlidingWindow, claimedAddress } from './_ratelimit';

const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

// Best-effort per-isolate cap (see _ratelimit.ts): a cheap first line before the
// outbound Stripe retrieve / RPC read, so a replayable token can't drive
// unbounded Stripe-quota / RPC burn. The on-chain one-shot receipt stays the
// global money backstop. A legit buyer finalizes once per purchase; 12/min is
// generous for a flaky retry.
const FINALIZE_PER_MIN = 12;
const finalizeWindow = new SlidingWindow(FINALIZE_PER_MIN, 60_000);

function isAllowedOrigin(origin: string): boolean {
  if (origin === ALLOWED_ORIGIN_EXACT || origin.endsWith(ALLOWED_ORIGIN_SUFFIX)) return true;
  try {
    const u = new URL(origin);
    return u.protocol === 'http:' && (u.hostname === 'localhost' || u.hostname === '127.0.0.1');
  } catch {
    return false;
  }
}

function cors(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key, authorization',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) h['Access-Control-Allow-Origin'] = origin;
  return h;
}

function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...cors(origin) },
  });
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors(origin) });
  if (req.method !== 'POST') return json({ error: 'method not allowed' }, 405, origin);

  // Auth — the same personal-sign token the gemini/checkout routes use. Binds
  // the request to a caller; we then require the PI to be bound to that caller.
  const bearer = (req.headers.get('authorization') ?? '').replace(/^Bearer\s+/i, '');
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? bearer ?? '';

  // Rate limit PRE-auth on the CLAIMED address (gates nothing of value — the
  // mint is bound + idempotent downstream — just bounds wasted Stripe/RPC work).
  const claimed = claimedAddress(token);
  if (claimed) {
    const wait = finalizeWindow.hit(claimed);
    if (wait > 0) {
      return new Response(JSON.stringify({ error: 'rate limited' }), {
        status: 429,
        headers: { 'content-type': 'application/json', 'retry-after': String(wait), ...cors(origin) },
      });
    }
  }

  let caller: string;
  try {
    caller = verifyAuthToken(token);
  } catch (e) {
    return json({ error: (e as Error).message }, 401, origin);
  }

  let piId: string;
  try {
    const body = JSON.parse((await req.text()) || '{}');
    piId = String(body.payment_intent ?? '');
  } catch {
    return json({ error: 'invalid JSON body' }, 400, origin);
  }
  if (!/^pi_[A-Za-z0-9]{8,255}$/.test(piId)) {
    return json({ error: 'invalid payment_intent' }, 400, origin);
  }

  // Stripe is the source of truth: re-read the PI (never trust client status).
  // OPAQUE responses below: a caller must not be able to use this endpoint as an
  // oracle for the existence / paid-state of PaymentIntents they don't own. An
  // unknown PI, a not-yet-succeeded PI, and a succeeded PI bound to SOMEONE ELSE
  // all return the SAME `{minted:false}`. Only the OWNER of a SUCCEEDED PI mints.
  let pi: import('stripe').Stripe.PaymentIntent;
  try {
    pi = await stripe().paymentIntents.retrieve(piId);
  } catch {
    return json({ minted: false }, 200, origin);
  }
  const bound = String(pi.metadata?.lh_address ?? '').toLowerCase();
  if (pi.status !== 'succeeded' || !bound || bound !== caller.toLowerCase()) {
    return json({ minted: false }, 200, origin);
  }

  try {
    const out = await mintSettledPayment(pi.id, bound);
    return json(out, 200, origin);
  } catch {
    // Net-not-yet-settled / transient RPC / a nonce race with the webhook path.
    // If the racing webhook mint actually LANDED, report it honestly instead of
    // a misleading `pending`; else the webhook backstop will mint. Never mint
    // gross here.
    try {
      const r = await readReceipt(receiptIdFor(pi.id));
      if (r.used) return json({ minted: true, idempotent: true }, 200, origin);
    } catch {
      /* receipt read failed too — fall through to pending */
    }
    return json({ minted: false, pending: true }, 200, origin);
  }
}
