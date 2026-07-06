// localharness credit proxy — FEED BROADCAST route (Edge).
//
// POST /api/broadcast { targetId, title, body } → Web-Pushes one note to EVERY
// subscriber of a subdomain's feed. This is the off-chain push delivery for the
// cartridge "Ready Up" feature: a cartridge (or any participant) triggers a
// "ready up" / "your turn" / "match starting" buzz to everyone subscribed to a
// feed, with no tab open on the recipient devices. It is notify.ts's FAN-OUT
// sibling — same auth, same token scheme, same `sendWebPush` plumbing — but it
// reads the feed's SUBSCRIBER SET on-chain (SubscribeFacet.subscribersOf — the
// feed's MEMBERSHIP roster, a feed feature distinct from push enrollment) and
// pushes to each subscriber's off-chain push-store subscription instead of the
// caller's own. Unlike notify.ts, a broadcast is FREE (not metered) — see below.
//
// WHO MAY BROADCAST — IDENTITY-GATED, NOT OWNER-GATED. The only gate on the
// sender is the standard proxy token (a valid Ethereum personal-sign over
// `localharness-proxy:<addr>:<ts>`): the sender must be a real identity, but it
// need NOT own the feed. Per the product, ANYONE participating in a feed can
// trigger a Ready-Up for it. Identity-gating is the sybil bar; the per-targetId
// RATE LIMIT (below) is the anti-spam control that owner-gating would otherwise
// provide.
//
// AUTH is byte-compatible with api/notify.ts / api/gemini.ts: the caller sends
// `<address>:<timestamp>:<signature>` in `x-goog-api-key` (or `x-api-key`) and
// the proxy recovers the signer — the sender must be a real identity. A
// Ready-Up broadcast is intentionally FREE (NOT metered): requiring $LH per tap
// would kill the viral, low-friction "anyone can ping the group" use case. The
// spam controls are the identity gate (a valid signed identity above) + the
// rate limits below.
//
// ORDER OF OPERATIONS (nothing may fail AFTER the broadcast commits except
// best-effort pushes): payload validation → VAPID config check → per-SENDER
// rate-limit (429 BEFORE auth — cheap rejection on the CLAIMED address; a
// spoofer burns a window) → auth → per-feed rate-limit (429 before the fan-out)
// → read subscribersOf → per subscriber: resolve push_sub + sendWebPush
// (best-effort; one failure never aborts the rest) → counts. FREE (rate-limited
// + identity-gated) — no meter debit.
//
// RATE LIMITS (best-effort, PER-ISOLATE — see api/_ratelimit.ts for why
// that's accepted): per SENDER ≤ BROADCAST_SENDER_PER_MIN/min (a broadcast
// fans out to up to MAX_FANOUT phones, so the per-sender budget is tight) +
// the per-FEED cooldown below. With broadcasts free, these rate limits + the
// identity gate ARE the spam story.

import { sendWebPushAll } from './_webpush';
import { SlidingWindow, claimedAddress } from './_ratelimit';
import { storePushSubs } from './_pushstore';

export const config = { runtime: 'edge' };

// ---- constants (mirror api/notify.ts) ---------------------------------------

// Auth primitives (CORS allow-check, personal-sign recovery + freshness, the
// generic eth_call + selector/encode helpers) are SHARED in `_auth.ts` (§5
// dedup) — byte-for-byte the logic that used to be inlined here.
import {
  isAllowedOrigin,
  verifyAuthToken,
  selector,
  stripHex,
  ethCall,
} from './_auth';

// Payload bounds — same as notify.ts; pushes are glanceable banners, trimmed
// then truncated, never rejected for length.
const MAX_TITLE_CHARS = 80;
const MAX_BODY_CHARS = 200;
const MAX_REQUEST_BODY_BYTES = 16_384; // { targetId, title, body } is tiny

// Fan-out cap — at most this many subscribers are pushed per broadcast. Bounds
// the per-invocation RPC + push fan-out on the public RPC and Edge wall-clock.
// `subscribersOf` may return more; the extras are dropped (response notes the
// truncation via `subscribers` being the capped count + a `truncated` flag).
const MAX_FANOUT = 500;

// Per-feed rate limit: at most one broadcast per RATE_LIMIT_MS per targetId.
// This is the anti-spam control that owner-gating would otherwise provide
// (broadcast is identity-gated, not owner-gated — anyone may trigger a feed).
//
// ⚠️ LIMITATION: this Map is per Edge ISOLATE, not global. Vercel may run
// several isolates per region (and several regions), so the effective floor is
// "1 per 20s per isolate", not a hard global "1 per 20s". It defeats a single
// caller hammering one warm isolate (the common abuse shape); a determined
// attacker spreading requests across isolates can exceed it. A hard global
// limit needs shared state (KV/Redis) — deliberately out of scope (this + the
// identity gate are the cheap first line; broadcasts are free).
const RATE_LIMIT_MS = 3_000;
const lastBroadcastAt = new Map<string, number>();

// Per-SENDER sliding window (also per-isolate, same caveat). 3/min: one
// broadcast fans out to up to MAX_FANOUT devices, so a sender's spam budget
// must be far tighter than notify's — 3 group-buzzes a minute covers any
// legit "ready up" cadence while a loop dies immediately. Checked BEFORE
// auth on the CLAIMED address (cheap rejection; a spoofer can burn an
// address's window in this isolate, never its funds — the identity gate
// below still requires a real signature).
const BROADCAST_SENDER_PER_MIN = 3;
const senderWindow = new SlidingWindow(BROADCAST_SENDER_PER_MIN, 60_000);

// ---- CORS (same policy as notify.ts; isAllowedOrigin shared via _auth.ts) -----

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

function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
  });
}

function encodeUint256Word(value: bigint): string {
  return value.toString(16).padStart(64, '0');
}

/**
 * `subscribersOf(uint256 targetId) -> address[]` (SubscribeFacet). Decodes the
 * ABI dynamic `address[]` return into a list of lowercase 0x addresses. Returns
 * [] for an empty / malformed result.
 */
async function subscribersOf(targetId: bigint): Promise<string[]> {
  const resultHex = await ethCall(
    '0x' + selector('subscribersOf(uint256)') + encodeUint256Word(targetId),
  );
  const h = stripHex(resultHex);
  // Dynamic array: [offset(32)] -> [length(32)] -> [elem(32)]*length.
  if (h.length < 128) return []; // needs at least offset + length words
  const off = Number(BigInt('0x' + h.slice(0, 64))) * 2;
  if (h.length < off + 64) return [];
  const len = Number(BigInt('0x' + h.slice(off, off + 64)));
  if (len === 0) return [];
  const out: string[] = [];
  const base = off + 64;
  for (let i = 0; i < len; i++) {
    const wordStart = base + i * 64;
    if (h.length < wordStart + 64) break;
    // Each word is a left-padded address; take the low 20 bytes (40 hex chars).
    out.push('0x' + h.slice(wordStart + 24, wordStart + 64).toLowerCase());
  }
  return out;
}

// ---- handler ----------------------------------------------------------------

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');

  if (req.method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders(origin) });
  }
  if (req.method !== 'POST') {
    return json({ error: 'method not allowed' }, 405, origin);
  }

  try {
    // ---- request body: { targetId, title, body } ------------------------------
    const declaredLen = Number(req.headers.get('content-length') ?? '0');
    if (Number.isFinite(declaredLen) && declaredLen > MAX_REQUEST_BODY_BYTES) {
      return json({ error: 'request body too large' }, 413, origin);
    }
    let targetIdRaw: string | number;
    let title: string;
    let body: string;
    try {
      const parsed = (await req.json()) as {
        targetId?: unknown;
        title?: unknown;
        body?: unknown;
      };
      targetIdRaw =
        typeof parsed.targetId === 'string' || typeof parsed.targetId === 'number'
          ? parsed.targetId
          : '';
      title = typeof parsed.title === 'string' ? parsed.title.trim() : '';
      body = typeof parsed.body === 'string' ? parsed.body.trim() : '';
    } catch {
      return json({ error: 'invalid JSON body' }, 400, origin);
    }
    // targetId → bigint (accepts a decimal string or a JSON number).
    let targetId: bigint;
    try {
      targetId = BigInt(targetIdRaw);
    } catch {
      return json({ error: 'missing or invalid targetId' }, 400, origin);
    }
    if (targetId <= 0n) {
      return json({ error: 'missing or invalid targetId' }, 400, origin);
    }
    if (!title) {
      return json({ error: 'missing title' }, 400, origin);
    }
    title = title.slice(0, MAX_TITLE_CHARS);
    body = body.slice(0, MAX_BODY_CHARS);

    // ---- VAPID config (BEFORE auth — a misconfigured proxy must reject early,
    // before any fan-out) -------------------------------------------------------
    const publicKey = process.env.VAPID_PUBLIC_KEY;
    const privateKey = process.env.VAPID_PRIVATE_KEY;
    const subject = process.env.VAPID_SUBJECT;
    if (!publicKey || !privateKey || !subject) {
      return json({ error: 'proxy misconfigured: web push is not set up' }, 500, origin);
    }
    const vapid = { publicKey, privateKey, subject };

    // ---- per-SENDER RATE LIMIT (BEFORE auth — rejecting a flood must not
    // cost a curve recovery per request). Keyed on the CLAIMED, unverified
    // address — safe: a spoofer burns the address's per-isolate rate window
    // (a one-minute nuisance), never its funds. Best-effort + PER-ISOLATE —
    // see api/_ratelimit.ts; the per-feed cooldown below still applies. ---------
    const token =
      req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
    const claimed = claimedAddress(token);
    if (claimed) {
      const wait = senderWindow.hit(claimed);
      if (wait > 0) {
        return json(
          {
            error: `rate limited: at most ${BROADCAST_SENDER_PER_MIN} broadcasts per 60s per sender`,
            retryAfterSeconds: wait,
          },
          429,
          origin,
        );
      }
    }

    // ---- AUTH — same token scheme + headers as notify.ts (verifyAuthToken in
    // _auth.ts is byte-for-byte the prior inlined parse/freshness/recovery). The
    // recovered identity is only GATED here (broadcast pushes to the feed's
    // subscribers, not the caller), so we don't bind the address further. -------
    const auth = verifyAuthToken(token, Math.floor(Date.now() / 1000), 'broadcast');
    if (!auth.ok) {
      return json({ error: auth.error }, auth.status, origin);
    }

    // ---- per-feed RATE LIMIT. Identity-gated broadcast is not owner-gated, so
    // (with broadcasts free) this is the anti-spam control. Per-isolate Map
    // (see RATE_LIMIT_MS comment). ---------------------------------------------
    const feedKey = targetId.toString();
    const nowMs = Date.now();
    const last = lastBroadcastAt.get(feedKey) ?? 0;
    if (nowMs - last < RATE_LIMIT_MS) {
      const retryAfter = Math.ceil((RATE_LIMIT_MS - (nowMs - last)) / 1000);
      return json(
        { error: `rate limited: at most one broadcast per ${RATE_LIMIT_MS / 1000}s per feed`, retryAfterSeconds: retryAfter },
        429,
        origin,
      );
    }

    // ---- FREE (rate-limited + identity-gated) --------------------------------
    // A Ready-Up broadcast is NOT metered: requiring $LH per tap would kill the
    // viral, low-friction "anyone can ping the group" use case. The spam
    // controls are the rate limits + the identity gate (a valid signed identity
    // is required to reach here).
    //
    // Mark the feed's last-broadcast time so the rate limit holds even if the
    // fan-out is slow.
    lastBroadcastAt.set(feedKey, nowMs);

    // ---- FAN-OUT: read the feed's subscriber set, push to each ----------------
    let allSubscribers: string[];
    try {
      allSubscribers = await subscribersOf(targetId);
    } catch (e) {
      // Surface the read failure but don't pretend we delivered. (Rare; the
      // rate-limit stamp still stands.)
      return json(
        { error: 'subscriber lookup failed: ' + (e as Error).message },
        502,
        origin,
      );
    }
    const totalSubscribers = allSubscribers.length;
    if (totalSubscribers === 0) {
      return json({ sent: 0, subscribers: 0, failed: 0 }, 200, origin);
    }
    const truncated = totalSubscribers > MAX_FANOUT;
    const targets = truncated ? allSubscribers.slice(0, MAX_FANOUT) : allSubscribers;

    // Resolve + push with BOUNDED CONCURRENCY (small batches) to be kind to the
    // public RPC and the push services. Per-subscriber best-effort: a failed
    // resolve or push counts as `failed` and NEVER aborts the rest.
    const BATCH = 10;
    let sent = 0;
    let failed = 0;
    let noTarget = 0; // subscriber with no published push_sub (not a failure)
    for (let i = 0; i < targets.length; i += BATCH) {
      const batch = targets.slice(i, i + BATCH);
      const results = await Promise.all(
        batch.map(async (addr): Promise<'sent' | 'failed' | 'none'> => {
          // The GitHub push store (`push-subs/<address>.json`) is the ONLY
          // enroll source. Never throws ([] on any failure).
          const subs = await storePushSubs(addr);
          if (subs.length === 0) return 'none'; // never enabled — not a failure
          // Fan out to EVERY device the subscriber enrolled; one acceptance
          // counts the subscriber as reached. sendWebPushAll never throws.
          const ok = await sendWebPushAll(subs, JSON.stringify({ title, body }), vapid);
          return ok > 0 ? 'sent' : 'failed';
        }),
      );
      for (const r of results) {
        if (r === 'sent') sent++;
        else if (r === 'failed') failed++;
        else noTarget++;
      }
    }

    return json(
      {
        sent,
        subscribers: targets.length,
        failed,
        ...(noTarget ? { noTarget } : {}),
        ...(truncated ? { truncated: true, totalSubscribers } : {}),
      },
      200,
      origin,
    );
  } catch (e) {
    return json({ error: (e as Error).message }, 500, origin);
  }
}
