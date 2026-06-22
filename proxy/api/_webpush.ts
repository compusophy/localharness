// Minimal Web Push sender on WebCrypto — RFC 8030 (push), RFC 8291
// (aes128gcm message encryption), RFC 8292 (VAPID).
//
// WHY NOT THE `web-push` NPM PACKAGE: every proxy function (gemini/mcp/
// scheduler) runs on Vercel's EDGE runtime (`config = { runtime: 'edge' }`),
// and `web-push` is built on Node's `https` + `crypto` modules, which the
// Edge runtime does not provide — it would throw at import time. This module
// is the same wire protocol hand-rolled on `crypto.subtle`, which exists on
// BOTH Edge and Node, so the scheduler can push regardless of runtime.
//
// The underscore prefix keeps Vercel from deploying this file as a route —
// it is an import-only helper for scheduler.ts.
//
// Scope: payload-bearing pushes (aes128gcm, the only widely-supported
// scheme) with VAPID auth. No retries, no topic/urgency knobs beyond TTL —
// a missed notification is acceptable; a failed push must never fail the
// scheduled run (the exported senders never throw).

export interface PushSubscriptionJson {
  endpoint: string;
  keys: { p256dh: string; auth: string };
  // Stable per-device id (src/app/notifications.rs stamps it). Optional: legacy
  // subs predate it. When present, dedupeSubs collapses entries sharing one
  // `dev` to a single delivery — the SAME physical device under two subdomain
  // origins has two DIFFERENT endpoints, so endpoint dedup alone double-buzzed
  // it (R5).
  dev?: string;
}

/**
 * Parse a push-sub SLOT value into validated subscriptions. Slots are
 * MULTI-DEVICE: a JSON array of subscription objects (the browser merges one
 * entry per device endpoint — src/registry/push.rs::merge_push_sub); legacy
 * single-object values still parse as a one-element list. Malformed entries
 * are skipped, never fatal. Returns [] for empty/unparsable input.
 */
export function parsePushSubs(text: string): PushSubscriptionJson[] {
  if (!text) return [];
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return [];
  }
  const arr = Array.isArray(v) ? v : [v];
  const out: PushSubscriptionJson[] = [];
  for (const e of arr) {
    const s = e as PushSubscriptionJson;
    if (
      typeof s?.endpoint === 'string' &&
      s.endpoint.startsWith('https://') &&
      typeof s.keys?.p256dh === 'string' &&
      typeof s.keys?.auth === 'string'
    ) {
      // Preserve the stable `dev` device id when present (used by dedupeSubs to
      // collapse one device's multiple endpoints); drop anything else.
      const dev = typeof s.dev === 'string' && s.dev ? s.dev : undefined;
      out.push(dev ? { endpoint: s.endpoint, keys: s.keys, dev } : { endpoint: s.endpoint, keys: s.keys });
    }
  }
  return out;
}

/**
 * Drop duplicate subscriptions, preserving order (first occurrence wins, which
 * is the MOST RECENT since slots are stored newest-first). Collapses by the
 * stable `dev` device id when present — the SAME physical device enrolled under
 * two subdomain origins holds two DIFFERENT endpoints in one address-keyed slot,
 * so endpoint dedup alone delivered to it twice (R5). Entries with no `dev`
 * (legacy) fall back to endpoint-keyed dedup.
 */
export function dedupeSubs(subs: PushSubscriptionJson[]): PushSubscriptionJson[] {
  const seenEndpoint = new Set<string>();
  const seenDev = new Set<string>();
  return subs.filter((s) => {
    if (s.dev) {
      if (seenDev.has(s.dev)) return false;
      seenDev.add(s.dev);
    }
    if (seenEndpoint.has(s.endpoint)) return false;
    seenEndpoint.add(s.endpoint);
    return true;
  });
}

/**
 * Fan a payload out to EVERY subscription (one POST per device, concurrent).
 * Returns the number of pushes the services accepted. Never throws.
 */
export async function sendWebPushAll(
  subs: PushSubscriptionJson[],
  payload: string,
  vapid: VapidEnv,
): Promise<number> {
  const results = await Promise.all(
    dedupeSubs(subs).map((s) => sendWebPush(s, payload, vapid)),
  );
  return results.filter(Boolean).length;
}

// ---- base64url <-> bytes ----------------------------------------------------

export function b64urlToBytes(s: string): Uint8Array {
  const b64 = s.replace(/-/g, '+').replace(/_/g, '/');
  const pad = b64.length % 4 === 0 ? '' : '='.repeat(4 - (b64.length % 4));
  const bin = atob(b64 + pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function bytesToB64url(b: Uint8Array | ArrayBuffer): string {
  const bytes = b instanceof Uint8Array ? b : new Uint8Array(b);
  let bin = '';
  for (const byte of bytes) bin += String.fromCharCode(byte);
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function utf8(s: string): Uint8Array {
  return new TextEncoder().encode(s);
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

// ---- HKDF-SHA256 (extract + expand in one WebCrypto deriveBits) -------------

async function hkdf(
  salt: Uint8Array,
  ikm: Uint8Array,
  info: Uint8Array,
  lengthBytes: number,
): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey('raw', ikm as BufferSource, 'HKDF', false, [
    'deriveBits',
  ]);
  const bits = await crypto.subtle.deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: salt as BufferSource, info: info as BufferSource },
    key,
    lengthBytes * 8,
  );
  return new Uint8Array(bits);
}

// ---- RFC 8291 message encryption (aes128gcm) --------------------------------

async function encryptPayload(
  sub: PushSubscriptionJson,
  plaintext: Uint8Array,
): Promise<Uint8Array> {
  const uaPublic = b64urlToBytes(sub.keys.p256dh); // 65-byte uncompressed point
  const authSecret = b64urlToBytes(sub.keys.auth); // 16 bytes
  if (uaPublic.length !== 65 || authSecret.length !== 16) {
    throw new Error('malformed subscription keys');
  }

  // Ephemeral application-server ECDH keypair (fresh per message).
  const asKeys = (await crypto.subtle.generateKey(
    { name: 'ECDH', namedCurve: 'P-256' },
    true,
    ['deriveBits'],
  )) as CryptoKeyPair;
  const asPublic = new Uint8Array(await crypto.subtle.exportKey('raw', asKeys.publicKey));

  const uaKey = await crypto.subtle.importKey(
    'raw',
    uaPublic as BufferSource,
    { name: 'ECDH', namedCurve: 'P-256' },
    false,
    [],
  );
  const ecdhSecret = new Uint8Array(
    await crypto.subtle.deriveBits({ name: 'ECDH', public: uaKey }, asKeys.privateKey, 256),
  );

  // IKM = HKDF(salt=auth_secret, ecdh_secret, "WebPush: info"||0||ua_pub||as_pub, 32)
  const keyInfo = concat(utf8('WebPush: info\0'), uaPublic, asPublic);
  const ikm = await hkdf(authSecret, ecdhSecret, keyInfo, 32);

  const salt = crypto.getRandomValues(new Uint8Array(16));
  const cek = await hkdf(salt, ikm, utf8('Content-Encoding: aes128gcm\0'), 16);
  const nonce = await hkdf(salt, ikm, utf8('Content-Encoding: nonce\0'), 12);

  // ONE record: plaintext || 0x02 (last-record delimiter), AES-128-GCM.
  const padded = concat(plaintext, new Uint8Array([0x02]));
  const aesKey = await crypto.subtle.importKey('raw', cek as BufferSource, 'AES-GCM', false, [
    'encrypt',
  ]);
  const ciphertext = new Uint8Array(
    await crypto.subtle.encrypt(
      { name: 'AES-GCM', iv: nonce as BufferSource },
      aesKey,
      padded as BufferSource,
    ),
  );

  // aes128gcm body header: salt(16) || rs(4, BE) || idlen(1) || keyid(=as_public)
  const header = new Uint8Array(16 + 4 + 1 + asPublic.length);
  header.set(salt, 0);
  new DataView(header.buffer).setUint32(16, 4096); // record size
  header[20] = asPublic.length;
  header.set(asPublic, 21);
  return concat(header, ciphertext);
}

// ---- RFC 8292 VAPID (ES256 JWT over the endpoint origin) --------------------

async function vapidAuthHeader(
  endpoint: string,
  publicKeyB64url: string,
  privateKeyB64url: string,
  subject: string,
): Promise<string> {
  const pub = b64urlToBytes(publicKeyB64url);
  if (pub.length !== 65) throw new Error('VAPID public key must be a 65-byte P-256 point');
  const jwk: JsonWebKey = {
    kty: 'EC',
    crv: 'P-256',
    x: bytesToB64url(pub.slice(1, 33)),
    y: bytesToB64url(pub.slice(33, 65)),
    d: privateKeyB64url,
    ext: true,
  };
  const key = await crypto.subtle.importKey(
    'jwk',
    jwk,
    { name: 'ECDSA', namedCurve: 'P-256' },
    false,
    ['sign'],
  );
  const header = bytesToB64url(utf8(JSON.stringify({ typ: 'JWT', alg: 'ES256' })));
  const claims = bytesToB64url(
    utf8(
      JSON.stringify({
        aud: new URL(endpoint).origin,
        exp: Math.floor(Date.now() / 1000) + 12 * 3600,
        sub: subject,
      }),
    ),
  );
  const signingInput = `${header}.${claims}`;
  // WebCrypto ECDSA emits the raw r||s (IEEE P1363) signature JWS wants.
  const sig = new Uint8Array(
    await crypto.subtle.sign(
      { name: 'ECDSA', hash: 'SHA-256' },
      key,
      utf8(signingInput) as BufferSource,
    ),
  );
  return `vapid t=${signingInput}.${bytesToB64url(sig)}, k=${publicKeyB64url}`;
}

// ---- the sender --------------------------------------------------------------

export interface VapidEnv {
  publicKey: string;
  privateKey: string;
  subject: string; // mailto: or https: contact, e.g. mailto:ops@example.com
}

/**
 * Encrypt `payload` to `sub` and POST it to the push service. NEVER throws —
 * returns true when the push service accepted (HTTP 201), false on any
 * failure (logged). Bounded by a 5s timeout so a slow push service can't eat
 * the Edge wall-clock budget of the scheduled run it decorates.
 */
export async function sendWebPush(
  sub: PushSubscriptionJson,
  payload: string,
  vapid: VapidEnv,
): Promise<boolean> {
  try {
    if (!sub?.endpoint || !sub.keys?.p256dh || !sub.keys?.auth) return false;
    const body = await encryptPayload(sub, utf8(payload));
    const authorization = await vapidAuthHeader(
      sub.endpoint,
      vapid.publicKey,
      vapid.privateKey,
      vapid.subject,
    );
    const res = await fetch(sub.endpoint, {
      method: 'POST',
      headers: {
        authorization,
        'content-encoding': 'aes128gcm',
        'content-type': 'application/octet-stream',
        ttl: '86400',
        urgency: 'normal',
      },
      body: body as BodyInit,
      signal: AbortSignal.timeout(5000),
    });
    if (!res.ok) {
      // 404/410 = subscription expired/unsubscribed (stale on-chain blob);
      // anything else = push-service hiccup. Either way: log + move on.
      console.warn(`[webpush] push service ${res.status} for ${new URL(sub.endpoint).origin}`);
      return false;
    }
    return true;
  } catch (e) {
    console.warn(`[webpush] send failed: ${(e as Error).message}`);
    return false;
  }
}
