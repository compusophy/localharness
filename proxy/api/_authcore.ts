// _authcore.ts — THE one personal-sign auth primitive shared by every proxy route.
//
// Dedups the three copies that drifted apart (audit L7/L10): the object-returning
// verifier in _auth.ts, the throwing one in _stripe.ts, and sponsor.ts's private
// `authenticate`/`personalSignRecover`. This module has NO heavy deps (no viem, no
// Stripe SDK) so ANY route can import it cheaply; the viem meter/eth_call helpers
// stay in _auth.ts and the Stripe SDK stays in _stripe.ts, both re-exporting from here.
//
// SECURITY-CRITICAL — keep byte-for-byte in lockstep with the Rust minter
// `registry::proxy_auth_token` (src/registry/names.rs). The signed message is
//   localharness-proxy:<addr>:<ts>[:<route>]
// `<addr>` lowercased, `<ts>` unix seconds. `<route>` (audit L9) binds the token to
// ONE endpoint so a token minted for a cheap route (metering) can't be REPLAYED to a
// high-value write (publish/schedule/sponsor/mint) inside the 300s freshness window.

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';

// ---- CORS / origin policy (one copy; was triplicated across _auth/sponsor/mpp) ---

export const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
export const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

/** Whether `origin` may receive CORS headers (apex + subdomains + localhost dev —
 * hostname-parsed for localhost, NOT prefix-matched: a `startsWith('http://
 * localhost')` also matched `http://localhost.evil.com`). */
export function isAllowedOrigin(origin: string): boolean {
  if (origin === ALLOWED_ORIGIN_EXACT || origin.endsWith(ALLOWED_ORIGIN_SUFFIX)) return true;
  try {
    const u = new URL(origin);
    return u.protocol === 'http:' && (u.hostname === 'localhost' || u.hostname === '127.0.0.1');
  } catch {
    return false;
  }
}

// ---- crypto helpers ---------------------------------------------------------

export function stripHex(h: string): string {
  return h.startsWith('0x') ? h.slice(2) : h;
}

export function isHexAddress(s: string): boolean {
  return /^0x[0-9a-fA-F]{40}$/.test(s);
}

function concat(a: Uint8Array, b: Uint8Array): Uint8Array {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

/**
 * Recover the signer's address from an Ethereum personal_sign signature.
 * `message` is wrapped with "\x19Ethereum Signed Message:\n<len>", keccak'd, then
 * ecrecover'd. `sigHex` is 65 bytes (r||s||v), v ∈ {27,28} or {0,1}.
 */
export function recoverAddress(message: string, sigHex: string): string {
  const msgBytes = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${msgBytes.length}`);
  const digest = keccak_256(concat(prefix, msgBytes));

  const sig = hexToBytes(stripHex(sigHex));
  if (sig.length !== 65) throw new Error('signature must be 65 bytes');
  let v = sig[64];
  if (v >= 27) v -= 27;

  const signature = secp256k1.Signature.fromCompact(bytesToHex(sig.slice(0, 64))).addRecoveryBit(v);
  const point = signature.recoverPublicKey(digest);
  return '0x' + bytesToHex(keccak_256(point.toRawBytes(false).slice(1)).slice(12));
}

// ---- localharness auth token (personal-sign) --------------------------------

// 5 min — tight replay window (clients sign per request).
export const FRESHNESS_WINDOW_SECS = 300;

export type AuthResult =
  | { ok: true; address: string; legacy?: boolean }
  | { ok: false; status: number; error: string };

/**
 * Verify a localharness auth token (`<address>:<timestamp>:<signature>`): parse
 * shape, range-check address + timestamp, enforce freshness, and ecrecover the
 * personal-sign, requiring the recovered address to match the claimed one.
 *
 * `route` (audit L9): when given, the token must sign
 * `localharness-proxy:<addr>:<ts>:<route>`. A token bound to a DIFFERENT route
 * fails this check AND the legacy fallback (its signature is over neither this
 * route's message nor the bare one), so cross-route replay is blocked for any
 * migrated client. For backward compat with not-yet-migrated clients, a failed
 * route-bound check falls back to the LEGACY unbound message; that path is flagged
 * `legacy: true` and stays replayable until the fallback is retired. Pass no
 * `route` to keep the pre-L9 unbound behavior exactly.
 *
 * Returns the authenticated `address` (CLAIMED casing) or a `{ status, error }`
 * the caller maps straight to `json(error, status)` — all 401. `now` is unix
 * seconds (the caller passes its own clock read so freshness uses one value).
 */
export function verifyAuthToken(token: string, now: number, route?: string): AuthResult {
  const parts = (token ?? '').split(':');
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
  const lower = address.toLowerCase();
  const matches = (message: string): boolean => {
    try {
      return recoverAddress(message, signature).toLowerCase() === lower;
    } catch {
      return false;
    }
  };
  const base = `localharness-proxy:${lower}:${timestamp}`;
  if (route !== undefined) {
    if (matches(`${base}:${route}`)) return { ok: true, address };
    if (matches(base)) return { ok: true, address, legacy: true }; // un-migrated client
    return { ok: false, status: 401, error: 'signature does not match address' };
  }
  if (matches(base)) return { ok: true, address };
  return { ok: false, status: 401, error: 'signature does not match address' };
}

/** Throwing variant — returns the lowercase authenticated address or throws. The
 *  shape the publish/telemetry/mpp/stripe routes expect (reads its own clock). */
export function verifyAuthTokenOrThrow(token: string, route?: string): string {
  const r = verifyAuthToken(token, Math.floor(Date.now() / 1000), route);
  if (!r.ok) throw new Error(r.error);
  return r.address.toLowerCase();
}
