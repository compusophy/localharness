#!/usr/bin/env node
// Parity gate for the SHARED auth primitives (api/_auth.ts, §5 dedup). Proves
// `verifyAuthToken` + `isAllowedOrigin` are behavior-identical to the logic that
// used to be inlined in fetch.ts / notify.ts / broadcast.ts / gemini.ts:
//   * an Ethereum personal-sign over `localharness-proxy:<addr>:<ts>` recovers
//     to the signer (the SAME \x19 prefix + keccak + ecrecover);
//   * the freshness window is exactly ±300s (boundary in / out);
//   * malformed tokens map to the SAME 401 error strings;
//   * the CORS origin allow-rules (apex, subdomains, http localhost/127.0.0.1)
//     match — and reject the localhost.evil.com prefix trap.
// No network. Run after the tsc step in test/run.sh (compiles _auth -> .ttest).

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';

import {
  verifyAuthToken,
  isAllowedOrigin,
  recoverAddress,
  FRESHNESS_WINDOW_SECS,
} from '../.ttest/_auth.js';

let failed = 0;
function ok(cond, label) {
  console.log(`${cond ? 'ok  ' : 'FAIL'} ${label}`);
  if (!cond) failed++;
}

// ---- sign a localharness auth token the way the browser/CLI client does ------
const PRIV = '0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d';
function addrOf(privHex) {
  const pub = secp256k1.getPublicKey(hexToBytes(privHex.replace(/^0x/, '')), false);
  return '0x' + bytesToHex(keccak_256(pub.slice(1)).slice(12));
}
const ADDR = addrOf(PRIV);

function personalSign(message, privHex) {
  const msg = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${msg.length}`);
  const full = new Uint8Array(prefix.length + msg.length);
  full.set(prefix, 0);
  full.set(msg, prefix.length);
  const digest = keccak_256(full);
  const sig = secp256k1.sign(digest, hexToBytes(privHex.replace(/^0x/, '')));
  const r = sig.r.toString(16).padStart(64, '0');
  const s = sig.s.toString(16).padStart(64, '0');
  const v = (27 + sig.recovery).toString(16).padStart(2, '0');
  return '0x' + r + s + v;
}

function tokenAt(ts, addr = ADDR, priv = PRIV) {
  const sig = personalSign(`localharness-proxy:${addr.toLowerCase()}:${ts}`, priv);
  return `${addr}:${ts}:${sig}`;
}

ok(recoverAddress(`localharness-proxy:${ADDR.toLowerCase()}:1000`, personalSign(`localharness-proxy:${ADDR.toLowerCase()}:1000`, PRIV)).toLowerCase() === ADDR.toLowerCase(), 'recoverAddress round-trips');
ok(FRESHNESS_WINDOW_SECS === 300, 'freshness window is 300s');

// ---- verifyAuthToken: happy path + boundaries -------------------------------
const now = 1_700_000_000;
{
  const r = verifyAuthToken(tokenAt(now), now);
  ok(r.ok === true && r.address === ADDR, 'fresh token verifies + returns claimed address');
}
{
  const r = verifyAuthToken(tokenAt(now - 300), now);
  ok(r.ok === true, 'token exactly 300s old still fresh (boundary in)');
}
{
  const r = verifyAuthToken(tokenAt(now - 301), now);
  ok(r.ok === false && r.status === 401 && r.error === 'stale or future timestamp', '301s old is stale (boundary out)');
}
{
  const r = verifyAuthToken(tokenAt(now + 301), now);
  ok(r.ok === false && r.error === 'stale or future timestamp', 'future timestamp rejected');
}

// ---- malformed token error-string parity ------------------------------------
ok(verifyAuthToken('', now).error === 'missing or malformed auth token', 'empty token -> missing/malformed');
ok(verifyAuthToken('a:b', now).error === 'missing or malformed auth token', '2-part token -> missing/malformed');
// `${ADDR}::sig` -> tsStr '' -> Number('')===0 (finite) -> passes the early
// shape checks, then timestamp 0 is ~now-1.7e9s old -> stale. This matches the
// ORIGINAL inlined behavior (Number('') === 0), so the shared code is identical.
ok(verifyAuthToken(`${ADDR}::sig`, now).error === 'stale or future timestamp', 'empty timestamp (===0) -> stale, like the original');
ok(verifyAuthToken(`0xnothex:${now}:0x00`, now).error === 'malformed auth token: address', 'bad address -> address error');
ok(verifyAuthToken(`${ADDR}:1.5:0x00`, now).error === 'malformed auth token: timestamp', 'fractional ts -> timestamp error');
{
  // valid shape, wrong signer -> signature does not match (recovery succeeds to a
  // different address) OR a bad-signature throw; both are 401.
  const bad = `${ADDR}:${now}:0x${'11'.repeat(65)}`;
  const r = verifyAuthToken(bad, now);
  ok(r.ok === false && r.status === 401, 'garbage 65-byte signature -> 401');
}
{
  // signature by a DIFFERENT key over the claimed address's preimage.
  const otherPriv = '0x' + '22'.repeat(32);
  const sig = personalSign(`localharness-proxy:${ADDR.toLowerCase()}:${now}`, otherPriv);
  const r = verifyAuthToken(`${ADDR}:${now}:${sig}`, now);
  ok(r.ok === false && r.error === 'signature does not match address', 'wrong-signer -> mismatch');
}

// ---- isAllowedOrigin parity -------------------------------------------------
ok(isAllowedOrigin('https://localharness.xyz') === true, 'apex allowed');
ok(isAllowedOrigin('https://claude.localharness.xyz') === true, 'subdomain allowed');
ok(isAllowedOrigin('http://localhost') === true, 'http localhost allowed');
ok(isAllowedOrigin('http://127.0.0.1') === true, 'http 127.0.0.1 allowed');
ok(isAllowedOrigin('http://localhost.evil.com') === false, 'localhost.evil.com prefix trap REJECTED');
ok(isAllowedOrigin('https://evil.com') === false, 'foreign origin rejected');
ok(isAllowedOrigin('https://notlocalharness.xyz') === false, 'non-subdomain rejected');
ok(isAllowedOrigin('https://localharness.xyz.evil.com') === false, 'apex-prefix trap rejected');

if (failed) {
  console.error(`\n${failed} auth-parity case(s) FAILED`);
  process.exit(1);
}
console.log('\nall auth-parity cases pass');
