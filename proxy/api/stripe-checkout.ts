// stripe-checkout.ts — create a Stripe Checkout Session for buying $LH, binding
// the buyer's localharness address at session-CREATE (from the AUTHENTICATED
// caller, never a buyer-editable field). EDGE runtime (matches the rest of the
// proxy; Stripe uses the fetch http client).
//
// Auth: the same `<address>:<timestamp>:<signature>` personal-sign token the
// gemini proxy uses, in x-goog-api-key / x-api-key / Authorization: Bearer. The
// recovered address is written to BOTH the session and the PaymentIntent
// metadata as `lh_address`; the webhook reads it from the trusted PaymentIntent.

export const config = { runtime: 'edge' };

import { stripe, verifyAuthToken, usdCentsToWei } from './_stripe';

const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';
const MIN_USD_CENTS = Number(process.env.LH_MIN_USD_CENTS ?? '100'); // $1
const MAX_USD_CENTS = Number(process.env.LH_MAX_USD_CENTS ?? '50000'); // $500
const SUCCESS_URL = process.env.LH_CHECKOUT_SUCCESS_URL ?? 'https://localharness.xyz/?bought=1';
const CANCEL_URL = process.env.LH_CHECKOUT_CANCEL_URL ?? 'https://localharness.xyz/?cancelled=1';

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

  // Auth — bind lh_address from the AUTHENTICATED caller only.
  const bearer = (req.headers.get('authorization') ?? '').replace(/^Bearer\s+/i, '');
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? bearer;
  let lhAddress: string;
  try {
    lhAddress = verifyAuthToken(token ?? '');
  } catch (e) {
    return json({ error: (e as Error).message }, 401, origin);
  }

  let usdCents: number;
  let embedded = false;
  try {
    const body = JSON.parse((await req.text()) || '{}');
    usdCents = Number(body.usd_cents);
    embedded = body.embedded === true;
  } catch {
    return json({ error: 'invalid JSON body' }, 400, origin);
  }
  if (!Number.isInteger(usdCents) || usdCents < MIN_USD_CENTS || usdCents > MAX_USD_CENTS) {
    return json(
      { error: `usd_cents must be an integer in [${MIN_USD_CENTS}, ${MAX_USD_CENTS}]` },
      400,
      origin,
    );
  }
  const lhWei = usdCentsToWei(usdCents).toString();

  // Tier 2 (off-session) enrolment is gated by the SAME flag as the topup
  // endpoint: only when ON do we create a Customer + save the card with an
  // off-session mandate, so deploying this file changes NOTHING about the live
  // one-time checkout until off-session is deliberately enabled (post legal
  // sign-off). The webhook tags the created Customer with lh_address.
  const offSession = process.env.LH_OFFSESSION_TOPUP_ENABLED === '1';

  // card + Link only — both settle synchronously (the webhook's NET amount is
  // ready at mint time; it fails closed otherwise). Link gives returning buyers
  // one-click saved cards.
  const base = {
    mode: 'payment' as const,
    payment_method_types: ['card', 'link'] as Array<'card' | 'link'>,
    line_items: [
      {
        price_data: {
          currency: 'usd' as const,
          product_data: { name: 'localharness $LH credits' },
          unit_amount: usdCents,
        },
        quantity: 1,
      },
    ],
    metadata: { lh_address: lhAddress, lh_wei: lhWei },
    payment_intent_data: {
      metadata: { lh_address: lhAddress, lh_wei: lhWei },
      // Save the card for headless top-ups later (off-session mandate).
      ...(offSession ? { setup_future_usage: 'off_session' as const } : {}),
    },
    // Create a Customer so the saved card can be charged off-session by address.
    ...(offSession ? { customer_creation: 'always' as const } : {}),
  };

  try {
    if (embedded) {
      // Embedded Checkout: rendered INSIDE our branded modal (no redirect). We
      // handle completion client-side, so no return_url is needed.
      const session = await stripe().checkout.sessions.create({
        ...base,
        ui_mode: 'embedded',
        redirect_on_completion: 'never',
      });
      return json({ client_secret: session.client_secret, lh_wei: lhWei }, 200, origin);
    }
    // Hosted Checkout (redirect) — kept for non-browser callers.
    const session = await stripe().checkout.sessions.create({
      ...base,
      success_url: SUCCESS_URL,
      cancel_url: CANCEL_URL,
    });
    return json({ url: session.url, lh_wei: lhWei }, 200, origin);
  } catch (e) {
    return json({ error: 'stripe error: ' + (e as Error).message }, 502, origin);
  }
}
