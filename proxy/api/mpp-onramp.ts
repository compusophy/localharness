// mpp-onramp.ts — Tempo MPP charge endpoint: an autonomous agent pays USDC.e and
// gets $LH minted at parity, no human, no card (design/cli-mainnet-onboarding.md
// C-2). The crypto-native sibling of /stripe/finalize.
//
// Flow (MPP charge intent == x402 "exact"):
//   1. Agent POSTs WITHOUT a payment credential → 402 + WWW-Authenticate: Payment
//      challenge (method "tempo", intent "charge", base64url `request` quoting
//      USDC.e price + our treasury recipient).
//   2. Agent pays the quoted USDC.e to the treasury on Tempo, then RETRIES with
//      `Authorization: Payment payload="<b64url {settlementTx, payTo}>"`.
//   3. We VERIFY the on-chain USDC.e transfer ourselves (recipient == treasury,
//      amount, confirmed; replay-protected by the settlement tx hash one-shot)
//      and GROSS-mint $LH into payTo's meter via the SAME MintGateFacet valve the
//      Stripe webhook uses → 200 + Payment-Receipt header.
//
// All money logic lives in the reusable _mpp.ts lego; this file is just the HTTP
// shell (CORS, auth, rate limit, the 402<->200 dance). The minted amount comes
// ONLY from the on-chain USDC.e amount — never from client input.

export const config = { runtime: 'edge' };

import { verifyAuthToken, isHexAddress } from './_stripe';
import {
  buildChallenge,
  challengeHeader,
  challengeBody,
  parseCredential,
  mintFromSettlement,
  lhWeiToUsdceUnits,
  treasuryAddress,
  LH_WEI_PER_USDCE,
  receiptIdForTx,
} from './_mpp';
import { SlidingWindow, claimedAddress } from './_ratelimit';

const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

// Min/max $LH a single charge may mint, in whole $LH (env-tunable). Mirrors the
// Stripe checkout's $1..$500 band: default 100 $LH (== 1 USDC.e) min, 50000 $LH
// (== 500 USDC.e) max. The agent picks an amount inside the band; we quote the
// USDC.e to pay at parity.
const MIN_LH = BigInt(process.env.MPP_MIN_LH ?? '100');
const MAX_LH = BigInt(process.env.MPP_MAX_LH ?? '50000');
const ONE_LH_WEI = 1_000_000_000_000_000_000n;

// Cheap per-isolate cap before outbound RPC / mint (see _ratelimit.ts). The
// on-chain one-shot receipt is the global money backstop; this just bounds wasted
// RPC burn from a replayable token.
const PER_MIN = Number(process.env.MPP_RATE_PER_MIN ?? '12');
const window = new SlidingWindow(PER_MIN, 60_000);

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
    'Access-Control-Allow-Headers':
      'content-type, x-goog-api-key, x-api-key, authorization',
    // Let a browser-side agent read the payment headers off the 402 / 200.
    'Access-Control-Expose-Headers': 'WWW-Authenticate, Payment-Receipt',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) h['Access-Control-Allow-Origin'] = origin;
  return h;
}

function json(
  body: unknown,
  status: number,
  origin: string | null,
  extra: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...cors(origin), ...extra },
  });
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors(origin) });
  if (req.method !== 'POST') return json({ error: 'method not allowed' }, 405, origin);

  // Auth — the same personal-sign token the gemini/stripe routes use, in
  // x-goog-api-key / x-api-key. NOTE the standard `Authorization` header is
  // RESERVED here for the MPP `Payment` credential (below), so unlike the stripe
  // routes the identity token must NOT ride in Authorization. Binds the request
  // to a caller; the DEFAULT $LH recipient is that authenticated caller.
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';

  // Rate limit PRE-auth on the claimed address (gates nothing of value — the mint
  // is on-chain-bound + idempotent — just bounds wasted RPC work).
  const claimed = claimedAddress(token);
  if (claimed) {
    const wait = window.hit(claimed);
    if (wait > 0) {
      return json({ error: 'rate limited' }, 429, origin, { 'retry-after': String(wait) });
    }
  }

  let caller: string;
  try {
    caller = verifyAuthToken(token);
  } catch (e) {
    return json({ error: (e as Error).message }, 401, origin);
  }

  // Treasury must be configured or the whole valve is closed (no safe default —
  // an unset recipient must never mint). Fail fast with a clear config error.
  try {
    treasuryAddress();
  } catch (e) {
    return json({ error: (e as Error).message }, 500, origin);
  }

  let body: Record<string, unknown>;
  try {
    body = JSON.parse((await req.text()) || '{}');
  } catch {
    return json({ error: 'invalid JSON body' }, 400, origin);
  }

  // Desired mint amount in whole $LH (default the minimum). The $LH recipient is
  // the authenticated caller unless an explicit on-behalf address is given (still
  // a 20-byte address; the mint is bound to it, never to caller-spoofable value).
  let lhAmount = MIN_LH;
  const rawLh = body.lh_amount ?? body.lhAmount;
  if (rawLh !== undefined) {
    try {
      lhAmount = BigInt(String(rawLh));
    } catch {
      return json({ error: 'lh_amount must be an integer (whole $LH)' }, 400, origin);
    }
  }
  if (lhAmount < MIN_LH || lhAmount > MAX_LH) {
    return json({ error: `lh_amount must be in [${MIN_LH}, ${MAX_LH}] whole $LH` }, 400, origin);
  }
  const payTo =
    typeof body.pay_to === 'string' && isHexAddress(body.pay_to)
      ? (body.pay_to as string).toLowerCase()
      : caller.toLowerCase();

  const resource = new URL(req.url).origin + '/mpp/onramp';

  // --- step 1: no credential -> 402 challenge --------------------------------
  let credential;
  try {
    credential = parseCredential(req.headers.get('authorization'));
  } catch (e) {
    return json({ error: (e as Error).message }, 402, origin);
  }

  if (!credential) {
    // Quote the USDC.e to pay for the requested $LH at parity. Decimals are read
    // on-chain at verify time; the quote uses 6 (USDC.e) so the advertised base
    // units match what the buyer transfers.
    const lhWei = lhAmount * ONE_LH_WEI;
    const usdceUnits = lhWeiToUsdceUnits(lhWei, 6);
    const ch = buildChallenge({ usdceUnits, resource });
    return json(challengeBody(ch), 402, origin, {
      'WWW-Authenticate': challengeHeader(ch),
    });
  }

  // --- step 2: credential present -> verify on-chain + mint ------------------
  // SECURITY: the mint recipient is the PROVEN on-chain USDC.e payer of the
  // settlement tx (resolved inside mintFromSettlement), NOT the caller-supplied
  // `payTo`/`pay_to` — otherwise anyone could replay another party's (or any
  // treasury-inbound) settlement and mint the $LH to themselves. `payTo` is
  // advisory only; the mint follows the money. The authenticated caller normally
  // IS the payer (an agent pays from its own identity).
  const out = await mintFromSettlement(credential.settlementTx, payTo);
  if (!out.minted) {
    // 402 = "still owed / not yet verifiable" (retry after confirmation); other
    // statuses are config/RPC faults.
    return json({ minted: false, error: out.error }, out.status, origin);
  }

  // 200 + Payment-Receipt: the mint succeeded (or was already done). The receipt
  // id is the deterministic one-shot receipt for this settlement tx.
  const receiptId = receiptIdForTx(credential.settlementTx);
  return json(
    {
      minted: true,
      idempotent: out.idempotent ?? false,
      lh_wei: out.lhWei,
      mint_tx: out.tx ?? null,
      settlement_tx: credential.settlementTx,
      pay_to: out.recipient ?? payTo, // the PROVEN on-chain payer the $LH was minted to
    },
    200,
    origin,
    { 'Payment-Receipt': `id="${receiptId}", settlement="${credential.settlementTx}"` },
  );
}

// Re-export the parity constant so a smoke test can assert the peg without
// re-deriving it.
export { LH_WEI_PER_USDCE };
