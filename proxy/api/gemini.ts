// localharness credit proxy — transparent Gemini passthrough (Edge).
//
// The browser app (Rust/wasm) in *platform-credits* mode points its
// GeminiClient at this proxy (`with_base_url`) and sends requests with the
// SAME path/shape it would send to Google. This function:
//   1. authenticates the caller from the `x-goog-api-key` header, which in
//      credits mode carries a localharness AUTH TOKEN of the form
//      `<address>:<timestamp>:<signature>` (an Ethereum personal-sign over
//      `localharness-proxy:<address>:<timestamp>`). A real Gemini key has
//      no colons, so the two are unambiguous.
//   2. checks the caller has an active on-chain credit session
//      (`sessionExpiryOf(address)` on the diamond registry),
//   3. forwards the request to Gemini using the SERVER-HELD key (injected as
//      the `x-goog-api-key` header — never in the URL) and streams the SSE
//      response straight back.
//
// The real GEMINI_API_KEY lives only in this project's Vercel env and is
// NEVER shipped to the browser. All non-/api paths are rewritten to this
// function by vercel.json, so the client can use the proxy origin as a
// drop-in Gemini base URL.
//
// KNOWN LIMIT (accepted for the invited testnet beta; see the credit-proxy
// memory): a session is time-bounded and all-you-can-use within its window,
// and the auth token is replayable within FRESHNESS_WINDOW_SECS. Abuse is
// bounded by the on-chain session + Gemini's own rate limits. The public /
// mainnet-safe fix is per-request x402 metering (pay-per-call) — not shipped
// here.

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import {
  createWalletClient,
  defineChain,
  encodeFunctionData,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

export const config = { runtime: 'edge' };

// ---- constants -------------------------------------------------------------

const TEMPO_RPC = 'https://rpc.moderato.tempo.xyz';
const REGISTRY = '0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930';
const GEMINI_BASE = 'https://generativelanguage.googleapis.com';
const CHAIN_ID = 42431;
// `$LH` (18-decimal wei) debited per request in per-request mode.
// Env-overridable; default 0.01 LH.
const COST_PER_REQUEST_WEI = ((): bigint => {
  try {
    return BigInt(process.env.COST_PER_REQUEST_WEI ?? '10000000000000000');
  } catch {
    return 10_000_000_000_000_000n;
  }
})();

const TEMPO_CHAIN = defineChain({
  id: CHAIN_ID,
  name: 'Tempo Moderato',
  nativeCurrency: { name: 'Tempo', symbol: 'TEMPO', decimals: 18 },
  rpcUrls: { default: { http: [TEMPO_RPC] } },
});

const METER_ABI = [
  {
    name: 'meter',
    type: 'function',
    stateMutability: 'nonpayable',
    inputs: [
      { name: 'user', type: 'address' },
      { name: 'amount', type: 'uint256' },
    ],
    outputs: [],
  },
] as const;
// Generous: the on-chain credit session is the real gate, so the token only
// needs to prove the caller signed *recently enough* (re-signed per session).
const FRESHNESS_WINDOW_SECS = 86400; // 24h
// Only browser origins under our own domain may invoke the proxy (H2). A
// server-side caller sends no Origin header and is allowed through.
const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';
// model path segment: letters, digits, dot, dash only (H3 — no path/query
// injection into the upstream URL).
const MODEL_RE = /^[a-zA-Z0-9.\-]+$/;
const METHOD_RE = /^(generateContent|streamGenerateContent)$/;

// ---- CORS ------------------------------------------------------------------

function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key',
    'Vary': 'Origin',
  };
  if (
    origin &&
    (origin === ALLOWED_ORIGIN_EXACT ||
      origin.endsWith(ALLOWED_ORIGIN_SUFFIX) ||
      origin.startsWith('http://localhost'))
  ) {
    h['Access-Control-Allow-Origin'] = origin;
  }
  return h;
}

function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
  });
}

// ---- crypto helpers --------------------------------------------------------

function keccak(data: Uint8Array): Uint8Array {
  return keccak_256(data);
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function stripHex(h: string): string {
  return h.startsWith('0x') ? h.slice(2) : h;
}

/** Lowercase 0x address from a 64-byte uncompressed pubkey (no 0x04 prefix). */
function toAddress(pubKeyXY: Uint8Array): string {
  return '0x' + bytesToHex(keccak(pubKeyXY).slice(12));
}

/**
 * Recover the signer's address from an Ethereum personal_sign signature.
 * `message` is wrapped with "\x19Ethereum Signed Message:\n<len>", keccak'd,
 * then ecrecover'd. `sigHex` is 65 bytes (r||s||v), v ∈ {27,28} or {0,1}.
 */
function recoverAddress(message: string, sigHex: string): string {
  const msgBytes = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(
    `\x19Ethereum Signed Message:\n${msgBytes.length}`,
  );
  const digest = keccak(concat(prefix, msgBytes));

  const sig = hexToBytes(stripHex(sigHex));
  if (sig.length !== 65) throw new Error('signature must be 65 bytes');
  const r = sig.slice(0, 32);
  const s = sig.slice(32, 64);
  let v = sig[64];
  if (v >= 27) v -= 27;

  const signature = secp256k1.Signature.fromCompact(
    bytesToHex(concat(r, s)),
  ).addRecoveryBit(v);
  const point = signature.recoverPublicKey(digest);
  return toAddress(point.toRawBytes(false).slice(1));
}

function encodeAddressWord(address: string): string {
  return stripHex(address).toLowerCase().padStart(64, '0');
}

function selector(sig: string): string {
  return bytesToHex(keccak(new TextEncoder().encode(sig)).slice(0, 4));
}

/** `sessionExpiryOf(address) -> uint256`, decoded as BigInt unix seconds. */
async function sessionExpiryOf(address: string): Promise<bigint> {
  const data =
    '0x' + selector('sessionExpiryOf(address)') + encodeAddressWord(address);
  const res = await fetch(TEMPO_RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_call',
      params: [{ to: REGISTRY, data }, 'latest'],
    }),
  });
  const body = (await res.json()) as { result?: string; error?: unknown };
  if (!body.result) {
    throw new Error('eth_call failed: ' + JSON.stringify(body.error ?? {}));
  }
  // Compare as BigInt (M1) — never lossily coerce a uint256 word to Number.
  return BigInt(body.result);
}

/** `creditOf(address) -> uint256` — the user's prepaid per-request balance. */
async function creditOf(address: string): Promise<bigint> {
  const data =
    '0x' + selector('creditOf(address)') + encodeAddressWord(address);
  const res = await fetch(TEMPO_RPC, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_call',
      params: [{ to: REGISTRY, data }, 'latest'],
    }),
  });
  const body = (await res.json()) as { result?: string; error?: unknown };
  if (!body.result) {
    throw new Error('eth_call failed: ' + JSON.stringify(body.error ?? {}));
  }
  return BigInt(body.result);
}

/**
 * Debit `amount` `$LH` from `user` via `CreditMeterFacet.meter`, signed by
 * the proxy's meter key (env `PROXY_METER_KEY`, set as the diamond's
 * `setMeter` and funded with native gas). A standard EIP-1559 tx — Tempo
 * accepts these (the contracts were deployed with forge). Awaits
 * submission (the tx hash), not inclusion, so latency is one RPC.
 *
 * NOTE: concurrent requests from one user can race the meter key's nonce
 * (bounded burst overspend) — acceptable for the invited beta; a queue /
 * batching is the hardening.
 */
async function meterDebit(user: string, amount: bigint): Promise<void> {
  const pk = process.env.PROXY_METER_KEY;
  if (!pk) throw new Error('missing PROXY_METER_KEY');
  const account = privateKeyToAccount(
    (pk.startsWith('0x') ? pk : `0x${pk}`) as `0x${string}`,
  );
  const wallet = createWalletClient({
    account,
    chain: TEMPO_CHAIN,
    transport: http(TEMPO_RPC),
  });
  const data = encodeFunctionData({
    abi: METER_ABI,
    functionName: 'meter',
    args: [user as `0x${string}`, amount],
  });
  await wallet.sendTransaction({ to: REGISTRY as `0x${string}`, data, value: 0n });
}

// ---- handler ---------------------------------------------------------------

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');

  if (req.method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders(origin) });
  }
  if (req.method !== 'POST') {
    return json({ error: 'method not allowed' }, 405, origin);
  }

  try {
    // Validate the path: /v1beta/models/<model>:<method>. The model and
    // method are extracted and allowlisted so nothing the caller controls can
    // reshape the key-bearing upstream request (H3).
    const reqUrl = new URL(req.url);
    const m = reqUrl.pathname.match(
      /^\/v1beta\/models\/([^:/]+):([^:/]+)$/,
    );
    if (!m || !MODEL_RE.test(m[1]) || !METHOD_RE.test(m[2])) {
      return json({ error: 'unsupported path' }, 400, origin);
    }

    // AUTH — the localharness token rides in x-goog-api-key as
    // `<address>:<timestamp>:<signature>`.
    const token = req.headers.get('x-goog-api-key') ?? '';
    const parts = token.split(':');
    if (parts.length !== 3) {
      return json({ error: 'missing or malformed auth token' }, 401, origin);
    }
    const [address, tsStr, signature] = parts;
    const timestamp = Number(tsStr);
    if (!address || !signature || !Number.isFinite(timestamp)) {
      return json({ error: 'malformed auth token' }, 401, origin);
    }

    const now = Math.floor(Date.now() / 1000);
    if (Math.abs(now - timestamp) > FRESHNESS_WINDOW_SECS) {
      return json({ error: 'stale or future timestamp' }, 401, origin);
    }

    const message = `localharness-proxy:${address.toLowerCase()}:${timestamp}`;
    let recovered: string;
    try {
      recovered = recoverAddress(message, signature);
    } catch (e) {
      return json({ error: 'bad signature: ' + (e as Error).message }, 401, origin);
    }
    if (recovered.toLowerCase() !== address.toLowerCase()) {
      return json({ error: 'signature does not match address' }, 401, origin);
    }

    // On-chain gate: serve if the caller has an active TIME session OR a
    // funded PER-REQUEST balance. Both modes supported transparently.
    const [expiry, credit] = await Promise.all([
      sessionExpiryOf(address),
      creditOf(address),
    ]);
    const hasSession = expiry > BigInt(now);
    const hasCredit = credit >= COST_PER_REQUEST_WEI;
    if (!hasSession && !hasCredit) {
      return json({ error: 'no active session or credit' }, 402, origin);
    }
    // Per-request: when no flat session covers it, debit the meter before
    // serving. Fail closed if the debit can't be submitted (don't serve a
    // free request).
    if (!hasSession) {
      try {
        await meterDebit(address, COST_PER_REQUEST_WEI);
      } catch (e) {
        return json({ error: 'metering failed: ' + (e as Error).message }, 502, origin);
      }
    }

    const apiKey = process.env.GEMINI_API_KEY;
    if (!apiKey) {
      return json({ error: 'proxy misconfigured: missing GEMINI_API_KEY' }, 500, origin);
    }

    // Forward verbatim: same path + query, raw body, real key in the HEADER
    // (never the URL). Stream the SSE body straight back.
    const upstreamUrl = GEMINI_BASE + reqUrl.pathname + reqUrl.search;
    const upstream = await fetch(upstreamUrl, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'x-goog-api-key': apiKey,
        accept: req.headers.get('accept') ?? 'text/event-stream',
      },
      body: await req.text(),
    });

    return new Response(upstream.body, {
      status: upstream.status,
      headers: {
        'content-type':
          upstream.headers.get('content-type') ?? 'text/event-stream',
        ...corsHeaders(origin),
      },
    });
  } catch (e) {
    return json({ error: (e as Error).message }, 500, origin);
  }
}
