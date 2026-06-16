// stripe-topup.ts — Tier 2: HEADLESS (off-session) card top-up for agents.
//
// The programmatic sibling of stripe-checkout.ts: an authenticated agent that
// has ALREADY saved a card (during a browser Checkout, setup_future_usage:
// 'off_session') can charge it without a browser. We create an off_session,
// confirm:true PaymentIntent on the buyer's saved card and tag it
// lh_flow='offsession_topup'; the MINT happens in stripe-webhook.ts on
// payment_intent.succeeded (reusing the SAME idempotent receipt + fiat-lock +
// clawback path as Checkout). This endpoint never mints inline.
//
// SAFETY: this whole rail is OFF unless LH_OFFSESSION_TOPUP_ENABLED==='1'. It
// stays dark until a money-transmitter / stored-value legal sign-off (see
// design/stripe-mainnet.md §7 H2) — recurring programmatic card pulls deepen
// that exposure beyond the one-time browser checkout. A per-address rolling 24h
// cap bounds the blast radius of a leaked auth token even once enabled.

export const config = { runtime: 'edge' };

import {
  stripe,
  verifyAuthToken,
  usdCentsToWei,
  findCustomerIdByLhAddress,
  rollingOffsessionCents,
} from './_stripe';

const MIN_USD_CENTS = Number(process.env.LH_MIN_USD_CENTS ?? '100'); // $1
const MAX_USD_CENTS = Number(process.env.LH_MAX_USD_CENTS ?? '50000'); // $500
// Per-address rolling 24h ceiling (a leaked auth token can otherwise pull the
// saved card repeatedly up to MAX). Default $50/day; env-overridable.
const DAILY_CAP_CENTS = Number(process.env.LH_OFFSESSION_DAILY_CAP_CENTS ?? '5000');
// Where the CLI sends a buyer to (re-)enter or re-auth a card in a browser.
const BUY_URL = process.env.LH_BUY_URL ?? 'https://localharness.xyz/?buy=1';

function json(body: unknown, status: number): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

export default async function handler(req: Request): Promise<Response> {
  if (req.method !== 'POST') return json({ error: 'method not allowed' }, 405);

  // SAFETY/LEGAL gate — dark until deliberately enabled (see header).
  if (process.env.LH_OFFSESSION_TOPUP_ENABLED !== '1') {
    return json({ error: 'off-session top-up is not enabled', save_url: BUY_URL }, 403);
  }

  // Auth — bind lh_address from the recovered signer, never a client field.
  const bearer = (req.headers.get('authorization') ?? '').replace(/^Bearer\s+/i, '');
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? bearer;
  let lhAddress: string;
  try {
    lhAddress = verifyAuthToken(token ?? '');
  } catch (e) {
    return json({ error: (e as Error).message }, 401);
  }

  let usdCents: number;
  let nonce: string | undefined;
  try {
    const body = JSON.parse((await req.text()) || '{}');
    usdCents = Number(body.usd_cents);
    if (typeof body.nonce === 'string' && body.nonce.length <= 80) nonce = body.nonce;
  } catch {
    return json({ error: 'invalid JSON body' }, 400);
  }
  if (!Number.isInteger(usdCents) || usdCents < MIN_USD_CENTS || usdCents > MAX_USD_CENTS) {
    return json({ error: `usd_cents must be an integer in [${MIN_USD_CENTS}, ${MAX_USD_CENTS}]` }, 400);
  }

  // Resolve the saved card's Customer; none → must do a browser checkout first.
  let customerId: string | null;
  try {
    customerId = await findCustomerIdByLhAddress(lhAddress);
  } catch (e) {
    return json({ error: 'stripe lookup failed: ' + (e as Error).message }, 502);
  }
  if (!customerId) {
    return json({ error: 'no saved card — add one with a browser checkout first', save_url: BUY_URL }, 409);
  }

  // Per-address rolling 24h cap.
  try {
    const since = Math.floor(Date.now() / 1000) - 86400;
    const spent = await rollingOffsessionCents(customerId, since);
    if (spent + usdCents > DAILY_CAP_CENTS) {
      return json(
        { error: `daily card limit reached (${DAILY_CAP_CENTS} cents/24h); spent ${spent}` },
        429,
      );
    }
  } catch (e) {
    return json({ error: 'cap check failed: ' + (e as Error).message }, 502);
  }

  const lhWei = usdCentsToWei(usdCents).toString();
  // Idempotency: a client-supplied nonce gives the caller full control; absent
  // it, a coarse minute bucket dedups an accidental network retry of the same
  // amount without merging two intentional distinct top-ups.
  const bucket = nonce ?? String(Math.floor(Date.now() / 60000));
  const idempotencyKey = `lh.topup:${lhAddress}:${usdCents}:${bucket}`;

  try {
    const pi = await stripe().paymentIntents.create(
      {
        amount: usdCents,
        currency: 'usd',
        customer: customerId,
        off_session: true,
        confirm: true,
        // The webhook mints ONLY when this exact tag is present (anti-double-mint).
        metadata: { lh_address: lhAddress, lh_flow: 'offsession_topup', lh_wei: lhWei },
      },
      { idempotencyKey },
    );
    if (pi.status === 'succeeded') {
      // Mint lands via the webhook (payment_intent.succeeded → mintFiatForPi).
      return json({ status: 'charged', lh_wei: lhWei, payment_intent: pi.id }, 200);
    }
    // requires_action / requires_payment_method without a throw — needs a browser.
    return json({ error: `card needs re-verification (status ${pi.status})`, reauth_url: BUY_URL }, 402);
  } catch (e) {
    const err = e as { code?: string; raw?: { code?: string }; message?: string };
    const code = err.code ?? err.raw?.code;
    if (code === 'authentication_required') {
      // SCA: the saved card requires interactive re-auth — an EXPECTED flow.
      return json({ error: 'card requires re-authentication', reauth_url: BUY_URL }, 402);
    }
    return json({ error: 'charge failed: ' + (err.message ?? String(e)) }, 402);
  }
}
