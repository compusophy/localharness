// prices.ts — public read-only GET /prices: the per-model $LH price table, so a
// client or the browser Usage panel can render per-model cost and pre-flight
// "can I afford one Opus call" WITHOUT hardcoding. Also the price an x402
// authorization must meet (the 402 challenge carries the same number). No auth,
// no chain state — pure config from _prices.ts. Short cache; values rarely move.

export const config = { runtime: 'edge' };

import { priceTable } from './_prices';

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
  return new Response(
    JSON.stringify({
      asset: '$LH',
      decimals: 18,
      note: "price_wei is the $LH (18-decimal) cost of one metered request; '*' is the per-provider fallback for an unlisted model",
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
