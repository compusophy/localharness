// localharness credit proxy — NOTIFY route (Edge).
//
// POST /api/notify { title, body, to? } → Web-Pushes a note.
//
//   * No `to`  — the CALLER's OWN registered device ("notify me when done"
//     from a shell, on-chain feedback #69).
//   * `to: <name>` — CROSS-AGENT: deliver to the named agent's enrolled
//     device(s). The sender is the AUTHENTICATED caller; the push title is
//     prefixed `@<callerName>:` (resolved from chain, never from the request)
//     so the recipient's inbox shows who pinged them and spoofing is
//     impossible. The meter debit (caller pays per push, same price as a
//     model call) is the spam leash; per-recipient blocklists are follow-up.
//
// Target resolution (both modes): the GitHub push store
// (`push-subs/<owner>.json`, _pushstore.ts) — the ONLY enroll path (POST
// /api/push-sub from the header bell / app open); the blob holds a JSON ARRAY
// of per-device subscriptions and a push fans out to ALL of them.
//
// DEAD-SUB PRUNING (telemetry #40): a push service 404/410 means the
// subscription is expired/unsubscribed — it is PRUNED from the store blob
// (best-effort) and, when NO endpoint accepted, the caller gets an honest
// "no live push subscription … re-enroll" instead of a generic send failure.
// Without this a stale endpoint was re-served forever and every closed-tab
// push died silently.
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
// validation → VAPID config check → SENDER rate limit (429 before auth — cheap
// rejection on the CLAIMED address) → auth → RECIPIENT rate limit (a victim's
// window must only be burnable by an AUTHENTICATED sender) → subscription
// lookup (before any debit) → credit gate + meter debit → the push itself
// (502 on failure).
//
// CROSS-AGENT DELIVERY IS ALWAYS POSSIBLE (so the debit is justified): a `to:`
// note is durably RECORDED in the recipient's on-chain MessageFacet inbox even
// with NO Web Push device enrolled — it surfaces in their bell at next open
// (import_onchain_messages; web/sw.js), and a live Web Push is layered on top
// only when a device IS enrolled. So a cross-agent note PROCEEDS and IS metered
// (the on-chain record is real delivery), returning `enrolled: <bool>` +
// `delivered`/`devices`. Only a SELF note with no device is a 404 — its sole
// channel is the caller's own push, so there is nothing to record. (The debit-
// before-delivery order means the one thing a debited caller pays for is a rare
// TOTAL failure where BOTH the on-chain record AND the push fail — a documented
// tradeoff of the meter-is-the-spam-filter model, not an undeliverable charge.)
//
// RATE LIMITS (best-effort, PER-ISOLATE — see api/_ratelimit.ts for why
// that's accepted; the meter debit stays the global hard backstop):
//   * per SENDER: ≤ NOTIFY_SENDER_PER_MIN pushes/min — a funded loop can't
//     buzz a phone continuously even though each push is paid. SELF-keyed on
//     the claimed address, so checked PRE-AUTH: worst case a spoofer burns
//     THAT address's window (a nuisance), never its funds — debits only happen
//     after real auth.
//   * per RECIPIENT (`to` only): ≤ NOTIFY_RECIPIENT_PER_MIN deliveries/min to
//     one target name ACROSS ALL SENDERS in this isolate — many funded
//     senders can't gang up on one phone. Keyed on the TARGET, not the caller,
//     so checked POST-AUTH: an unauthenticated request must not be able to burn
//     a victim's recipient window and block legit cross-agent notifies.

import { sendWebPushAllDetailed, type PushSubscriptionJson } from './_webpush';
import { recordOnChainMessage } from './_message';
import { storePushSubs, pruneStorePushSubs } from './_pushstore';
import { SlidingWindow, claimedAddress } from './_ratelimit';

export const config = { runtime: 'edge' };

// ---- constants (mirror api/fetch.ts) ----------------------------------------

// Auth + metering primitives (CORS allow-check, personal-sign recovery +
// freshness, the creditOf/sessionExpiryOf reads, the generic eth_call, the
// meter debit) are SHARED in `_auth.ts` (§5 dedup) — byte-for-byte the logic
// that used to be inlined here.
import {
  isAllowedOrigin,
  verifyAuthToken,
  selector,
  encodeAddressWord,
  stripHex,
  ethCall,
  sessionExpiryOf,
  creditOf,
  meterDebit,
  InsufficientCreditError,
} from './_auth';

// SAME per-request price as a model call — the platform FLOOR price for any paid
// capability. Single source of truth: `_prices.ts` (default 1 $LH,
// env-overridable via COST_PER_REQUEST_WEI). A self-push is a paid capability
// like a model turn (the meter IS the spam filter — see the header).
import { COST_PER_REQUEST_WEI } from './_prices';

// Payload bounds. Pushes are glanceable phone banners, not documents —
// overlong inputs are TRIMMED (whitespace) then TRUNCATED, never rejected.
const MAX_TITLE_CHARS = 80;
const MAX_BODY_CHARS = 200;
const MAX_REQUEST_BODY_BYTES = 16_384; // { title, body } is tiny

// Rate limits (best-effort, PER-ISOLATE — see api/_ratelimit.ts + the header).
// Sender: 10/min is generous for legit "notify me when done" agent loops but
// kills a tight buzz loop. Recipient: 10/min to one target name across ALL
// senders in this isolate — a phone gets at most ~one banner every 6s from
// here even if many funded senders pile on. The meter (1 $LH/push, the
// platform floor) stays the global hard backstop an isolate-spread attacker
// still pays.
const NOTIFY_SENDER_PER_MIN = 10;
const NOTIFY_RECIPIENT_PER_MIN = 10;
const senderWindow = new SlidingWindow(NOTIFY_SENDER_PER_MIN, 60_000);
const recipientWindow = new SlidingWindow(NOTIFY_RECIPIENT_PER_MIN, 60_000);

// The MessageFacet inbox writer (sendMessage) + its 1024-byte body cap live in
// the shared `_message.ts` — both this route (cross-agent no-push fallback) and
// api/sponsor.ts (welcome-on-creation) record notes through it.

// ---- CORS (same policy as fetch.ts/gemini.ts; isAllowedOrigin shared via
// _auth.ts) --------------------------------------------------------------------

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

/** ABI-encode a single dynamic `string` argument (offset + len + padded). */
function encodeStringArg(s: string): string {
  const bytes = new TextEncoder().encode(s);
  const padded = Math.ceil(bytes.length / 32) * 32;
  let hex = '';
  for (const b of bytes) hex += b.toString(16).padStart(2, '0');
  return (
    (32).toString(16).padStart(64, '0') +
    bytes.length.toString(16).padStart(64, '0') +
    hex.padEnd(padded * 2, '0')
  );
}

/** Thrown by pushSubsOfName when the name isn't registered at all. */
class NoSuchAgentError extends Error {}

/**
 * A NAMED agent's device subscriptions (cross-agent notify): name → tokenId →
 * owner → the owner's push-store blob. Throws NoSuchAgentError for an
 * unregistered name; `subs` is [] when the agent exists but no device ever
 * enrolled. `toId` is the recipient's tokenId — kept so a no-subscription
 * delivery can still RECORD the note on-chain (MessageFacet inbox) for the
 * recipient to read at boot (#35). `owner` is kept so dead store subs
 * discovered at send time can be PRUNED.
 */
async function pushSubsOfName(
  name: string,
): Promise<{ subs: PushSubscriptionJson[]; toId: bigint; owner: string }> {
  const id = BigInt(
    await ethCall('0x' + selector('idOfName(string)') + encodeStringArg(name)),
  );
  if (id === 0n) throw new NoSuchAgentError(`no agent named "${name}"`);
  const ownerWord = await ethCall(
    '0x' + selector('ownerOf(uint256)') + id.toString(16).padStart(64, '0'),
  );
  const owner = '0x' + stripHex(ownerWord).slice(-40);
  return { subs: await storePushSubs(owner), toId: id, owner };
}

/**
 * The CALLER's display name for cross-agent attribution: `mainNameOf(caller)`
 * when a MAIN identity exists, else the short 0x address. Chain-derived from
 * the AUTHENTICATED address — a sender cannot spoof who a push is from.
 */
async function callerDisplayName(address: string): Promise<string> {
  try {
    const name = decodeAbiBytesUtf8(
      await ethCall('0x' + selector('mainNameOf(address)') + encodeAddressWord(address)),
    ).trim();
    if (name) return name;
  } catch {
    // fall through to the short address
  }
  return address.slice(0, 6) + '…' + address.slice(-4);
}

// InsufficientCreditError + meterDebit moved to the shared `_auth.ts`.
// recordOnChainMessage moved to the shared `_message.ts` (see import above).

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
    let to: string;
    try {
      const parsed = (await req.json()) as {
        title?: unknown;
        body?: unknown;
        to?: unknown;
      };
      title = typeof parsed.title === 'string' ? parsed.title.trim() : '';
      body = typeof parsed.body === 'string' ? parsed.body.trim() : '';
      to = typeof parsed.to === 'string' ? parsed.to.trim().toLowerCase() : '';
    } catch {
      return json({ error: 'invalid JSON body' }, 400, origin);
    }
    if (!title) {
      return json({ error: 'missing title' }, 400, origin);
    }
    if (to && !/^[a-z0-9-]{1,63}$/.test(to)) {
      return json({ error: 'invalid `to` agent name' }, 400, origin);
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

    // ---- SENDER RATE LIMIT (BEFORE auth — rejecting a flood must not cost a
    // curve recovery per request). Keyed on the CLAIMED, unverified address;
    // safe because nothing of value is gated here and the window is SELF-keyed
    // — a spoofer burns only THAT address's per-isolate window (a one-minute
    // nuisance), never its funds: the meter debit below only ever runs after
    // real signature verification. Best-effort + PER-ISOLATE — _ratelimit.ts. --
    const token =
      req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
    const claimed = claimedAddress(token);
    if (claimed) {
      const wait = senderWindow.hit(claimed);
      if (wait > 0) {
        return json(
          {
            error: `rate limited: at most ${NOTIFY_SENDER_PER_MIN} notifies per 60s per sender`,
            retryAfterSeconds: wait,
          },
          429,
          origin,
        );
      }
    }

    // ---- AUTH — same token scheme + headers as api/gemini.ts (verifyAuthToken
    // in _auth.ts is byte-for-byte the prior inlined parse/freshness/recovery) --
    const now = Math.floor(Date.now() / 1000);
    // Route-bind the token to this endpoint (audit L9).
    const auth = verifyAuthToken(token, now, 'notify');
    if (!auth.ok) {
      return json({ error: auth.error }, auth.status, origin);
    }
    const address = auth.address;

    // ---- RECIPIENT RATE LIMIT (AFTER auth — the window is keyed on the TARGET
    // name, not the caller, so it must only be consumable by an AUTHENTICATED
    // sender; checked pre-auth, an anonymous request could burn a victim's
    // window with a garbage token and block legit cross-agent notifies to them).
    // Per-RECIPIENT cap (cross-agent only): one phone can't be buzzed
    // continuously even by MANY funded senders — deliveries to a target name
    // share one window across all senders in this isolate.
    if (to) {
      const wait = recipientWindow.hit(to);
      if (wait > 0) {
        return json(
          {
            error: `rate limited: "${to}" can receive at most ${NOTIFY_RECIPIENT_PER_MIN} notifies per 60s across all senders`,
            retryAfterSeconds: wait,
          },
          429,
          origin,
        );
      }
    }

    // ---- subscription lookup (BEFORE the debit — an undeliverable note must
    // not be charged). Cross-agent (`to`) also stamps WHO it's from, and keeps
    // the recipient's tokenId so a no-push note can still be RECORDED on-chain.
    let subs: PushSubscriptionJson[];
    let recipientId = 0n; // recipient tokenId (cross-agent only); 0 = self/none
    let subsOwner = address; // whose store blob holds the subs (for pruning)
    try {
      if (to) {
        const r = await pushSubsOfName(to);
        subs = r.subs;
        recipientId = r.toId;
        subsOwner = r.owner;
      } else {
        subs = await storePushSubs(address);
      }
    } catch (e) {
      if (e instanceof NoSuchAgentError) {
        return json({ error: e.message }, 404, origin);
      }
      return json({ error: 'subscription lookup failed: ' + (e as Error).message }, 502, origin);
    }
    // SELF with no subscription: there's nothing the proxy can record on the
    // CALLER's behalf (a self-note's only channel is the caller's own push), so
    // the 404 + "enable notifications" hint is the right answer. A CROSS-AGENT
    // note ALWAYS proceeds — even with no push sub it is durably RECORDED in the
    // recipient's on-chain inbox below (#35), so it surfaces in their bell at
    // next open whether or not they have a Web Push device.
    if (to === '' && subs.length === 0) {
      return json(
        { error: 'no push subscription enrolled — enable notifications in the app first (tap the header bell)' },
        404,
        origin,
      );
    }
    if (to) {
      const from = await callerDisplayName(address);
      title = `@${from}: ${title}`.slice(0, MAX_TITLE_CHARS);
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

    // ---- delivery — the one failure a debited caller pays for ------------------
    // The { title, body } JSON the service worker (web/sw.js) renders for a push
    // AND the exact payload recorded on-chain — so the recipient's on-chain
    // import folds a bell entry byte-identical to a live/stashed push and dedups
    // the two (src/app/notifications.rs::import_onchain_messages). `title`
    // already carries the `@<from>:` attribution for a cross-agent note.
    const payload = JSON.stringify({ title, body });

    if (to) {
      // CROSS-AGENT: ALWAYS record a durable copy in the recipient's on-chain
      // MessageFacet inbox, regardless of push. A Web Push to a closed/
      // backgrounded PWA tab is best-effort and frequently never reaches the
      // in-app log (mobile SW-OPFS write failures, no re-mount) — the on-chain
      // record is the RELIABLE channel: it surfaces in their bell via
      // import_onchain_messages at next open. Web Push, when the recipient is
      // enrolled, is layered on top as the realtime banner. Run both in
      // parallel and succeed if EITHER lands (the on-chain copy alone is enough
      // for the in-app log; the push alone is enough for the live banner).
      let recorded = false;
      let sent = 0;
      let gone: string[] = [];
      await Promise.all([
        recordOnChainMessage(recipientId, payload)
          .then(() => {
            recorded = true;
          })
          .catch((e) => {
            console.warn(
              'notify: on-chain record failed (push may still fire):',
              (e as Error).message,
            );
          }),
        subs.length > 0
          ? sendWebPushAllDetailed(subs, payload, { publicKey, privateKey, subject })
              .then((r) => {
                sent = r.sent;
                gone = r.gone;
              })
              .catch(() => {})
          : Promise.resolve(),
      ]);
      // Dead subscriptions (push service 404/410): prune them from the store
      // so the next notify doesn't ship to a corpse (#40). Best-effort.
      const pruned = gone.length ? await pruneStorePushSubs(subsOwner, gone) : 0;
      if (!recorded && sent === 0) {
        return json(
          {
            error:
              gone.length > 0
                ? `could not deliver: on-chain record failed and the recipient's ${gone.length} push endpoint(s) are expired (pruned) — they must re-open the app to re-enroll`
                : 'could not deliver: on-chain record and push both failed',
          },
          502,
          origin,
        );
      }
      return json(
        {
          sent: true,
          recorded,
          delivered: sent > 0,
          devices: sent,
          enrolled: subs.length > 0,
          to,
          ...(pruned > 0 ? { pruned } : {}),
        },
        200,
        origin,
      );
    }

    // SELF-notify: the caller's own devices — push only. FAN-OUT to every
    // enrolled device; success = at least one push service accepted. Dead
    // subscriptions (404/410) are pruned and reported HONESTLY: "you have no
    // live subscription" beats a generic send failure (#40).
    const { sent, gone } = await sendWebPushAllDetailed(subs, payload, {
      publicKey,
      privateKey,
      subject,
    });
    const pruned = gone.length ? await pruneStorePushSubs(address, gone) : 0;
    if (sent === 0) {
      return json(
        {
          error:
            gone.length > 0
              ? `no live push subscription — ${gone.length} enrolled endpoint(s) expired (pruned); open the app on the device and tap the bell to re-enroll`
              : 'push send failed (service rejected or timed out)',
        },
        502,
        origin,
      );
    }
    return json({ sent: true, devices: sent, ...(pruned > 0 ? { pruned } : {}) }, 200, origin);
  } catch (e) {
    return json({ error: (e as Error).message }, 500, origin);
  }
}
