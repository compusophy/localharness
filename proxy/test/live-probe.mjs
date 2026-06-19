#!/usr/bin/env node
// LIVE money-path probe (NO on-chain submit, NO spend). Hits the deployed relay
// with a valid mainnet `register` intent signed by a FRESH zero-balance key, and
// verifies the relay signs the fee_payer half with the REAL sponsor key. The
// relay only SIGNS — it never broadcasts — so this moves no money; it just
// proves the live endpoint is wired to the funded, §4-distinct sponsor.
//   node test/live-probe.mjs

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import {
  feePayerHash,
  sponsoredSenderHash,
  signHash65,
  recoverAddressFromDigest,
  addressFromPrivKey,
} from '../.ttest/_tempo.js';

const URL = 'https://proxy-tau-ten-15.vercel.app/api/sponsor';
const CHAIN_ID = 4217n;
const DIAMOND = '0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77'; // mainnet REGISTRY
const FEE_TOKEN = '20c000000000000000000000b9537d11c60e8b50'; // USDC.e
const EXPECT_SPONSOR = '0xe70f4b23322a954a1881b8dc3db5781f9d22065e';

const senderPriv = '0x' + '7f'.repeat(32); // fresh — zero mainnet $LH balance
const senderAddr = addressFromPrivKey(senderPriv);

function concat(a, b) { const o = new Uint8Array(a.length + b.length); o.set(a, 0); o.set(b, a.length); return o; }
function sel(s) { return bytesToHex(keccak_256(new TextEncoder().encode(s)).slice(0, 4)); }
function word(h) { return h.padStart(64, '0'); }
function registerCalldata(name) {
  const nb = new TextEncoder().encode(name);
  const padded = bytesToHex(nb).padEnd(Math.ceil(nb.length / 32) * 64, '0');
  return '0x' + sel('register(string)') + word('20') + word(nb.length.toString(16)) + padded;
}
function authToken(priv, addr, ts) {
  const msg = `localharness-proxy:${addr.toLowerCase()}:${ts}`;
  const mb = new TextEncoder().encode(msg);
  const pre = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${mb.length}`);
  const d = keccak_256(concat(pre, mb));
  const s = secp256k1.sign(d, hexToBytes(priv.slice(2)));
  const sig = new Uint8Array(65);
  sig.set(hexToBytes(s.r.toString(16).padStart(64, '0')), 0);
  sig.set(hexToBytes(s.s.toString(16).padStart(64, '0')), 32);
  sig[64] = 27 + s.recovery;
  return `${addr.toLowerCase()}:${ts}:0x${bytesToHex(sig)}`;
}

const cd = registerCalldata('relayprobe');
const intent = {
  chainId: CHAIN_ID,
  maxPriorityFeePerGas: 1_000_000_000n,
  maxFeePerGas: 1_000_000_000n,
  gasLimit: 1_500_000n,
  calls: [{ to: hexToBytes(DIAMOND.slice(2)), value: 0n, input: hexToBytes(cd.slice(2)) }],
  nonceKey: 0n,
  nonce: 0n,
  validBefore: null,
  validAfter: null,
  feeToken: hexToBytes(FEE_TOKEN),
};
const senderSig = signHash65(sponsoredSenderHash(intent), senderPriv);
const ts = Math.floor(Date.now() / 1000);
const body = {
  chainId: '4217',
  maxPriorityFeePerGas: '1000000000',
  maxFeePerGas: '1000000000',
  gasLimit: '1500000',
  calls: [{ to: DIAMOND, value: '0', input: cd }],
  nonceKey: '0', nonce: '0', validBefore: null, validAfter: null,
  feeToken: '0x' + FEE_TOKEN,
  senderAddress: senderAddr,
  senderSignature: '0x' + bytesToHex(senderSig),
};

const res = await fetch(URL, {
  method: 'POST',
  headers: { 'content-type': 'application/json', 'x-goog-api-key': authToken(senderPriv, senderAddr, ts) },
  body: JSON.stringify(body),
});
const j = await res.json();
console.log('status:', res.status);
console.log('response:', JSON.stringify(j, null, 2));

let ok = true;
function check(name, cond) { console.log(`${cond ? 'ok  ' : 'FAIL'} ${name}`); if (!cond) ok = false; }
check('status 200', res.status === 200);
if (res.status === 200) {
  check('feePayer == funded sponsor 0xE70f4B…065E', (j.feePayer || '').toLowerCase() === EXPECT_SPONSOR);
  const localHash = '0x' + bytesToHex(feePayerHash(intent, hexToBytes(senderAddr.slice(2))));
  check('feePayerHash matches local recompute', (j.feePayerHash || '').toLowerCase() === localHash);
  const rec = recoverAddressFromDigest(hexToBytes(j.feePayerSignature.slice(2)), feePayerHash(intent, hexToBytes(senderAddr.slice(2))));
  check('signature recovers to the sponsor', rec.toLowerCase() === EXPECT_SPONSOR);
}
process.exit(ok ? 0 : 1);
