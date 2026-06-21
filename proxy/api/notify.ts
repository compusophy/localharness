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
// Target resolution (both modes) is the UNION of every slot a device can
// enroll under, fanned out to ALL devices (each slot holds a JSON ARRAY of
// per-device subscriptions — src/registry/push.rs::merge_push_sub):
//   1. `metadata(mainOf(owner), keccak256("localharness.push_sub"))` — the
//      admin "enable notifications" flow (src/app/notifications.rs);
//   2. `metadata(tokenId, ...)` — same slot keyed by the name's own id;
//   3. `pushSubOf(owner)` — the address-keyed PushFacet slot the header
//      bell's device self-registration writes.
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
// validation → VAPID config check → RATE LIMIT (429 before auth — cheap
// rejection on the CLAIMED address) → auth → subscription lookup (before any
// debit) → credit gate + meter debit → the push itself (502 on failure).
//
// CROSS-AGENT ENROLLMENT CHECK: when a `to:` target has NO device enrolled for
// Web Push, the note cannot be delivered anywhere (the in-app inbox is fed by
// push too — web/sw.js), so this returns a clear, structured `enrolled: false`
// 200 with NO debit instead of a 404 — the sender did nothing wrong and must
// not be told to retry. The client tool / CLI relay the `message` verbatim.
//
// RATE LIMITS (best-effort, PER-ISOLATE — see api/_ratelimit.ts for why
// that's accepted; the meter debit stays the global hard backstop):
//   * per SENDER: ≤ NOTIFY_SENDER_PER_MIN pushes/min — a funded loop can't
//     buzz a phone continuously even though each push is paid;
//   * per RECIPIENT (`to` only): ≤ NOTIFY_RECIPIENT_PER_MIN deliveries/min to
//     one target name ACROSS ALL SENDERS in this isolate — many funded
//     senders can't gang up on one phone.
// Checked pre-auth on the claimed address: worst case a spoofer burns a
// window (nuisance), never funds — debits only happen after real auth.

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
import {
  parsePushSubs,
  dedupeSubs,
  sendWebPushAll,
  type PushSubscriptionJson,
} from './_webpush';
import { recordOnChainMessage } from './_message';
import { SlidingWindow, claimedAddress } from './_ratelimit';

export const config = { runtime: 'edge' };

// ---- constants (mirror api/fetch.ts) ----------------------------------------

import { TEMPO_RPC, REGISTRY, CHAIN_ID } from './_chain';

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

// Rate limits (best-effort, PER-ISOLATE — see api/_ratelimit.ts + the header).
// Sender: 10/min is generous for legit "notify me when done" agent loops but
// kills a tight buzz loop. Recipient: 10/min to one target name across ALL
// senders in this isolate — a phone gets at most ~one banner every 6s from
// here even if many funded senders pile on. The meter (0.01 $LH/push) stays
// the global hard backstop an isolate-spread attacker still pays.
const NOTIFY_SENDER_PER_MIN = 10;
const NOTIFY_RECIPIENT_PER_MIN = 10;
const senderWindow = new SlidingWindow(NOTIFY_SENDER_PER_MIN, 60_000);
const recipientWindow = new SlidingWindow(NOTIFY_RECIPIENT_PER_MIN, 60_000);

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

// The MessageFacet inbox writer (sendMessage) + its 1024-byte body cap live in
// the shared `_message.ts` — both this route (cross-agent no-push fallback) and
// api/sponsor.ts (welcome-on-creation) record notes through it.

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

/** `metadata(tokenId, push_sub_key)` → validated subscriptions ([] if none). */
async function subsFromMetadata(tokenId: bigint): Promise<PushSubscriptionJson[]> {
  if (tokenId === 0n) return [];
  const data =
    '0x' +
    selector('metadata(uint256,bytes32)') +
    tokenId.toString(16).padStart(64, '0') +
    PUSH_SUB_KEY;
  return parsePushSubs(decodeAbiBytesUtf8(await ethCall(data)).trim());
}

/** Address-keyed PushFacet slot (`pushSubOf(address)`) → subscriptions. */
async function subsFromAddress(address: string): Promise<PushSubscriptionJson[]> {
  const data = '0x' + selector('pushSubOf(address)') + encodeAddressWord(address);
  return parsePushSubs(decodeAbiBytesUtf8(await ethCall(data)).trim());
}

/**
 * Resolve an OWNER ADDRESS to ALL its enrolled device subscriptions — the
 * UNION of every slot a device registers under (see the header), deduped by
 * endpoint. MULTI-DEVICE by design: a phone and a desktop on the same seed
 * each hold an entry; a push fans out to every one (a first-match rule
 * silently dropped every device but one). `tokenId` is the name's own id
 * when targeting by name (0n when targeting the caller).
 */
async function resolveSubsForOwner(
  owner: string,
  tokenId: bigint,
): Promise<PushSubscriptionJson[]> {
  const main = BigInt(
    await ethCall('0x' + selector('mainOf(address)') + encodeAddressWord(owner)),
  );
  const [a, b, c] = await Promise.all([
    subsFromMetadata(main),
    main === tokenId ? Promise.resolve([]) : subsFromMetadata(tokenId),
    subsFromAddress(owner),
  ]);
  return dedupeSubs([...a, ...b, ...c]);
}

/** The CALLER's own device subscriptions (self-notify). */
async function pushSubsOfCaller(address: string): Promise<PushSubscriptionJson[]> {
  return resolveSubsForOwner(address, 0n);
}

/** Thrown by pushSubsOfName when the name isn't registered at all. */
class NoSuchAgentError extends Error {}

/**
 * A NAMED agent's device subscriptions (cross-agent notify): name → tokenId →
 * owner → the slot union. Throws NoSuchAgentError for an unregistered name;
 * `subs` is [] when the agent exists but no device ever enrolled. `toId` is the
 * recipient's tokenId — kept so a no-subscription delivery can still RECORD the
 * note on-chain (MessageFacet inbox) for the recipient to read at boot (#35).
 */
async function pushSubsOfName(
  name: string,
): Promise<{ subs: PushSubscriptionJson[]; toId: bigint }> {
  const id = BigInt(
    await ethCall('0x' + selector('idOfName(string)') + encodeStringArg(name)),
  );
  if (id === 0n) throw new NoSuchAgentError(`no agent named "${name}"`);
  const ownerWord = await ethCall(
    '0x' + selector('ownerOf(uint256)') + id.toString(16).padStart(64, '0'),
  );
  const owner = '0x' + stripHex(ownerWord).slice(-40);
  return { subs: await resolveSubsForOwner(owner, id), toId: id };
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

    // ---- RATE LIMIT (BEFORE auth — rejecting a flood must not cost a curve
    // recovery per request). Keyed on the CLAIMED, unverified address; safe
    // because nothing of value is gated here — a spoofer burns the address's
    // per-isolate rate window (a one-minute nuisance), never its funds: the
    // meter debit below only ever runs after real signature verification.
    // Best-effort + PER-ISOLATE — see api/_ratelimit.ts. ------------------------
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

    // ---- AUTH — same token scheme + headers as api/gemini.ts -------------------
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

    // ---- subscription lookup (BEFORE the debit — an undeliverable note must
    // not be charged). Cross-agent (`to`) also stamps WHO it's from, and keeps
    // the recipient's tokenId so a no-push note can still be RECORDED on-chain.
    let subs: PushSubscriptionJson[];
    let recipientId = 0n; // recipient tokenId (cross-agent only); 0 = self/none
    try {
      if (to) {
        const r = await pushSubsOfName(to);
        subs = r.subs;
        recipientId = r.toId;
      } else {
        subs = await pushSubsOfCaller(address);
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
        { error: 'no push subscription on-chain — enable notifications in the app first' },
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
          ? sendWebPushAll(subs, payload, { publicKey, privateKey, subject })
              .then((n) => {
                sent = n;
              })
              .catch(() => {})
          : Promise.resolve(),
      ]);
      if (!recorded && sent === 0) {
        return json(
          { error: 'could not deliver: on-chain record and push both failed' },
          502,
          origin,
        );
      }
      return json(
        { sent: true, recorded, delivered: sent > 0, devices: sent, enrolled: subs.length > 0, to },
        200,
        origin,
      );
    }

    // SELF-notify: the caller's own devices — push only. FAN-OUT to every
    // enrolled device; success = at least one push service accepted.
    const sent = await sendWebPushAll(subs, payload, { publicKey, privateKey, subject });
    if (sent === 0) {
      return json({ error: 'push send failed (service rejected or timed out)' }, 502, origin);
    }
    return json({ sent: true, devices: sent }, 200, origin);
  } catch (e) {
    return json({ error: (e as Error).message }, 500, origin);
  }
}
