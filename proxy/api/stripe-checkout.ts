// stripe-checkout.ts — create a Stripe Checkout Session for buying $LH, binding
// the buyer's localharness address at session-CREATE (from the AUTHENTICATED
// caller, never a buyer-editable field). NODE runtime (Stripe SDK + we read the
// raw body ourselves, so Vercel's body parser is irrelevant).
//
// Auth: the same `<address>:<timestamp>:<signature>` personal-sign token the
// gemini proxy uses, in x-goog-api-key / x-api-key / Authorization: Bearer.
// The recovered address is written to BOTH the session and the PaymentIntent
// metadata as `lh_address`; the webhook reads it from the trusted PaymentIntent.

export const config = { runtime: 'nodejs' };

import type { IncomingMessage, ServerResponse } from 'node:http';
import { stripe, verifyAuthToken, usdCentsToWei } from './_stripe';

const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';
// First-buyer / bill-shock bounds on a single purchase (env-overridable).
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

function setCors(res: ServerResponse, origin: string | undefined): void {
  res.setHeader('Access-Control-Allow-Methods', 'POST, OPTIONS');
  res.setHeader(
    'Access-Control-Allow-Headers',
    'content-type, x-goog-api-key, x-api-key, authorization',
  );
  res.setHeader('Vary', 'Origin');
  if (origin && isAllowedOrigin(origin)) res.setHeader('Access-Control-Allow-Origin', origin);
}

function send(res: ServerResponse, status: number, body: unknown): void {
  res.statusCode = status;
  res.setHeader('content-type', 'application/json');
  res.end(JSON.stringify(body));
}

async function readRawBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(typeof chunk === 'string' ? Buffer.from(chunk) : (chunk as Buffer));
  }
  return Buffer.concat(chunks).toString('utf8');
}

export default async function handler(req: IncomingMessage & { method?: string }, res: ServerResponse) {
  const origin = req.headers.origin as string | undefined;
  setCors(res, origin);

  if (req.method === 'OPTIONS') {
    res.statusCode = 204;
    res.end();
    return;
  }
  if (req.method !== 'POST') return send(res, 405, { error: 'method not allowed' });

  // Auth — bind lh_address from the AUTHENTICATED caller only.
  const h = req.headers;
  const bearer = String(h.authorization ?? '').replace(/^Bearer\s+/i, '');
  const token = String(h['x-goog-api-key'] ?? h['x-api-key'] ?? bearer ?? '');
  let lhAddress: string;
  try {
    lhAddress = verifyAuthToken(token);
  } catch (e) {
    return send(res, 401, { error: (e as Error).message });
  }

  let usdCents: number;
  try {
    const body = JSON.parse((await readRawBody(req)) || '{}');
    usdCents = Number(body.usd_cents);
  } catch {
    return send(res, 400, { error: 'invalid JSON body' });
  }
  if (!Number.isInteger(usdCents) || usdCents < MIN_USD_CENTS || usdCents > MAX_USD_CENTS) {
    return send(res, 400, { error: `usd_cents must be an integer in [${MIN_USD_CENTS}, ${MAX_USD_CENTS}]` });
  }
  // Surface what they'll receive so the UI can confirm before redirect.
  const lhWei = usdCentsToWei(usdCents).toString();

  try {
    const session = await stripe().checkout.sessions.create({
      mode: 'payment',
      // Card only: async methods (ACH/SEPA) settle later, so the webhook's NET
      // amount wouldn't be known at mint time (the webhook fails closed on that).
      payment_method_types: ['card'],
      line_items: [
        {
          price_data: {
            currency: 'usd',
            product_data: { name: 'localharness $LH credits' },
            unit_amount: usdCents,
          },
          quantity: 1,
        },
      ],
      success_url: SUCCESS_URL,
      cancel_url: CANCEL_URL,
      metadata: { lh_address: lhAddress, lh_wei: lhWei },
      payment_intent_data: { metadata: { lh_address: lhAddress, lh_wei: lhWei } },
    });
    return send(res, 200, { url: session.url, lh_wei: lhWei });
  } catch (e) {
    return send(res, 502, { error: 'stripe error: ' + (e as Error).message });
  }
}
