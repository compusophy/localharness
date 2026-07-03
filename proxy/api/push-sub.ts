// /api/push-sub — OFF-CHAIN Web Push subscription enrollment (Edge).
//
// POST { sub } — personal-sign authed (route-bound 'push-sub'): upsert THIS
// device's push subscription (`{endpoint, keys, dev?}` — the exact JSON
// PushManager.subscribe yields, `dev`-stamped by the browser) under the
// AUTHENTICATED caller's ADDRESS in the GitHub store (_pushstore.ts). This
// REPLACES the on-chain `setPushSub` / MAIN-metadata publish the bell + app
// open used to fire: that was a sponsored write which bypassed the mainnet
// relay and failed with "insufficient funds" for normal users. Enrolling for
// notifications is now free, instant, and chain-free.
//
// GET ?address=0x… — the stored subs for an address (OPEN, like the on-chain
// slots it replaces were; a sub is only pushable by the holder of the VAPID
// private key). notify/broadcast/scheduler import the store helper DIRECTLY —
// this GET exists for the CLI / debugging.
//
// AUTH matches every other proxy route: `<address>:<timestamp>:<signature>`
// (personal-sign over `localharness-proxy:<address>:<timestamp>:push-sub`) in
// `x-goog-api-key` / `x-api-key`. No meter debit — registering a device is
// onboarding, not a paid capability; the per-sender rate window is the leash.

import {
  isStoreAddress,
  putStorePushSub,
  storePushSubs,
} from './_pushstore';
import { parsePushSubs } from './_webpush';
import { isAllowedOrigin, verifyAuthToken } from './_auth';
import { SlidingWindow, claimedAddress } from './_ratelimit';

export const config = { runtime: 'edge' };

// A push subscription JSON is ~300-500 bytes; 16KB is generous headroom.
const MAX_REQUEST_BODY_BYTES = 16_384;
// Per-sender write cap (best-effort, per-isolate — api/_ratelimit.ts): enroll
// fires on the bell tap + once per app open, so a handful/min is ample; the
// cap keeps a loop from churning the shared GitHub-store token.
const PUSH_SUB_PER_MIN = 10;
const postWindow = new SlidingWindow(PUSH_SUB_PER_MIN, 60_000);

function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) h['Access-Control-Allow-Origin'] = origin;
  return h;
}

function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
  });
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders(origin) });
  }

  // GET ?address= — the stored subs (helper for CLI/debug; [] when none).
  if (req.method === 'GET') {
    const address = (new URL(req.url).searchParams.get('address') ?? '').trim();
    if (!isStoreAddress(address)) return json({ error: 'bad address' }, 400, origin);
    return json({ subs: await storePushSubs(address) }, 200, origin);
  }

  if (req.method !== 'POST') {
    return json({ error: 'method not allowed' }, 405, origin);
  }

  try {
    const declaredLen = Number(req.headers.get('content-length') ?? '0');
    if (Number.isFinite(declaredLen) && declaredLen > MAX_REQUEST_BODY_BYTES) {
      return json({ error: 'request body too large' }, 413, origin);
    }

    // Rate limit on the CLAIMED address BEFORE auth (a flood must not cost a
    // curve recovery per request; the window gates nothing of value — a
    // spoofer burns only that address's per-isolate window, never its slot:
    // the write below only ever happens after real signature verification).
    const token =
      req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
    const claimed = claimedAddress(token);
    if (claimed) {
      const wait = postWindow.hit(claimed);
      if (wait > 0) {
        return json(
          {
            error: `rate limited: at most ${PUSH_SUB_PER_MIN} enrollments per 60s`,
            retryAfterSeconds: wait,
          },
          429,
          origin,
        );
      }
    }

    const now = Math.floor(Date.now() / 1000);
    const auth = verifyAuthToken(token, now, 'push-sub');
    if (!auth.ok) return json({ error: auth.error }, auth.status, origin);

    let subRaw: unknown;
    try {
      subRaw = ((await req.json()) as { sub?: unknown }).sub;
    } catch {
      return json({ error: 'invalid JSON body' }, 400, origin);
    }
    // Reuse the ONE subscription validator (endpoint https + keys shape;
    // preserves `dev`, strips anything else) so the store only ever holds
    // entries the push sender can consume.
    const valid = parsePushSubs(JSON.stringify(subRaw ?? null));
    if (valid.length !== 1) {
      return json(
        { error: 'missing/invalid `sub` (need {endpoint, keys:{p256dh, auth}})' },
        400,
        origin,
      );
    }

    const { stored, devices } = await putStorePushSub(auth.address, valid[0]);
    return json({ registered: true, stored, devices }, 200, origin);
  } catch (e) {
    return json({ error: (e as Error).message }, 502, origin);
  }
}
