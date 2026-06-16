// prices.ts — public read-only GET /prices: the per-model $LH price table, so a
// client or the browser Usage panel can render per-model cost and pre-flight
// "can I afford one Opus call" WITHOUT hardcoding. Also the price an x402
// authorization must meet (the 402 challenge carries the same number). No auth,
// no chain state — pure config from _prices.ts. Short cache; values rarely move.

export const config = { runtime: 'edge' };

import { priceTable } from './_prices';
import { CHAIN_ID } from './_chain';

export default async function handler(req: Request): Promise<Response> {
  if (req.method === 'OPTIONS') {
    return new Response(null, {
      status: 204,
      headers: { 'Access-Control-Allow-Methods': 'GET, OPTIONS', 'Access-Control-Allow-Origin': '*' },
    });
  }
  if (req.method !== 'GET') {
    return new Response(JSON.stringify({ error: 'method not allowed' }), {
      status: 405,
      headers: { 'content-type': 'application/json' },
    });
  }
  // The x402 meter payee (LH_METER_PAYEE) lets a client sign an X-PAYMENT
  // authorization PROACTIVELY (read price + payee here, attach on the first
  // request — no 402 round-trip). null when x402 metering is off (the caller
  // then uses the session/creditOf path).
  const payee = (process.env.LH_METER_PAYEE ?? '').toLowerCase() || null;
  return new Response(
    JSON.stringify({
      asset: '$LH',
      decimals: 18,
      note: "price_wei is the $LH (18-decimal) cost of one metered request; '*' is the per-provider fallback for an unlisted model. To pay per-call, sign an x402 authorization for price_wei to x402.payTo and send it as the X-PAYMENT header.",
      x402: payee ? { payTo: payee, scheme: 'x402-exact', asset: '$LH', chainId: CHAIN_ID } : null,
      prices: priceTable(),
    }),
    {
      status: 200,
      headers: {
        'content-type': 'application/json',
        'access-control-allow-origin': '*',
        'cache-control': 'public, max-age=60',
      },
    },
  );
}
