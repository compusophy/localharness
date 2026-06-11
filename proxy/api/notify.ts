// localharness credit proxy — PC-TO-MOBILE NOTIFY route (Edge).
//
// POST /api/notify { title, body } → Web-Pushes a note to the CALLER's OWN
// registered device. This is the headless "notify me when done" affordance
// (on-chain feedback #69): an agent running in a shell (CLI / MCP / a script)
// signs the standard proxy token and the proxy delivers the push to the phone
// the caller's owner already enrolled via the browser app's "enable
// notifications" flow — no tab, no new key, no new trust.
//
// SELF-ONLY BY DESIGN: the push target is derived from the AUTHENTICATED
// caller, never from the request — caller address → `mainOf(caller)` (no MAIN
// → no fallback → 404) → the Web Push subscription published under
// `metadata(mainTokenId, keccak256("localharness.push_sub"))` by
// src/app/notifications.rs. There is deliberately NO cross-user targeting:
// pushing to ANOTHER owner's phone is a consent-design problem (opt-in,
// rate limits, blocklists — issue #68) and stays out of scope here.
//
// AUTH + BILLING are byte-compatible with api/fetch.ts / api/gemini.ts: the
// caller sends `<address>:<timestamp>:<signature>` (an Ethereum personal-sign
// over `localharness-proxy:<address>:<timestamp>`) in `x-goog-api-key` (or
// `x-api-key`); the proxy recovers the signer, gates on an active SessionFacet
// session OR a CreditMeterFacet balance, and debits the SAME flat per-request
// cost — a paid capability like any other proxied call, so a loop can't buzz
// a phone for free.
//
// ORDER OF OPERATIONS (the fetch.ts invariant: nothing proxy-side may fail
// AFTER the caller is charged except the actual upstream send): payload
// validation → VAPID config check → auth → subscription lookup (404 before
// any debit) → credit gate + meter debit → the push itself (502 on failure).

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import {
  createPublicClient,
  createWalletClient,
  defineChain,
  encodeFunctionData,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { sendWebPush, type PushSubscriptionJson } from './_webpush';

export const config = { runtime: 'edge' };

// ---- constants (mirror api/fetch.ts) ----------------------------------------

const TEMPO_RPC = 'https://rpc.moderato.tempo.xyz';
const REGISTRY = '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c';
const CHAIN_ID = 42431;

// SAME per-request price as a model call — same env knob, same default
// 0.01 $LH. A self-push is a paid capability like a model turn (the meter IS
// the spam filter — see the header).
const COST_PER_REQUEST_WEI = ((): bigint => {
  try {
    return BigInt(process.env.COST_PER_REQUEST_WEI ?? '10000000000000000');
  } catch {
    return 10_000_000_000_000_000n;
  }
})();

const FRESHNESS_WINDOW_SECS = 300; // same tight replay window as gemini.ts
const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

// Payload bounds. Pushes are glanceable phone banners, not documents —
// overlong inputs are TRIMMED (whitespace) then TRUNCATED, never rejected.
const MAX_TITLE_CHARS = 80;
const MAX_BODY_CHARS = 200;
const MAX_REQUEST_BODY_BYTES = 16_384; // { title, body } is tiny

// Web Push subscription slot — written by the browser app's admin "enable
// notifications" flow (src/app/notifications.rs) under the owner's MAIN
// tokenId, v1 plaintext JSON. Same slot the scheduler worker reads.
const PUSH_SUB_KEY = bytesToHex(
  keccak_256(new TextEncoder().encode('localharness.push_sub')),
);

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

/** Whether `s` is a well-formed 0x-prefixed 20-byte hex address. */
function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}

// ---- CORS (same policy as fetch.ts/gemini.ts) --------------------------------

function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key',
    'Vary': 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) {
    h['Access-Control-Allow-Origin'] = origin;
  }
  return h;
}

/** Whether `origin` may receive CORS headers (apex + subdomains + localhost
 * dev — hostname-parsed, not prefix-matched; see gemini.ts). */
function isAllowedOrigin(origin: string): boolean {
  if (origin === ALLOWED_ORIGIN_EXACT || origin.endsWith(ALLOWED_ORIGIN_SUFFIX)) {
    return true;
  }
  try {
    const u = new URL(origin);
    return (
      u.protocol === 'http:' &&
      (u.hostname === 'localhost' || u.hostname === '127.0.0.1')
    );
  } catch {
    return false;
  }
}

function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
  });
}

// ---- crypto helpers (mirror fetch.ts/gemini.ts) -------------------------------

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
 * Same preimage + recovery as gemini.ts — the token scheme is shared.
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

/** One `eth_call` against the diamond; returns the raw result hex or throws. */
async function ethCall(data: string): Promise<string> {
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
  return body.result;
}

/** `sessionExpiryOf(address) -> uint256`, decoded as BigInt unix seconds. */
async function sessionExpiryOf(address: string): Promise<bigint> {
  return BigInt(
    await ethCall('0x' + selector('sessionExpiryOf(address)') + encodeAddressWord(address)),
  );
}

/** `creditOf(address) -> uint256` — the user's prepaid per-request balance. */
async function creditOf(address: string): Promise<bigint> {
  return BigInt(
    await ethCall('0x' + selector('creditOf(address)') + encodeAddressWord(address)),
  );
}

/** Decode an ABI-encoded dynamic `bytes` return into UTF-8 text ('' if empty). */
function decodeAbiBytesUtf8(resultHex: string): string {
  const h = stripHex(resultHex);
  if (h.length < 128) return ''; // needs at least offset + length words
  const off = Number(BigInt('0x' + h.slice(0, 64))) * 2;
  const len = Number(BigInt('0x' + h.slice(off, off + 64)));
  if (len === 0) return '';
  const dataStart = off + 64;
  if (h.length < dataStart + len * 2) return '';
  const bytes = new Uint8Array(len);
  for (let i = 0; i < len; i++) {
    bytes[i] = parseInt(h.slice(dataStart + i * 2, dataStart + i * 2 + 2), 16);
  }
  return new TextDecoder().decode(bytes);
}

/**
 * The CALLER's published Web Push subscription, or null. SELF-ONLY slot rule:
 * `mainOf(caller)` and NOTHING else — a caller with no MAIN identity (or an
 * empty/malformed slot) simply has no push target. (The scheduler worker's
 * `pushSubOf` falls back to a job's targetId; here there is no job, so no
 * fallback.)
 */
async function pushSubOfCaller(address: string): Promise<PushSubscriptionJson | null> {
  const main = BigInt(
    await ethCall('0x' + selector('mainOf(address)') + encodeAddressWord(address)),
  );
  if (main === 0n) return null;
  const data =
    '0x' +
    selector('metadata(uint256,bytes32)') +
    main.toString(16).padStart(64, '0') +
    PUSH_SUB_KEY;
  const text = decodeAbiBytesUtf8(await ethCall(data)).trim();
  if (!text) return null;
  let sub: PushSubscriptionJson;
  try {
    sub = JSON.parse(text) as PushSubscriptionJson;
  } catch {
    return null;
  }
  if (
    typeof sub?.endpoint !== 'string' ||
    !sub.endpoint.startsWith('https://') ||
    typeof sub.keys?.p256dh !== 'string' ||
    typeof sub.keys?.auth !== 'string'
  ) {
    return null;
  }
  return sub;
}

/** Thrown when the on-chain debit REVERTED (caller is genuinely out of $LH). */
class InsufficientCreditError extends Error {}

/**
 * Debit `amount` $LH from `user` via `CreditMeterFacet.meter` — identical
 * semantics to fetch.ts/gemini.ts::meterDebit: await the receipt
 * (authoritative), throw on a definitive revert, return normally on an
 * ambiguous wait failure (never risk a double-charge on retry).
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
  const hash = await wallet.sendTransaction({
    to: REGISTRY as `0x${string}`,
    data,
    value: 0n,
  });

  const pub = createPublicClient({ chain: TEMPO_CHAIN, transport: http(TEMPO_RPC) });
  let status: 'success' | 'reverted';
  try {
    ({ status } = await pub.waitForTransactionReceipt({
      hash,
      timeout: 12_000,
      pollingInterval: 500,
    }));
  } catch {
    return; // ambiguous (RPC/timeout) — serve; do NOT double-charge on retry
  }
  if (status === 'reverted') {
    throw new InsufficientCreditError('on-chain debit reverted (insufficient $LH)');
  }
}

// ---- handler ------------------------------------------------------------------

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');

  if (req.method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders(origin) });
  }
  if (req.method !== 'POST') {
    return json({ error: 'method not allowed' }, 405, origin);
  }

  try {
    // ---- request body: { title, body } -----------------------------------------
    const declaredLen = Number(req.headers.get('content-length') ?? '0');
    if (Number.isFinite(declaredLen) && declaredLen > MAX_REQUEST_BODY_BYTES) {
      return json({ error: 'request body too large' }, 413, origin);
    }
    let title: string;
    let body: string;
    try {
      const parsed = (await req.json()) as { title?: unknown; body?: unknown };
      title = typeof parsed.title === 'string' ? parsed.title.trim() : '';
      body = typeof parsed.body === 'string' ? parsed.body.trim() : '';
    } catch {
      return json({ error: 'invalid JSON body' }, 400, origin);
    }
    if (!title) {
      return json({ error: 'missing title' }, 400, origin);
    }
    title = title.slice(0, MAX_TITLE_CHARS);
    body = body.slice(0, MAX_BODY_CHARS);

    // ---- VAPID config (BEFORE auth/debit — a misconfigured proxy must cost
    // the caller nothing) ---------------------------------------------------------
    const publicKey = process.env.VAPID_PUBLIC_KEY;
    const privateKey = process.env.VAPID_PRIVATE_KEY;
    const subject = process.env.VAPID_SUBJECT;
    if (!publicKey || !privateKey || !subject) {
      return json({ error: 'proxy misconfigured: web push is not set up' }, 500, origin);
    }

    // ---- AUTH — same token scheme + headers as api/gemini.ts -------------------
    const token =
      req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
    const parts = token.split(':');
    if (parts.length !== 3) {
      return json({ error: 'missing or malformed auth token' }, 401, origin);
    }
    const [address, tsStr, signature] = parts;
    const timestamp = Number(tsStr);
    if (!address || !signature || !Number.isFinite(timestamp)) {
      return json({ error: 'malformed auth token' }, 401, origin);
    }
    if (!isHexAddress(address)) {
      return json({ error: 'malformed auth token: address' }, 401, origin);
    }
    if (!Number.isInteger(timestamp) || timestamp < 0) {
      return json({ error: 'malformed auth token: timestamp' }, 401, origin);
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

    // ---- subscription lookup (BEFORE the debit — a caller with no enrolled
    // device must not be charged for an undeliverable push) ----------------------
    let sub: PushSubscriptionJson | null;
    try {
      sub = await pushSubOfCaller(address);
    } catch (e) {
      return json({ error: 'subscription lookup failed: ' + (e as Error).message }, 502, origin);
    }
    if (!sub) {
      return json(
        { error: 'no push subscription on-chain — enable notifications in the app first' },
        404,
        origin,
      );
    }

    // ---- credit gate + meter debit — same model as a Gemini model call ---------
    const cost = COST_PER_REQUEST_WEI;
    const [expiry, credit] = await Promise.all([
      sessionExpiryOf(address),
      creditOf(address),
    ]);
    const hasSession = expiry > BigInt(now);
    const hasCredit = credit >= cost;
    if (!hasSession && !hasCredit) {
      return json(
        {
          error:
            'no $LH credit or active session for this identity — fund the per-request meter (localharness redeem / send / topup) or open a session explicitly (localharness session). See https://localharness.xyz/llms.txt',
        },
        402,
        origin,
      );
    }
    // Prefer per-request metering over a lingering free session (gemini.ts
    // rationale: a funded meter means the caller opted into per-call billing).
    if (hasCredit) {
      try {
        await meterDebit(address, cost);
      } catch (e) {
        if (e instanceof InsufficientCreditError) {
          if (!hasSession) {
            return json(
              {
                error:
                  'insufficient $LH credit — the on-chain debit reverted (balance changed since the gate read)',
              },
              402,
              origin,
            );
          }
          // else: covered by an active session — fall through and serve.
        } else {
          return json({ error: 'metering failed: ' + (e as Error).message }, 502, origin);
        }
      }
    }

    // ---- the push itself — the one failure a debited caller pays for -----------
    // Same { title, body } JSON the scheduler worker sends; the service worker
    // (web/sw.js) renders it. sendWebPush never throws (5s-capped POST).
    const sent = await sendWebPush(sub, JSON.stringify({ title, body }), {
      publicKey,
      privateKey,
      subject,
    });
    if (!sent) {
      return json({ error: 'push send failed (service rejected or timed out)' }, 502, origin);
    }
    return json({ sent: true }, 200, origin);
  } catch (e) {
    return json({ error: (e as Error).message }, 500, origin);
  }
}
