#!/usr/bin/env node
// Unit test for _webpush.ts parsePushSubs + dedupeSubs (R5: per-device dedup).
// Proves two different-endpoint subs sharing one `dev` collapse to ONE delivery,
// while distinct devices and legacy (no-dev) subs are preserved.

import webpushMod from '../.ttest/_webpush.js';
const { parsePushSubs, dedupeSubs } = webpushMod;

let failed = false;
function ok(name, cond, detail = '') {
  if (cond) console.log(`ok   ${name}`);
  else {
    console.error(`FAIL ${name} ${detail}`);
    failed = true;
  }
}

const sub = (ep, dev) => ({
  endpoint: 'https://push.example/' + ep,
  keys: { p256dh: 'k', auth: 'a' },
  ...(dev ? { dev } : {}),
});

// --- parsePushSubs preserves `dev` -------------------------------------------
{
  const parsed = parsePushSubs(JSON.stringify([sub('a', 'device-1')]));
  ok('parse: preserves dev', parsed.length === 1 && parsed[0].dev === 'device-1', JSON.stringify(parsed));
}
{
  const parsed = parsePushSubs(JSON.stringify([sub('a')]));
  ok('parse: legacy sub has no dev', parsed.length === 1 && parsed[0].dev === undefined, JSON.stringify(parsed));
}

// --- two different-endpoint subs, SAME dev -> ONE delivery (R5) --------------
{
  const subs = [sub('origin-a', 'device-1'), sub('origin-b', 'device-1')];
  const deduped = dedupeSubs(subs);
  ok(
    'dedupe: same dev, two endpoints -> one send',
    deduped.length === 1 && deduped[0].endpoint === 'https://push.example/origin-a',
    JSON.stringify(deduped),
  );
}

// --- distinct devices coexist -----------------------------------------------
{
  const deduped = dedupeSubs([sub('a', 'dev-phone'), sub('b', 'dev-desktop')]);
  ok('dedupe: distinct devs preserved', deduped.length === 2, JSON.stringify(deduped));
}

// --- legacy subs (no dev) still endpoint-deduped -----------------------------
{
  const deduped = dedupeSubs([sub('a'), sub('a'), sub('b')]);
  ok('dedupe: legacy endpoint dedup', deduped.length === 2, JSON.stringify(deduped));
}

// --- mixed: a dev-tagged + a legacy with the same endpoint collapse ----------
{
  const deduped = dedupeSubs([sub('a', 'dev-1'), sub('a')]);
  ok('dedupe: same endpoint across dev/legacy collapses', deduped.length === 1, JSON.stringify(deduped));
}

// --- sendWebPushAllDetailed: 410 -> gone, 201 -> sent (telemetry #40) --------
// Real crypto path (WebCrypto exists in Node), mocked push service.
{
  const { sendWebPushAllDetailed } = webpushMod;
  const { webcrypto } = await import('node:crypto');
  const b64url = (b) =>
    Buffer.from(b).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  const ua = await webcrypto.subtle.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, true, ['deriveBits']);
  const uaPub = new Uint8Array(await webcrypto.subtle.exportKey('raw', ua.publicKey));
  const vapidKp = await webcrypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign']);
  const vapidPub = new Uint8Array(await webcrypto.subtle.exportKey('raw', vapidKp.publicKey));
  const vapidJwk = await webcrypto.subtle.exportKey('jwk', vapidKp.privateKey);
  const mkSub = (ep) => ({
    endpoint: ep,
    keys: { p256dh: b64url(uaPub), auth: b64url(webcrypto.getRandomValues(new Uint8Array(16))) },
  });
  const realFetch = globalThis.fetch;
  globalThis.fetch = async (url) =>
    new Response('', { status: String(url).includes('dead') ? 410 : 201 });
  const r = await sendWebPushAllDetailed(
    [mkSub('https://push.example/dead'), mkSub('https://push.example/live')],
    JSON.stringify({ title: 't', body: 'b' }),
    { publicKey: b64url(vapidPub), privateKey: vapidJwk.d, subject: 'mailto:test@example.com' },
  );
  globalThis.fetch = realFetch;
  ok(
    'detailed: 410 classified gone, 201 sent',
    r.sent === 1 && r.gone.length === 1 && r.gone[0] === 'https://push.example/dead',
    JSON.stringify(r),
  );
}

if (failed) {
  console.error('\nwebpush-dedupe test FAILED');
  process.exit(1);
}
console.log('\nall webpush-dedupe cases pass');
