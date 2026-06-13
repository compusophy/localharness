// localharness credit proxy — multi-provider LLM passthrough (Edge).
//
// Routes by path: /v1beta/models/<model>:<method> -> Gemini (the original,
// byte-identical path), /v1/messages -> Anthropic, /v1/chat/completions ->
// OpenAI. A client in *platform-
// credits* mode points its backend client at this proxy (`with_base_url`) and
// sends requests with the SAME path/shape it would send to the provider. This
// function:
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
  createPublicClient,
  createWalletClient,
  defineChain,
  encodeFunctionData,
  http,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

export const config = { runtime: 'edge' };

// ---- constants -------------------------------------------------------------

const TEMPO_RPC = 'https://rpc.moderato.tempo.xyz';
const REGISTRY = '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c';
const GEMINI_BASE = 'https://generativelanguage.googleapis.com';
const ANTHROPIC_BASE = 'https://api.anthropic.com';
const ANTHROPIC_VERSION = '2023-06-01';
const OPENAI_BASE = 'https://api.openai.com';
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

// Per-model price in `$LH` wei. Gemini stays FLAT (COST_PER_REQUEST_WEI —
// unchanged, so its pricing is byte-identical); Anthropic is per-model. An
// unknown anthropic model falls to a mid price, NEVER free (so a caller can't
// request an unpriced model to dodge the meter). All env-overridable.
function envWei(name: string, def: bigint): bigint {
  try {
    const v = process.env[name];
    return v ? BigInt(v) : def;
  } catch {
    return def;
  }
}
const PRICE_ANTHROPIC: Record<string, bigint> = {
  'claude-haiku-4-5-20251001': envWei('PRICE_ANTHROPIC_HAIKU_WEI', 10_000_000_000_000_000n), // 0.01
  'claude-sonnet-4-6': envWei('PRICE_ANTHROPIC_SONNET_WEI', 50_000_000_000_000_000n), // 0.05
  'claude-opus-4-8': envWei('PRICE_ANTHROPIC_OPUS_WEI', 200_000_000_000_000_000n), // 0.20
};
const PRICE_ANTHROPIC_DEFAULT = envWei('PRICE_ANTHROPIC_DEFAULT_WEI', 50_000_000_000_000_000n);

// OpenAI pricing mirrors the Anthropic tiers (mini ≙ Haiku, flagship ≙ Sonnet,
// pro ≙ Opus). Same rule: an UNKNOWN model falls to the mid default, NEVER free.
const PRICE_OPENAI: Record<string, bigint> = {
  'gpt-5-nano': envWei('PRICE_OPENAI_NANO_WEI', 10_000_000_000_000_000n), // 0.01
  'gpt-5-mini': envWei('PRICE_OPENAI_MINI_WEI', 10_000_000_000_000_000n), // 0.01
  'gpt-5.1': envWei('PRICE_OPENAI_FLAGSHIP_WEI', 50_000_000_000_000_000n), // 0.05
  'gpt-5-pro': envWei('PRICE_OPENAI_PRO_WEI', 200_000_000_000_000_000n), // 0.20
};
const PRICE_OPENAI_DEFAULT = envWei('PRICE_OPENAI_DEFAULT_WEI', 50_000_000_000_000_000n);

// Hard ceiling on the `$LH` a SINGLE request may debit. Bill-shock guard: a
// stateless Edge function has no per-identity rate store, so the spend cap is
// the user's on-chain `creditOf` balance — but a misconfigured price env (an
// extra zero on PRICE_ANTHROPIC_*_WEI) or any future code path must never debit
// an absurd amount in one shot. Anything above this is clamped DOWN to it, so
// the meter can never charge more than the configured maximum per call. 1 $LH
// is ~100x the default per-request cost — comfortably above any real model
// price, far below "drain the wallet in one call". Env-overridable.
const MAX_COST_PER_REQUEST_WEI = envWei('MAX_COST_PER_REQUEST_WEI', 1_000_000_000_000_000_000n);

type Provider = 'gemini' | 'anthropic' | 'openai';

function priceOf(provider: Provider, model: string): bigint {
  const raw =
    provider === 'gemini'
      ? COST_PER_REQUEST_WEI
      : provider === 'anthropic'
        ? (PRICE_ANTHROPIC[model] ?? PRICE_ANTHROPIC_DEFAULT)
        : (PRICE_OPENAI[model] ?? PRICE_OPENAI_DEFAULT);
  // Clamp to the per-request ceiling — never debit more than the cap in one call.
  return raw > MAX_COST_PER_REQUEST_WEI ? MAX_COST_PER_REQUEST_WEI : raw;
}

/** Whether `s` is a well-formed 0x-prefixed 20-byte hex address. */
function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}

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
const FRESHNESS_WINDOW_SECS = 300; // 5 min — tight replay window (clients sign per request)
// Reject absurdly large request bodies up front (declared Content-Length) so one
// caller can't make the proxy buffer a multi-GB body. Real LLM requests (long
// context + tools) are a few MB; 16 MB is comfortably above legitimate use.
const MAX_BODY_BYTES = 16_000_000;
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
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key, anthropic-version, authorization',
    'Vary': 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) {
    h['Access-Control-Allow-Origin'] = origin;
  }
  return h;
}

/** Whether `origin` may receive CORS headers. The localhost branch parses the
 * URL and checks the HOSTNAME — a bare `startsWith('http://localhost')` also
 * matched `http://localhost.evil.com`, letting an attacker origin read proxy
 * responses cross-origin. */
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

/** Thrown when the on-chain debit REVERTED — the caller is actually out of
 * `$LH` for this request (`CreditMeterFacet.meter` reverts `InsufficientCredits`
 * rather than ever letting a balance go negative). The handler maps this to 402,
 * distinct from an ambiguous RPC failure (502). */
class InsufficientCreditError extends Error {}

/**
 * Debit `amount` `$LH` from `user` via `CreditMeterFacet.meter`, signed by
 * the proxy's meter key (env `PROXY_METER_KEY`, set as the diamond's
 * `setMeter` and funded with native gas). A standard EIP-1559 tx — Tempo
 * accepts these (the contracts were deployed with forge).
 *
 * The debit is AUTHORITATIVE: we await the RECEIPT, not just submission, and
 * throw `InsufficientCreditError` if it reverted. This closes the prior
 * burst-overspend window where a flurry of concurrent requests all passed the
 * cheap `creditOf` gate, got served, but only the first N debits fit the
 * balance — the rest reverted on-chain (`InsufficientCredits`) unnoticed, so
 * the PLATFORM ate the over-served calls (the user's balance can never go
 * negative — the contract reverts — so this was never a user-balance bug).
 * An ambiguous wait failure (RPC/timeout) is deliberately NOT treated as a
 * revert: we return normally so the caller is still served, rather than risk a
 * double-charge if they retry a debit that actually landed. Bursts produce
 * definitive reverts, not timeouts, so the leak still closes.
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

// ---- body reading (size-capped) --------------------------------------------

/** Thrown by `readTextCapped` when the streamed body exceeds `MAX_BODY_BYTES`. */
class BodyTooLargeError extends Error {}

/**
 * Read a request body to a string, ABORTING past `MAX_BODY_BYTES`. The
 * up-front `Content-Length` check is advisory only — a chunked request (no
 * declared length) bypasses it, so without this a caller could stream an
 * unbounded body into the Edge function's memory BEFORE we ever reach the auth
 * gate (especially on the Anthropic path, which must read the body to learn the
 * model). This streams the reader and throws the moment the running total
 * crosses the cap, so the buffer can never exceed it regardless of headers.
 */
async function readTextCapped(req: Request): Promise<string> {
  const body = req.body;
  if (!body) return '';
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (value) {
      total += value.length;
      if (total > MAX_BODY_BYTES) {
        await reader.cancel().catch(() => {});
        throw new BodyTooLargeError('request body too large');
      }
      chunks.push(value);
    }
  }
  const merged = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    merged.set(c, off);
    off += c.length;
  }
  return new TextDecoder().decode(merged);
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
  const declaredLen = Number(req.headers.get('content-length') ?? '0');
  if (Number.isFinite(declaredLen) && declaredLen > MAX_BODY_BYTES) {
    return json({ error: 'request body too large' }, 413, origin);
  }

  try {
    // Route by path → provider + model. Gemini: /v1beta/models/<model>:<method>
    // (model/method allowlisted so nothing the caller controls reshapes the
    // key-bearing upstream URL — H3). Anthropic: /v1/messages; OpenAI:
    // /v1/chat/completions (both carry the model in the JSON body; read once
    // here and forwarded verbatim).
    //
    // ALL branches read the body HERE, before auth/metering. `readTextCapped`
    // throws past MAX_BODY_BYTES (a chunked body declares no Content-Length, so
    // the up-front header check can't catch it) — and that throw must land
    // BEFORE the $LH debit. The Gemini body used to be read lazily in the
    // forward block, AFTER `meterDebit`: an oversized chunked body got charged,
    // then 413'd via the outer catch without ever reaching the upstream.
    const reqUrl = new URL(req.url);
    let provider: Provider;
    let model: string;
    let requestBody: string;

    const gem = reqUrl.pathname.match(/^\/v1beta\/models\/([^:/]+):([^:/]+)$/);
    if (gem) {
      if (!MODEL_RE.test(gem[1]) || !METHOD_RE.test(gem[2])) {
        return json({ error: 'unsupported path' }, 400, origin);
      }
      provider = 'gemini';
      model = gem[1];
      requestBody = await readTextCapped(req);
    } else if (
      reqUrl.pathname === '/v1/messages' ||
      reqUrl.pathname === '/v1/chat/completions'
    ) {
      provider = reqUrl.pathname === '/v1/messages' ? 'anthropic' : 'openai';
      requestBody = await readTextCapped(req);
      let parsed: { model?: unknown };
      try {
        parsed = JSON.parse(requestBody);
      } catch {
        return json({ error: 'invalid JSON body' }, 400, origin);
      }
      model = typeof parsed.model === 'string' ? parsed.model : '';
      if (!model || !MODEL_RE.test(model)) {
        return json({ error: 'missing or invalid model' }, 400, origin);
      }
    } else {
      return json({ error: 'unsupported path' }, 400, origin);
    }

    // AUTH — the localharness token `<address>:<timestamp>:<signature>` rides in
    // x-goog-api-key (Gemini clients), x-api-key (Anthropic clients), or
    // `Authorization: Bearer <token>` (OpenAI-shaped clients). A real provider
    // key has no colons, so the forms stay unambiguous.
    const bearer = (req.headers.get('authorization') ?? '').replace(/^Bearer\s+/i, '');
    const token =
      req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? bearer;
    const parts = token.split(':');
    if (parts.length !== 3) {
      return json({ error: 'missing or malformed auth token' }, 401, origin);
    }
    const [address, tsStr, signature] = parts;
    const timestamp = Number(tsStr);
    if (!address || !signature || !Number.isFinite(timestamp)) {
      return json({ error: 'malformed auth token' }, 401, origin);
    }
    // Validate the address shape up front. The recovered-address match below
    // already forces a well-formed 0x-address (recovery always yields one), but
    // checking explicitly — like mcp.ts's `isHexAddress` — keeps `address` from
    // ever flowing UNvalidated into `encodeAddressWord` / the on-chain
    // `meter(user,...)` debit, and rejects garbage with a clean 401 instead of
    // relying on the later equality as an implicit validator.
    if (!isHexAddress(address)) {
      return json({ error: 'malformed auth token: address' }, 401, origin);
    }
    // The signed timestamp must be a non-negative INTEGER (unix seconds). The
    // client signs the decimal it embeds; reject fractional/odd numerics so a
    // token whose `tsStr` re-stringifies differently than what was signed fails
    // here rather than later at the signature-mismatch (clearer error, and no
    // chance of an exotic numeric slipping the freshness math).
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

    // PROVIDER-KEY presence — checked BEFORE the metering section. A missing
    // server key is a proxy misconfiguration and must surface as a 500 WITHOUT
    // debiting: this check used to live in the forward block, AFTER
    // `meterDebit`, so a misconfigured proxy charged callers for requests it
    // could never forward. Invariant: nothing proxy-side may fail after the
    // user is charged except the upstream call itself. (Kept after auth so an
    // unauthenticated probe can't learn the proxy's key configuration.)
    const upstreamKey =
      provider === 'gemini'
        ? process.env.GEMINI_API_KEY
        : provider === 'anthropic'
          ? process.env.ANTHROPIC_API_KEY
          : process.env.OPENAI_API_KEY;
    if (!upstreamKey) {
      return json(
        {
          error:
            provider === 'gemini'
              ? 'proxy misconfigured: missing GEMINI_API_KEY'
              : provider === 'anthropic'
                ? 'proxy missing ANTHROPIC_API_KEY — add it to the proxy env to enable Claude on credits'
                : 'proxy missing OPENAI_API_KEY — add it to the proxy env to enable OpenAI models on credits',
        },
        500,
        origin,
      );
    }

    // On-chain gate: serve if the caller has an active TIME session OR a
    // funded PER-REQUEST balance. Both modes supported transparently.
    const cost = priceOf(provider, model);
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
    // PREFER per-request metering: a FUNDED meter (`creditOf >= cost`) means the
    // caller opted into real per-call billing, so debit the per-model cost even
    // if a (free beta, `sessionPrice==0`) session is ALSO active — otherwise the
    // free session would silently make every call free. Callers with ONLY a
    // session and no meter balance (the free-beta / CLI fallback) are still
    // served free. Fail closed if the debit can't be submitted.
    if (hasCredit) {
      try {
        await meterDebit(address, cost);
      } catch (e) {
        // A definitive revert = genuinely out of $LH for THIS request. If a
        // (free-beta) session also covers the caller, serve under it; otherwise
        // it's a real 402. Anything else is an ambiguous infra error (502).
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

    // Forward to the right upstream with the SERVER-held key in the HEADER
    // (never the URL). Stream the SSE body straight back. The body was read
    // (and size-capped) at routing time and the key checked before metering —
    // from here the only thing that can fail is the upstream call itself,
    // which is the one failure a debited caller legitimately pays for.
    let upstream: Response;
    if (provider === 'gemini') {
      // Rebuild the upstream query from an ALLOWLIST instead of forwarding the
      // caller-controlled `reqUrl.search` verbatim. The real client sends only
      // `alt=sse` (GeminiClient::stream_generate). Passing the raw search string
      // let a caller append arbitrary params to the key-bearing Google URL
      // (e.g. `?key=...`, which Google reads as an alternate credential, or
      // other request-reshaping params) — nothing the caller controls should
      // touch the upstream URL beyond the already-allowlisted model/method.
      const upstreamQs = reqUrl.searchParams.get('alt') === 'sse' ? '?alt=sse' : '';
      upstream = await fetch(GEMINI_BASE + reqUrl.pathname + upstreamQs, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'x-goog-api-key': upstreamKey,
          accept: req.headers.get('accept') ?? 'text/event-stream',
        },
        body: requestBody,
      });
    } else if (provider === 'anthropic') {
      upstream = await fetch(ANTHROPIC_BASE + '/v1/messages', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'x-api-key': upstreamKey,
          'anthropic-version': ANTHROPIC_VERSION,
          accept: req.headers.get('accept') ?? 'text/event-stream',
        },
        body: requestBody,
      });
    } else {
      upstream = await fetch(OPENAI_BASE + '/v1/chat/completions', {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          authorization: `Bearer ${upstreamKey}`,
          accept: req.headers.get('accept') ?? 'text/event-stream',
        },
        body: requestBody,
      });
    }

    return new Response(upstream.body, {
      status: upstream.status,
      headers: {
        'content-type':
          upstream.headers.get('content-type') ?? 'text/event-stream',
        ...corsHeaders(origin),
      },
    });
  } catch (e) {
    if (e instanceof BodyTooLargeError) {
      return json({ error: 'request body too large' }, 413, origin);
    }
    return json({ error: (e as Error).message }, 500, origin);
  }
}
