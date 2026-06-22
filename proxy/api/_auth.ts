// _auth.ts — the proxy's SHARED auth + metering primitives (§5 dedup).
//
// fetch.ts / notify.ts / broadcast.ts / gemini.ts all repeated the SAME
// origin/CORS allow-check, the `localharness-proxy:<address>:<timestamp>`
// personal-sign recovery + freshness window, the `creditOf` / `sessionExpiryOf`
// eth_call reads, and the `CreditMeterFacet.meter` debit. They live ONCE here.
//
// SECURITY-CRITICAL: these are byte-for-byte the inlined logic they replace —
// SAME recovery (\x19 personal_sign prefix, ecrecover), SAME freshness window
// (FRESHNESS_WINDOW_SECS = 300), SAME CORS rules (apex + *.localharness.xyz +
// localhost/127.0.0.1 over http), SAME authoritative-receipt meter debit. Do not
// change any of these semantics — every caller depends on them being identical.

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

import { TEMPO_RPC, REGISTRY, CHAIN_ID } from './_chain';

// ---- CORS / origin policy ---------------------------------------------------

// Only browser origins under our own domain (apex + subdomains) and localhost
// dev may receive CORS headers / invoke the proxy. A server-side caller sends no
// Origin header and is allowed through by the per-route handler.
export const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
export const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

/** Whether `origin` may receive CORS headers (apex + subdomains + localhost
 * dev — hostname-parsed, not prefix-matched: a bare `startsWith('http://
 * localhost')` also matched `http://localhost.evil.com`, letting an attacker
 * origin read proxy responses cross-origin). */
export function isAllowedOrigin(origin: string): boolean {
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

// ---- crypto helpers ---------------------------------------------------------

export function keccak(data: Uint8Array): Uint8Array {
  return keccak_256(data);
}

export function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

export function stripHex(h: string): string {
  return h.startsWith('0x') ? h.slice(2) : h;
}

/** Lowercase 0x address from a 64-byte uncompressed pubkey (no 0x04 prefix). */
export function toAddress(pubKeyXY: Uint8Array): string {
  return '0x' + bytesToHex(keccak(pubKeyXY).slice(12));
}

/** Whether `s` is a well-formed 0x-prefixed 20-byte hex address. */
export function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}

/**
 * Recover the signer's address from an Ethereum personal_sign signature.
 * `message` is wrapped with "\x19Ethereum Signed Message:\n<len>", keccak'd,
 * then ecrecover'd. `sigHex` is 65 bytes (r||s||v), v ∈ {27,28} or {0,1}.
 */
export function recoverAddress(message: string, sigHex: string): string {
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

export function encodeAddressWord(address: string): string {
  return stripHex(address).toLowerCase().padStart(64, '0');
}

/** 4-byte function selector hex (no 0x) — keccak256(sig)[..4]. */
export function selector(sig: string): string {
  return bytesToHex(keccak(new TextEncoder().encode(sig)).slice(0, 4));
}

// ---- localharness auth token (personal-sign) --------------------------------

// 5 min — tight replay window (clients sign per request). The auth token
// `<address>:<timestamp>:<signature>` (personal-sign over
// `localharness-proxy:<address>:<timestamp>`) is replayable within this window.
export const FRESHNESS_WINDOW_SECS = 300;

/**
 * Verify a localharness auth token (`<address>:<timestamp>:<signature>`): parse
 * shape, range-check the address + timestamp, enforce the freshness window, and
 * ecrecover the personal-sign over `localharness-proxy:<address>:<timestamp>`,
 * requiring the recovered address to match the claimed one.
 *
 * Returns the authenticated `address` (the CLAIMED casing, unchanged) on success
 * or a `{ status, error }` the caller maps straight to a `json(error, status)`
 * — all 401, with the SAME error strings the inlined blocks returned. `now` is
 * unix seconds (the caller passes its own `Math.floor(Date.now()/1000)` so the
 * freshness check uses the handler's single clock read).
 */
export function verifyAuthToken(
  token: string,
  now: number,
): { ok: true; address: string } | { ok: false; status: number; error: string } {
  const parts = token.split(':');
  if (parts.length !== 3) {
    return { ok: false, status: 401, error: 'missing or malformed auth token' };
  }
  const [address, tsStr, signature] = parts;
  const timestamp = Number(tsStr);
  if (!address || !signature || !Number.isFinite(timestamp)) {
    return { ok: false, status: 401, error: 'malformed auth token' };
  }
  if (!isHexAddress(address)) {
    return { ok: false, status: 401, error: 'malformed auth token: address' };
  }
  if (!Number.isInteger(timestamp) || timestamp < 0) {
    return { ok: false, status: 401, error: 'malformed auth token: timestamp' };
  }
  if (Math.abs(now - timestamp) > FRESHNESS_WINDOW_SECS) {
    return { ok: false, status: 401, error: 'stale or future timestamp' };
  }
  const message = `localharness-proxy:${address.toLowerCase()}:${timestamp}`;
  let recovered: string;
  try {
    recovered = recoverAddress(message, signature);
  } catch (e) {
    return { ok: false, status: 401, error: 'bad signature: ' + (e as Error).message };
  }
  if (recovered.toLowerCase() !== address.toLowerCase()) {
    return { ok: false, status: 401, error: 'signature does not match address' };
  }
  return { ok: true, address };
}

// ---- on-chain reads + meter debit -------------------------------------------

export const TEMPO_CHAIN = defineChain({
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

/** One `eth_call` against the diamond; returns the raw result hex or throws. */
export async function ethCall(data: string): Promise<string> {
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

/** `sessionExpiryOf(address) -> uint256`, decoded as BigInt unix seconds.
 * Compare as BigInt — never lossily coerce a uint256 word to Number. */
export async function sessionExpiryOf(address: string): Promise<bigint> {
  return BigInt(
    await ethCall('0x' + selector('sessionExpiryOf(address)') + encodeAddressWord(address)),
  );
}

/** `creditOf(address) -> uint256` — the user's prepaid per-request balance. */
export async function creditOf(address: string): Promise<bigint> {
  return BigInt(
    await ethCall('0x' + selector('creditOf(address)') + encodeAddressWord(address)),
  );
}

/** Thrown when the on-chain debit REVERTED — the caller is actually out of
 * `$LH` for this request (`CreditMeterFacet.meter` reverts `InsufficientCredits`
 * rather than ever letting a balance go negative). The handler maps this to 402,
 * distinct from an ambiguous RPC failure (502). */
export class InsufficientCreditError extends Error {}

/**
 * Debit `amount` `$LH` from `user` via `CreditMeterFacet.meter`, signed by the
 * proxy's meter key (env `PROXY_METER_KEY`). The debit is AUTHORITATIVE: we
 * await the RECEIPT (when `confirm`), not just submission, and throw
 * `InsufficientCreditError` if it reverted. An ambiguous wait failure
 * (RPC/timeout) is deliberately NOT treated as a revert: we return normally so
 * the caller is still served, rather than risk a double-charge if they retry a
 * debit that actually landed.
 *
 * `confirm=false` (streaming callers) awaits only the broadcast — it must NOT
 * serialize first-byte latency behind the receipt. Burst safety then comes from
 * the broadcast assigning the address's account nonce serially + the caller's
 * own in-isolate reservation + an up-front floor debit.
 */
export async function meterDebit(
  user: string,
  amount: bigint,
  confirm = true,
): Promise<void> {
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

  if (!confirm) return;

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
