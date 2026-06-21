#!/usr/bin/env node
// Tests for the Tempo MPP USDC.e -> $LH on-ramp lego (_mpp.ts) + the
// mpp-onramp.ts handler. Proves:
//   * peg parity: 1 USDC.e (6dp) -> 100 $LH (1e18 wei); round-trip quote.
//   * MPP 402 challenge round-trips (WWW-Authenticate header parse + base64url
//     request payload); credential parse from Authorization: Payment.
//   * on-chain verify: rejects wrong-treasury / no-transfer; accepts a correct
//     USDC.e Transfer to the treasury and derives $LH at parity from the
//     on-chain amount (NOT client input).
//   * the handler emits a 402 challenge with no credential and a 200 +
//     Payment-Receipt on an already-minted (idempotent) settlement.
// Run after the tsc step in test/run.sh.

import { keccak_256 } from '@noble/hashes/sha3';
import { bytesToHex, hexToBytes } from '@noble/hashes/utils';
import { secp256k1 } from '@noble/curves/secp256k1';

import * as mpp from '../.ttest/_mpp.js';
import onrampMod from '../.ttest/mpp-onramp.js';
const handler = onrampMod.default ?? onrampMod;

const TREASURY = '0x1111111111111111111111111111111111111111';
const USDCE = '0x20c000000000000000000000b9537d11c60e8b50';
const ISSUER_PRIV = '0x' + '07'.repeat(32);
const SUBMITTER_PRIV = '0x' + '08'.repeat(32);

// _mpp/_stripe read these at call time (process.env), so set before any call.
process.env.ONRAMP_TREASURY = TREASURY;
process.env.ONRAMP_USDCE = USDCE;
process.env.ONRAMP_RPC = 'http://stub.local';
process.env.ONRAMP_REGISTRY = '0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77';
process.env.ONRAMP_CHAIN_ID = '4217';
process.env.FIAT_ISSUER_KEY = ISSUER_PRIV;
process.env.ONRAMP_SUBMITTER_KEY = SUBMITTER_PRIV;
process.env.ONRAMP_MIN_CONFIRMATIONS = '1';

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

// JSON.stringify chokes on BigInt; stringify any verify/mint result safely.
function show(v) {
  return JSON.stringify(v, (_k, val) => (typeof val === 'bigint' ? val.toString() : val));
}

function addr(priv) {
  const pub = secp256k1.getPublicKey(hexToBytes(priv.slice(2)), false);
  return '0x' + bytesToHex(keccak_256(pub.slice(1)).slice(12));
}

/** Personal-sign proxy auth token over `localharness-proxy:<addr>:<ts>`. */
function authToken(priv, ts) {
  const a = addr(priv).toLowerCase();
  const message = `localharness-proxy:${a}:${ts}`;
  const msgBytes = new TextEncoder().encode(message);
  const prefix = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${msgBytes.length}`);
  const digest = keccak_256(concat(prefix, msgBytes));
  const s = secp256k1.sign(digest, hexToBytes(priv.slice(2)));
  const sig = new Uint8Array(65);
  sig.set(hexToBytes(s.r.toString(16).padStart(64, '0')), 0);
  sig.set(hexToBytes(s.s.toString(16).padStart(64, '0')), 32);
  sig[64] = 27 + s.recovery;
  return `${a}:${ts}:0x${bytesToHex(sig)}`;
}

const word = (h) => h.replace(/^0x/, '').toLowerCase().padStart(64, '0');
const TRANSFER_TOPIC = '0x' + bytesToHex(keccak_256(new TextEncoder().encode('Transfer(address,address,uint256)')));

// --- peg parity --------------------------------------------------------------
{
  const oneUsdce = 1_000_000n; // 1 USDC.e at 6 decimals
  const lhWei = mpp.usdceUnitsToLhWei(oneUsdce, 6);
  ok('1 USDC.e -> 100 $LH wei', lhWei === 100n * 10n ** 18n, `got ${lhWei}`);
  ok('LH_WEI_PER_USDCE constant', mpp.LH_WEI_PER_USDCE === 100n * 10n ** 18n);

  // Quote round-trip: want 250 $LH -> 2.5 USDC.e -> back to >= 250 $LH.
  const wantLhWei = 250n * 10n ** 18n;
  const units = mpp.lhWeiToUsdceUnits(wantLhWei, 6);
  ok('quote 250 $LH -> 2.5 USDC.e units', units === 2_500_000n, `got ${units}`);
  ok('quote round-trips (no underpay)', mpp.usdceUnitsToLhWei(units, 6) >= wantLhWei);
}

// --- MPP challenge header round-trip ----------------------------------------
{
  const ch = mpp.buildChallenge({ usdceUnits: 1_000_000n, resource: 'https://x/mpp/onramp' });
  const header = mpp.challengeHeader(ch);
  ok('challenge header names tempo/charge', /method="tempo"/.test(header) && /intent="charge"/.test(header));
  const reqMatch = /request="([^"]+)"/.exec(header);
  ok('challenge header carries request payload', !!reqMatch);
  if (reqMatch) {
    const b64 = reqMatch[1].replace(/-/g, '+').replace(/_/g, '/');
    const pad = b64.length % 4 ? '='.repeat(4 - (b64.length % 4)) : '';
    const req = JSON.parse(Buffer.from(b64 + pad, 'base64').toString('utf8'));
    ok('request payTo == treasury', req.payTo.toLowerCase() === TREASURY.toLowerCase());
    ok('request asset == USDC.e', req.asset.toLowerCase() === USDCE.toLowerCase());
    ok('request maxAmountRequired == 1 USDC.e units', req.maxAmountRequired === '1000000');
    ok('request scheme/network', req.scheme === 'mpp' && req.network === 'tempo');
  }
  const body = mpp.challengeBody(ch);
  ok('challenge body is RFC9457 problem', body.status === 402 && Array.isArray(body.accepts));
}

// --- credential parse --------------------------------------------------------
{
  const tx = '0x' + 'ab'.repeat(32);
  const payTo = '0x2222222222222222222222222222222222222222';
  const payload = Buffer.from(JSON.stringify({ settlementTx: tx, payTo })).toString('base64url');
  const cred = mpp.parseCredential(`Payment payload="${payload}"`);
  ok('credential parses settlementTx + payTo', cred && cred.settlementTx === tx && cred.payTo === payTo.toLowerCase());
  ok('no Payment scheme -> null', mpp.parseCredential('Bearer xyz') === null);
  let threw = false;
  try {
    mpp.parseCredential('Payment payload="' + Buffer.from('{"settlementTx":"0xbad"}').toString('base64url') + '"');
  } catch {
    threw = true;
  }
  ok('malformed credential throws', threw);
}

// --- receiptId determinism ---------------------------------------------------
{
  const tx = '0x' + 'cd'.repeat(32);
  ok('receiptIdForTx is deterministic', mpp.receiptIdForTx(tx) === mpp.receiptIdForTx(tx.toUpperCase()));
}

// --- on-chain verify: stub the RPC ------------------------------------------
// One Transfer log of `amountHex` USDC.e to `toAddr`, mined at block 10, head 20.
function stubRpc({ toAddr, amountHex, status = '0x1', txAddress = USDCE, receiptUsed = false }) {
  globalThis.fetch = async (_url, opts) => {
    const { method, params } = JSON.parse(opts.body);
    let result;
    if (method === 'eth_getTransactionReceipt') {
      result = {
        status,
        from: '0x3333333333333333333333333333333333333333',
        blockNumber: '0xa',
        logs: [
          {
            address: txAddress,
            topics: [
              TRANSFER_TOPIC,
              '0x' + word('3333333333333333333333333333333333333333'),
              '0x' + word(toAddr.replace(/^0x/, '')),
            ],
            data: '0x' + amountHex.padStart(64, '0'),
          },
        ],
      };
    } else if (method === 'eth_blockNumber') {
      result = '0x14'; // 20
    } else if (method === 'eth_call') {
      const to = (params[0].to ?? '').toLowerCase();
      const data = (params[0].data ?? '').toLowerCase();
      // decimals() -> 6; receiptInfo(...) -> used flag for idempotency reads.
      if (to === USDCE.toLowerCase()) result = '0x' + (6).toString(16).padStart(64, '0');
      else {
        // MintGateFacet.receiptInfo: (to, amount, used, clawed, clawedWei)
        result =
          '0x' +
          word('0') +
          word('64') + // amount = 100
          word(receiptUsed ? '1' : '0') +
          word('0') +
          word('0');
        void data;
      }
    } else {
      result = '0x';
    }
    return new Response(JSON.stringify({ jsonrpc: '2.0', id: 1, result }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  };
}

{
  // correct: 5 USDC.e -> 500 $LH, derived from the on-chain amount.
  stubRpc({ toAddr: TREASURY, amountHex: (5_000_000n).toString(16) });
  const v = await mpp.verifySettlement('0x' + 'ee'.repeat(32));
  ok('verify accepts correct treasury transfer', v.ok, show(v));
  ok('verify derives $LH from on-chain amount (500 $LH)', v.ok && v.lhWei === 500n * 10n ** 18n, v.ok ? `${v.lhWei}` : '');
}
{
  // wrong recipient -> no creditable transfer.
  stubRpc({ toAddr: '0x9999999999999999999999999999999999999999', amountHex: (5_000_000n).toString(16) });
  const v = await mpp.verifySettlement('0x' + 'ef'.repeat(32));
  ok('verify rejects transfer to a non-treasury', !v.ok && v.status === 402, show(v));
}
{
  // reverted tx.
  stubRpc({ toAddr: TREASURY, amountHex: (5_000_000n).toString(16), status: '0x0' });
  const v = await mpp.verifySettlement('0x' + 'f0'.repeat(32));
  ok('verify rejects reverted settlement', !v.ok, show(v));
}
{
  // a Transfer log on a DIFFERENT token (not USDC.e) is ignored.
  stubRpc({ toAddr: TREASURY, amountHex: (5_000_000n).toString(16), txAddress: '0xdeaddeaddeaddeaddeaddeaddeaddeaddeaddead' });
  const v = await mpp.verifySettlement('0x' + 'f1'.repeat(32));
  ok('verify ignores a non-USDC.e token transfer', !v.ok, show(v));
}

// --- mintFromSettlement idempotency (already-used receipt -> no submit) ------
{
  stubRpc({ toAddr: TREASURY, amountHex: (5_000_000n).toString(16), receiptUsed: true });
  const out = await mpp.mintFromSettlement('0x' + 'f2'.repeat(32), '0x4444444444444444444444444444444444444444');
  ok('mint is idempotent on a used receipt (no resubmit)', out.minted && out.idempotent === true, show(out));
}

// --- handler: no credential -> 402 challenge --------------------------------
{
  const ts = Math.floor(Date.now() / 1000);
  const token = authToken('0x' + '0a'.repeat(32), ts);
  const req = new Request('http://localhost/mpp/onramp', {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-goog-api-key': token, origin: 'http://localhost' },
    body: JSON.stringify({ lh_amount: '100' }),
  });
  const res = await handler(req);
  const www = res.headers.get('WWW-Authenticate') ?? '';
  ok('handler 402 with challenge', res.status === 402 && /method="tempo"/.test(www), `status=${res.status} www=${www}`);
}

// --- handler: with credential on a used receipt -> 200 + Payment-Receipt -----
{
  stubRpc({ toAddr: TREASURY, amountHex: (1_000_000n).toString(16), receiptUsed: true });
  const ts = Math.floor(Date.now() / 1000);
  const token = authToken('0x' + '0b'.repeat(32), ts);
  const tx = '0x' + 'c0'.repeat(32);
  const payload = Buffer.from(JSON.stringify({ settlementTx: tx, payTo: addr('0x' + '0b'.repeat(32)) })).toString('base64url');
  const req = new Request('http://localhost/mpp/onramp', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-goog-api-key': token,
      authorization: `Payment payload="${payload}"`,
      origin: 'http://localhost',
    },
    body: JSON.stringify({ lh_amount: '100' }),
  });
  const res = await handler(req);
  const receipt = res.headers.get('Payment-Receipt') ?? '';
  const j = await res.json();
  ok('handler 200 + Payment-Receipt on mint', res.status === 200 && j.minted === true && /id=/.test(receipt), `status=${res.status} receipt=${receipt} body=${JSON.stringify(j)}`);
}

// --- handler: bad auth -> 401 -----------------------------------------------
{
  const req = new Request('http://localhost/mpp/onramp', {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-goog-api-key': 'not-a-token', origin: 'http://localhost' },
    body: '{}',
  });
  const res = await handler(req);
  ok('handler 401 on bad auth', res.status === 401, `status=${res.status}`);
}

if (failed) {
  console.error('\nmpp-onramp tests FAILED');
  process.exit(1);
}
console.log('\nall mpp-onramp cases pass');
