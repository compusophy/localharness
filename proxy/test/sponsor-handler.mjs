#!/usr/bin/env node
// Handler-level integration test for api/sponsor.ts (the rate-capped relay).
// Drives the real exported handler with stubbed RPC (balanceOf) over the global
// fetch, proving: a valid onboarding intent gets a fee_payer signature that
// recovers to the sponsor address over the SAME fee_payer hash the CLI would
// recompute; and the three caps reject (funded caller, bad selector, sender
// mismatch). Run after the tsc step in run-tempo-test.sh.

import { secp256k1 } from '@noble/curves/secp256k1';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import sponsorMod from '../.ttest/sponsor.js';
const handler = sponsorMod.default ?? sponsorMod;
import {
  feePayerHash,
  sponsoredSenderHash,
  signHash65,
  recoverAddressFromDigest,
  addressFromPrivKey,
} from '../.ttest/_tempo.js';

const DIAMOND = '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c';
const TOKEN = '0x90B84c7234Aae89BadA7f69160B9901B9bc37B17';
const FEE_TOKEN = '20c0000000000000000000000000000000000001';
const SPONSOR_ADDR = addressFromPrivKey(
  '0x046a830b5203d1d2c0a205a1432746e4381d0874711b2de7f575a973644b9d43',
).toLowerCase();

const senderPriv = '0x' + '11'.repeat(32);
const senderAddr = addressFromPrivKey(senderPriv); // lowercase 0x

let failed = false;
function ok(name, cond, detail = '') {
  if (cond) console.log(`ok   ${name}`);
  else {
    console.error(`FAIL ${name} ${detail}`);
    failed = true;
  }
}

function concat(a, b) {
  const o = new Uint8Array(a.length + b.length);
  o.set(a, 0);
  o.set(b, a.length);
  return o;
}
function sel(s) {
  return bytesToHex(keccak_256(new TextEncoder().encode(s)).slice(0, 4));
}
function word(hexNoPrefix) {
  return hexNoPrefix.padStart(64, '0');
}
/** register(string) calldata. */
function registerCalldata(name) {
  const nameBytes = new TextEncoder().encode(name);
  const padded = bytesToHex(nameBytes).padEnd(Math.ceil(nameBytes.length / 32) * 64, '0');
  return '0x' + sel('register(string)') + word('20') + word(nameBytes.length.toString(16)) + padded;
}
/** Personal-sign token over `localharness-proxy:<addr>:<ts>`. */
function authToken(priv, addr, ts) {
  const message = `localharness-proxy:${addr.toLowerCase()}:${ts}`;
  const msgBytes = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${msgBytes.length}`);
  const digest = keccak_256(concat(prefix, msgBytes));
  const s = secp256k1.sign(digest, hexToBytes(priv.slice(2)));
  const sig = new Uint8Array(65);
  sig.set(hexToBytes(s.r.toString(16).padStart(64, '0')), 0);
  sig.set(hexToBytes(s.s.toString(16).padStart(64, '0')), 32);
  sig[64] = 27 + s.recovery;
  return `${addr.toLowerCase()}:${ts}:0x${bytesToHex(sig)}`;
}

function makeIntent(toAddr, calldataHex) {
  return {
    chainId: 42431n,
    maxPriorityFeePerGas: 1_000_000_000n,
    maxFeePerGas: 1_000_000_000n,
    gasLimit: 1_500_000n,
    calls: [{ to: hexToBytes(toAddr.slice(2)), value: 0n, input: hexToBytes(calldataHex.slice(2)) }],
    nonceKey: 0n,
    nonce: 3n,
    validBefore: null,
    validAfter: null,
    feeToken: hexToBytes(FEE_TOKEN),
  };
}

function bodyFor(intent, calldataHex, toAddr) {
  const senderSig = signHash65(sponsoredSenderHash(intent), senderPriv);
  return {
    chainId: '42431',
    maxPriorityFeePerGas: '1000000000',
    maxFeePerGas: '1000000000',
    gasLimit: '1500000',
    calls: [{ to: toAddr, value: '0', input: calldataHex }],
    nonceKey: '0',
    nonce: '3',
    validBefore: null,
    validAfter: null,
    feeToken: '0x' + FEE_TOKEN,
    senderAddress: senderAddr,
    senderSignature: '0x' + bytesToHex(senderSig),
  };
}

function makeReq(body, token) {
  return new Request('http://localhost/api/sponsor', {
    method: 'POST',
    headers: { 'content-type': 'application/json', origin: 'http://localhost', 'x-goog-api-key': token },
    body: JSON.stringify(body),
  });
}

// Stub fetch → eth_call balanceOf returns `balanceHex`.
function stubBalance(balanceHex) {
  globalThis.fetch = async () =>
    new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, result: '0x' + balanceHex.padStart(64, '0') }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
}

const ts = Math.floor(Date.now() / 1000);
const token = authToken(senderPriv, senderAddr, ts);

// --- happy path: unfunded caller, register on the diamond -------------------
{
  stubBalance('0');
  const cd = registerCalldata('relaytest');
  const intent = makeIntent(DIAMOND, cd);
  const res = await handler(makeReq(bodyFor(intent, cd, DIAMOND), token));
  const j = await res.json();
  ok('happy: 200', res.status === 200, `status=${res.status} body=${JSON.stringify(j)}`);
  if (res.status === 200) {
    const localHash = bytesToHex(feePayerHash(intent, hexToBytes(senderAddr.slice(2))));
    ok('happy: feePayerHash matches CLI recompute', j.feePayerHash === '0x' + localHash);
    const recovered = recoverAddressFromDigest(hexToBytes(j.feePayerSignature.slice(2)), feePayerHash(intent, hexToBytes(senderAddr.slice(2))));
    ok('happy: signature recovers to sponsor', recovered.toLowerCase() === SPONSOR_ADDR, `recovered=${recovered} sponsor=${SPONSOR_ADDR}`);
    ok('happy: feePayer field == sponsor', j.feePayer.toLowerCase() === SPONSOR_ADDR);
  }
}

// --- funded caller is refused (onboarding-only) -----------------------------
{
  stubBalance('1bc16d674ec80000'); // 2 $LH > 1 $LH ceiling
  const cd = registerCalldata('relaytest');
  const intent = makeIntent(DIAMOND, cd);
  const res = await handler(makeReq(bodyFor(intent, cd, DIAMOND), token));
  const j = await res.json();
  ok('funded caller refused 403 LH_RELAY_FUNDED', res.status === 403 && j.code === 'LH_RELAY_FUNDED', `status=${res.status} code=${j.code}`);
}

// --- non-allowlisted selector (transfer on the token) is refused ------------
{
  stubBalance('0');
  const cd = '0x' + sel('transfer(address,uint256)') + word('00'.repeat(20)) + word('1');
  const intent = makeIntent(TOKEN, cd);
  const res = await handler(makeReq(bodyFor(intent, cd, TOKEN), token));
  const j = await res.json();
  ok('token transfer refused 403 LH_RELAY_SELECTOR', res.status === 403 && j.code === 'LH_RELAY_SELECTOR', `status=${res.status} code=${j.code}`);
}

// --- approve to a non-diamond spender is refused ----------------------------
{
  stubBalance('0');
  const evilSpender = 'ab'.repeat(20);
  const cd = '0x' + sel('approve(address,uint256)') + word(evilSpender) + word('ff');
  const intent = makeIntent(TOKEN, cd);
  const res = await handler(makeReq(bodyFor(intent, cd, TOKEN), token));
  const j = await res.json();
  ok('approve non-diamond spender refused', res.status === 403 && j.code === 'LH_RELAY_SELECTOR', `status=${res.status} code=${j.code}`);
}

// --- approve(diamond, amount) on the token IS allowed -----------------------
{
  stubBalance('0');
  const cd = '0x' + sel('approve(address,uint256)') + word(DIAMOND.slice(2).toLowerCase()) + word('64');
  const intent = makeIntent(TOKEN, cd);
  const res = await handler(makeReq(bodyFor(intent, cd, TOKEN), token));
  ok('approve(diamond,..) allowed 200', res.status === 200, `status=${res.status}`);
}

// --- sender mismatch (token signed by a different key) is refused -----------
{
  stubBalance('0');
  const cd = registerCalldata('relaytest');
  const intent = makeIntent(DIAMOND, cd);
  const otherTs = Math.floor(Date.now() / 1000);
  const otherPriv = '0x' + '22'.repeat(32);
  const otherAddr = addressFromPrivKey(otherPriv);
  const otherToken = authToken(otherPriv, otherAddr, otherTs);
  // body still claims senderAddress = senderAddr (mismatch vs the auth caller)
  const res = await handler(makeReq(bodyFor(intent, cd, DIAMOND), otherToken));
  const j = await res.json();
  ok('sender mismatch refused 403 LH_RELAY_SENDER', res.status === 403 && j.code === 'LH_RELAY_SENDER', `status=${res.status} code=${j.code}`);
}

// --- chain mismatch is refused ----------------------------------------------
{
  stubBalance('0');
  const cd = registerCalldata('relaytest');
  const intent = makeIntent(DIAMOND, cd);
  const body = bodyFor(intent, cd, DIAMOND);
  body.chainId = '4217'; // mainnet — relay is on testnet
  // re-sign sender over the mutated intent so it's the chain check that trips
  const mIntent = { ...intent, chainId: 4217n };
  body.senderSignature = '0x' + bytesToHex(signHash65(sponsoredSenderHash(mIntent), senderPriv));
  const res = await handler(makeReq(body, token));
  const j = await res.json();
  ok('chain mismatch refused 400 LH_RELAY_CHAIN', res.status === 400 && j.code === 'LH_RELAY_CHAIN', `status=${res.status} code=${j.code}`);
}

if (failed) {
  console.error('\nsponsor handler test FAILED');
  process.exit(1);
}
console.log('\nall sponsor-handler cases pass');
