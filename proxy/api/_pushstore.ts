// _pushstore.ts — OFF-CHAIN Web Push subscription store (GitHub Contents API).
//
// Push subscriptions moved OFF-CHAIN: the old enroll path was a SPONSORED
// on-chain write (`setPushSub` / `setMetadata`) fired on every bell tap and app
// open — on mainnet it bypassed the relay and died with "insufficient funds"
// for normal (unfunded) users, and buzzing a phone never needed a blockchain.
// Subs now ride the SAME GitHub store as jobs/signal/chat, keyed by the OWNER
// ADDRESS: `push-subs/<address>.json` holds a JSON array of per-device
// subscription objects (newest first, upserted by the stable `dev` device id,
// else by endpoint — mirrors the old src/registry/push.rs::merge_push_sub).
//
// Written by POST /api/push-sub (personal-sign authed); read FIRST by the
// notify / broadcast / scheduler resolution, which then falls back to the
// LEGACY on-chain slots (MAIN-tokenId metadata + PushFacet.pushSubOf) so
// devices enrolled before the migration keep working — no migration needed.
//
// The underscore prefix keeps Vercel from deploying this as a route.

import { parsePushSubs, type PushSubscriptionJson } from './_webpush';

const PUSH_REPO =
  process.env.GH_PUSH_REPO ?? process.env.GH_JOBS_REPO ?? 'compusophy/localharness-jobs';
const GH_TOKEN = process.env.GH_JOBS_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '';
const PUSH_DIR = 'push-subs';
/** Max device entries kept per address — one seed rarely spans more physical
 *  devices; the cap bounds blob growth from endpoint churn. */
export const MAX_DEVICE_SUBS = 8;

const ADDR_RE = /^0x[0-9a-fA-F]{40}$/;

/** True iff `address` can key a store blob (0x + 40 hex). */
export function isStoreAddress(address: string): boolean {
  return ADDR_RE.test(address);
}

function pathFor(address: string): string {
  return `${PUSH_DIR}/${address.toLowerCase()}.json`;
}

function ghHeaders(): Record<string, string> {
  return {
    authorization: `Bearer ${GH_TOKEN}`,
    accept: 'application/vnd.github+json',
    'content-type': 'application/json',
    'user-agent': 'localharness-pushstore',
  };
}

function b64encodeUtf8(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000)
    s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

function b64decodeUtf8(b64: string): string {
  const bin = atob(b64.replace(/\n/g, ''));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

async function ghGet(path: string): Promise<{ text: string; sha: string } | null> {
  const res = await fetch(
    `https://api.github.com/repos/${PUSH_REPO}/contents/${path}?ref=main`,
    { headers: ghHeaders() },
  );
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`get ${path}: ${res.status}`);
  const j = (await res.json()) as { content?: string; sha?: string };
  if (!j.content || !j.sha) return null;
  return { text: b64decodeUtf8(j.content), sha: j.sha };
}

async function ghPut(path: string, body: string, message: string, sha?: string): Promise<void> {
  const res = await fetch(`https://api.github.com/repos/${PUSH_REPO}/contents/${path}`, {
    method: 'PUT',
    headers: ghHeaders(),
    body: JSON.stringify({
      message,
      content: b64encodeUtf8(body),
      branch: 'main',
      ...(sha ? { sha } : {}),
    }),
  });
  if (!res.ok) {
    const d = await res.text();
    throw new Error(`put ${path}: ${res.status} ${d.slice(0, 160)}`);
  }
}

/**
 * PURE upsert of ONE device's subscription into a stored list (exported for the
 * unit test): drop the prior entry for the SAME device (`dev` id when present,
 * always also the same endpoint — legacy no-dev entries), insert newest-first,
 * cap at MAX_DEVICE_SUBS (evicting oldest). Returns `null` when the list
 * already holds exactly this subscription — no write needed (this is what
 * makes the browser's register-on-every-load idempotent and cheap).
 */
export function mergeSubIntoList(
  list: PushSubscriptionJson[],
  sub: PushSubscriptionJson,
): PushSubscriptionJson[] | null {
  const same = (a: PushSubscriptionJson, b: PushSubscriptionJson) =>
    a.endpoint === b.endpoint &&
    a.keys.p256dh === b.keys.p256dh &&
    a.keys.auth === b.keys.auth &&
    (a.dev ?? '') === (b.dev ?? '');
  if (list.some((e) => same(e, sub))) return null;
  const kept = list.filter(
    (e) => !((sub.dev && e.dev === sub.dev) || e.endpoint === sub.endpoint),
  );
  return [sub, ...kept].slice(0, MAX_DEVICE_SUBS);
}

/**
 * ALL device subscriptions stored off-chain for `address` ([] when none /
 * store unconfigured / any error — NEVER throws: resolution falls back to the
 * legacy on-chain slots and a push miss must never fail the caller's request).
 */
export async function storePushSubs(address: string): Promise<PushSubscriptionJson[]> {
  if (!GH_TOKEN || !isStoreAddress(address)) return [];
  try {
    const hit = await ghGet(pathFor(address));
    return hit ? parsePushSubs(hit.text.trim()) : [];
  } catch (e) {
    console.warn(`[pushstore] read failed for ${address}: ${(e as Error).message}`);
    return [];
  }
}

/**
 * Upsert ONE device subscription under `address` (read-merge-write under the
 * blob sha, one retry on a concurrent-write conflict). Returns the resulting
 * device count and whether a write actually happened (`stored: false` = the
 * exact sub was already present). Throws on store misconfiguration / a
 * persistent GitHub failure — the ROUTE surfaces that as a 502.
 */
export async function putStorePushSub(
  address: string,
  sub: PushSubscriptionJson,
): Promise<{ stored: boolean; devices: number }> {
  if (!GH_TOKEN) throw new Error('push store not configured (no GitHub token)');
  if (!isStoreAddress(address)) throw new Error('bad address');
  const path = pathFor(address);
  let lastErr: Error | null = null;
  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const hit = await ghGet(path);
      const list = hit ? parsePushSubs(hit.text.trim()) : [];
      const merged = mergeSubIntoList(list, sub);
      if (merged === null) return { stored: false, devices: list.length };
      await ghPut(
        path,
        JSON.stringify(merged),
        `push-sub ${address.toLowerCase()} (${merged.length})`,
        hit?.sha,
      );
      return { stored: true, devices: merged.length };
    } catch (e) {
      lastErr = e as Error; // sha conflict (concurrent device) — re-read + retry once
    }
  }
  throw lastErr ?? new Error('push store write failed');
}
