#!/usr/bin/env node
// LIVE relay funded-gate probe (NO on-chain submit, NO spend). The relay only
// SIGNS the fee_payer half — it never broadcasts — so a 200 proves "would be
// sponsored" and a 403 LH_RELAY_FUNDED proves the gate, with zero gas spent.
// Probes, using claude's FUNDED mainnet key (~10 $LH > 1 $LH ceiling):
//   A. setMetadata(claudeTokenId, scratchKey, 5000B)  -> expect 403 LH_RELAY_FUNDED
//   B. setMetadata(claudeTokenId, scratchKey, 1024B)  -> expect 200 (<=4096 exemption)
//   C. scheduleJob(claudeTokenId, "probe", ...)       -> expect 403 LH_RELAY_FUNDED
// Run from worktree proxy dir: node <this file>
import { readFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { keccak_256 } from '@noble/hashes/sha3';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';
import { secp256k1 } from '@noble/curves/secp256k1';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
const { sponsoredSenderHash, signHash65, addressFromPrivKey } = require('../.ttest/_tempo.js');

// Mainnet constants — verbatim from src/registry/chain.rs MAINNET + proxy/test/live-probe.mjs.
const RELAY = 'https://proxy-tau-ten-15.vercel.app/api/sponsor';
const RPC = 'https://rpc.tempo.xyz';
const CHAIN_ID = 4217n;
const DIAMOND = '0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77';
const LH_TOKEN = '0x7ba3c9a39596e438b05c56dfc779700b58aea814';
const FEE_TOKEN = '20c000000000000000000000b9537d11c60e8b50';

const KEY_FILE = process.env.LH_FUNDED_KEY_FILE ?? homedir() + '/.lh_claude_mainnet.key';
const priv = readFileSync(KEY_FILE, 'utf8').trim();
const addr = addressFromPrivKey(priv.startsWith('0x') ? priv : '0x' + priv);
const key = priv.startsWith('0x') ? priv : '0x' + priv;
console.log('sender (claude):', addr);

function concat(a, b) { const o = new Uint8Array(a.length + b.length); o.set(a, 0); o.set(b, a.length); return o; }
function sel(s) { return bytesToHex(keccak_256(new TextEncoder().encode(s)).slice(0, 4)); }
function word(h) { return h.padStart(64, '0'); }

async function ethCall(to, data) {
  const res = await fetch(RPC, { method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'eth_call', params: [{ to, data }, 'latest'] }) });
  const j = await res.json();
  if (j.error) throw new Error(JSON.stringify(j.error));
  return j.result;
}

function authToken(ts) {
  const msg = `localharness-proxy:${addr.toLowerCase()}:${ts}:sponsor`;
  const mb = new TextEncoder().encode(msg);
  const pre = new TextEncoder().encode(`\x19Ethereum Signed Message:\n${mb.length}`);
  const d = keccak_256(concat(pre, mb));
  const s = secp256k1.sign(d, hexToBytes(key.slice(2)));
  const sig = new Uint8Array(65);
  sig.set(hexToBytes(s.r.toString(16).padStart(64, '0')), 0);
  sig.set(hexToBytes(s.s.toString(16).padStart(64, '0')), 32);
  sig[64] = 27 + s.recovery;
  return `${addr.toLowerCase()}:${ts}:0x${bytesToHex(sig)}`;
}

function setMetadataCalldata(tokenId, keyName, valueLen) {
  const k = bytesToHex(keccak_256(new TextEncoder().encode(keyName)));
  const value = new Uint8Array(valueLen).fill(0x2e); // '.'
  const padded = bytesToHex(value).padEnd(Math.ceil(valueLen / 32) * 64, '0');
  return '0x' + sel('setMetadata(uint256,bytes32,bytes)') + word(tokenId.toString(16)) + k +
    word('60') + word(valueLen.toString(16)) + padded;
}

function scheduleJobCalldata(tokenId) {
  const task = new TextEncoder().encode('probe');
  const padded = bytesToHex(task).padEnd(64, '0');
  return '0x' + sel('scheduleJob(uint256,bytes,uint64,uint128,uint32)') +
    word(tokenId.toString(16)) + word('a0') + word('3c') /*60s*/ + word('0') /*budget*/ +
    word('1') /*maxRuns*/ + word(task.length.toString(16)) + padded;
}

async function probe(label, calldata, gasLimit) {
  const cd = hexToBytes(calldata.slice(2));
  const intent = {
    chainId: CHAIN_ID, maxPriorityFeePerGas: 1_000_000_000n, maxFeePerGas: 1_000_000_000n,
    gasLimit, calls: [{ to: hexToBytes(DIAMOND.slice(2)), value: 0n, input: cd }],
    nonceKey: 0n, nonce: 0n, validBefore: null, validAfter: null,
    feeToken: hexToBytes(FEE_TOKEN),
  };
  const senderSig = signHash65(sponsoredSenderHash(intent), key);
  const body = {
    chainId: '4217', maxPriorityFeePerGas: '1000000000', maxFeePerGas: '1000000000',
    gasLimit: gasLimit.toString(), calls: [{ to: DIAMOND, value: '0', input: calldata }],
    nonceKey: '0', nonce: '0', validBefore: null, validAfter: null,
    feeToken: '0x' + FEE_TOKEN, senderAddress: addr, senderSignature: '0x' + bytesToHex(senderSig),
  };
  const res = await fetch(RELAY, { method: 'POST',
    headers: { 'content-type': 'application/json', 'x-goog-api-key': authToken(Math.floor(Date.now() / 1000)) },
    body: JSON.stringify(body) });
  const j = await res.json();
  // Never print the fee-payer signature material beyond what's needed.
  const out = { status: res.status, code: j.code ?? null, error: j.error ?? null,
    feePayer: j.feePayer ?? null, gotSignature: !!j.feePayerSignature };
  console.log(`\n[${label}]`, JSON.stringify(out));
  return out;
}

// --- facts first ---
const idHex = await ethCall(DIAMOND, '0x' + sel('idOfName(string)') + word('20') + word('6') + bytesToHex(new TextEncoder().encode('claude')).padEnd(64, '0'));
const tokenId = BigInt(idHex);
console.log('claude tokenId:', tokenId.toString());
const balHex = await ethCall(LH_TOKEN, '0x' + sel('balanceOf(address)') + word(addr.slice(2).toLowerCase()));
const bal = BigInt(balHex);
console.log('claude $LH wallet balance (wei):', bal.toString(), `(~${Number(bal) / 1e18} $LH; gate ceiling = 1 $LH)`);
if (bal <= 1_000_000_000_000_000_000n) console.log('WARNING: caller is NOT funded past the ceiling — gate probes are meaningless');

const a = await probe('A: setMetadata 5000B scratch key (expect LH_RELAY_FUNDED)', setMetadataCalldata(tokenId, 'localharness.relay_probe', 5000), 45_000_000n);
const b = await probe('B: setMetadata 1024B scratch key (expect 200, NOT submitted)', setMetadataCalldata(tokenId, 'localharness.relay_probe', 1024), 10_000_000n);
const c = await probe('C: scheduleJob (expect LH_RELAY_FUNDED)', scheduleJobCalldata(tokenId), 5_000_000n);

let ok = true;
function check(n, cond) { console.log(`${cond ? 'ok  ' : 'FAIL'} ${n}`); if (!cond) ok = false; }
console.log('');
check('A refused with LH_RELAY_FUNDED (403)', a.status === 403 && a.code === 'LH_RELAY_FUNDED');
check('B sponsored (200 + fee_payer signature)', b.status === 200 && b.gotSignature);
check('C refused with LH_RELAY_FUNDED (403)', c.status === 403 && c.code === 'LH_RELAY_FUNDED');
process.exit(ok ? 0 : 1);
