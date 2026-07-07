#!/usr/bin/env node
// Parity gate: api/_tempo.ts must reproduce the live-proven Tempo 0x76 wire
// hashes pinned in src/tempo_tx.rs (GOLDEN_FEE_PAYER_HASH /
// GOLDEN_SPONSORED_SENDER_HASH). If this fails, the relay would sign the wrong
// fee_payer commitment and every sponsored tx would be rejected (or worse, sign
// a tx the caller didn't authorize). Run: node test/tempo-feepayer.mjs
//
// Imports the COMPILED _tempo (see the tsc step in run-tempo-test.sh).

import {
  feePayerHash,
  sponsoredSenderHash,
  addressFromPrivKey,
  bytesToHex,
  hexToBytes,
} from '../.ttest/_tempo.js';

const GOLDEN_FEE_PAYER_HASH =
  'a6e9b8ae237b8711335dad82bdcb3cda9b52278f4a479392bbc153e888a4b5b5';
const GOLDEN_SPONSORED_SENDER_HASH =
  '3e6d7f767fb15c062735b045126a54e9ea8f4d098cebe942cb18761532242d17';
// CREATE (empty-to) shape — src/tempo_tx.rs::golden_create_tx (telemetry #45).
const GOLDEN_CREATE_SENDER_HASH =
  '8ab02d6dcd60133884d552c6d653009c557598b09f1e2f9b9efc74719931448d';
const GOLDEN_CREATE_FEE_PAYER_HASH =
  '3ad4d733941509176da33faf5b2acd14e990cd778f15087f5ac0fca075122105';

// --- golden_tx fixture (mirror of src/tempo_tx.rs::golden_tx) ----------------
const senderPriv = '0x' + '00'.repeat(31) + '01';
const senderAddr = hexToBytes(addressFromPrivKey(senderPriv).slice(2));

const input = new Uint8Array(68);
input.set([0xa9, 0x05, 0x9c, 0xbb], 0);
for (let i = 0; i < 64; i++) input[4 + i] = i;

const tx = {
  chainId: 42431n,
  maxPriorityFeePerGas: 1_000_000_000n,
  maxFeePerGas: 2_000_000_000n,
  gasLimit: 1_500_000n,
  calls: [{ to: new Uint8Array(20).fill(0xd7), value: 0n, input }],
  nonceKey: 0n,
  nonce: 7n,
  validBefore: null,
  validAfter: null,
  feeToken: hexToBytes('20c0000000000000000000000000000000000001'),
};

let failed = false;
function check(name, got, want) {
  if (got !== want) {
    console.error(`FAIL ${name}\n  got:  ${got}\n  want: ${want}`);
    failed = true;
  } else {
    console.log(`ok   ${name}`);
  }
}

check('fee_payer_hash', bytesToHex(feePayerHash(tx, senderAddr)), GOLDEN_FEE_PAYER_HASH);
check('sponsored_sender_hash', bytesToHex(sponsoredSenderHash(tx)), GOLDEN_SPONSORED_SENDER_HASH);

// --- golden_create_tx fixture (mirror of src/tempo_tx.rs::golden_create_tx) --
// Sponsored CREATE: `to` must encode EMPTY (0x80) — the fixture's non-zero
// [0xd7;20] `to` pins that it is IGNORED. Init-code = solc preamble + 0..63.
const initCode = new Uint8Array(68);
initCode.set([0x60, 0x80, 0x60, 0x40, 0x52], 0);
for (let i = 0; i < 63; i++) initCode[5 + i] = i;

const createTx = {
  chainId: 42431n,
  maxPriorityFeePerGas: 1_000_000_000n,
  maxFeePerGas: 2_000_000_000n,
  gasLimit: 25_000_000n,
  calls: [{ to: new Uint8Array(20).fill(0xd7), value: 0n, input: initCode }],
  nonceKey: 0n,
  nonce: 3n,
  validBefore: null,
  validAfter: null,
  feeToken: hexToBytes('20c0000000000000000000000000000000000001'),
  create: true,
};

check('create_sender_hash', bytesToHex(sponsoredSenderHash(createTx)), GOLDEN_CREATE_SENDER_HASH);
check('create_fee_payer_hash', bytesToHex(feePayerHash(createTx, senderAddr)), GOLDEN_CREATE_FEE_PAYER_HASH);

if (failed) {
  console.error('\nTempo wire parity BROKEN — _tempo.ts diverged from tempo_tx.rs');
  process.exit(1);
}
console.log('\nall tempo wire hashes match the Rust golden vectors');
