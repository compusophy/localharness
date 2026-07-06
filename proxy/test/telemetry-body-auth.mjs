#!/usr/bin/env node
// api/telemetry.ts body-auth lane — the PANIC beacon. navigator.sendBeacon
// (the wasm panic hook's reporter) cannot set headers, so the pre-signed token
// rides in the JSON body (`auth`). Proves: body auth files an issue exactly
// like header auth, a bad body token 401s (no unauthed lane), and the header
// path is unchanged. GitHub is stubbed via globalThis.fetch — no network.

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';

process.env.GH_TELEMETRY_TOKEN = 'test-token';
const telemetryMod = await import('../.ttest/telemetry.js');
const handler = telemetryMod.default?.default ?? telemetryMod.default;

let failed = 0;
function ok(cond, label) {
  console.log(`${cond ? 'ok  ' : 'FAIL'} ${label}`);
  if (!cond) failed++;
}

// ---- mint a route-bound token the way the wasm client does ------------------
const PRIV = '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d';
const pub = secp256k1.getPublicKey(hexToBytes(PRIV.slice(2)), false);
const ADDR = '0x' + bytesToHex(keccak_256(pub.slice(1)).slice(12));

function personalSign(message) {
  const msg = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${msg.length}`);
  const full = new Uint8Array(prefix.length + msg.length);
  full.set(prefix, 0);
  full.set(msg, prefix.length);
  const sig = secp256k1.sign(keccak_256(full), hexToBytes(PRIV.slice(2)));
  return (
    '0x' +
    sig.r.toString(16).padStart(64, '0') +
    sig.s.toString(16).padStart(64, '0') +
    (27 + sig.recovery).toString(16).padStart(2, '0')
  );
}

function tokenFor(route) {
  const ts = Math.floor(Date.now() / 1000);
  return `${ADDR}:${ts}:${personalSign(`localharness-proxy:${ADDR.toLowerCase()}:${ts}:${route}`)}`;
}

// ---- stub GitHub -------------------------------------------------------------
let filedBodies = [];
globalThis.fetch = async (url, init) => {
  if (String(url).includes('/search/issues')) {
    return new Response(JSON.stringify({ items: [] }), { status: 200 });
  }
  filedBodies.push(JSON.parse(init.body));
  return new Response(JSON.stringify({ html_url: 'https://x/1', number: 1 }), { status: 201 });
};

const post = (payload, headers = {}) =>
  handler(
    new Request('https://proxy.test/api/telemetry', {
      method: 'POST',
      headers,
      body: JSON.stringify(payload),
    }),
  );

// body auth (the sendBeacon lane) files
{
  const res = await post({
    kind: 'panic',
    title: 'panicked at src/app/x.rs:1',
    signature: 'panic-abc',
    body: 'boom\n\nlast steps:\n· mount',
    auth: tokenFor('telemetry'),
  });
  const j = await res.json();
  ok(res.status === 200 && j.filed === true, `body-auth panic report files (${res.status})`);
  ok(
    filedBodies.length === 1 && filedBodies[0].title.startsWith('[panic] '),
    'issue titled [panic] …',
  );
}

// bad body token → 401 (no unauthed lane)
{
  const res = await post({ kind: 'panic', title: 't', auth: 'garbage' });
  ok(res.status === 401, `garbage body token 401s (${res.status})`);
}

// wrong-route body token → 401 (route binding holds in the body lane)
{
  const res = await post({ kind: 'panic', title: 't', auth: tokenFor('gemini') });
  // NOTE: a route-bound token for another route also fails the legacy fallback.
  ok(res.status === 401, `wrong-route body token 401s (${res.status})`);
}

// header auth unchanged
{
  filedBodies = [];
  const res = await post(
    { kind: 'error', title: 'header path', signature: 'sig-h', body: 'b' },
    { 'x-goog-api-key': tokenFor('telemetry') },
  );
  ok(res.status === 200 && filedBodies.length === 1, `header auth still files (${res.status})`);
}

process.exit(failed ? 1 : 0);
