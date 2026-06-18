#!/usr/bin/env node
// One-off: read ALL on-chain feedback from the FeedbackFacet via the view
// functions (feedbackCount + feedbackRange) — no eth_getLogs 100k-block window,
// so it sees every entry regardless of age. Queries both the testnet (Moderato)
// and mainnet diamonds. Pure-JS keccak256 (self-verified) for selectors; raw
// JSON-RPC via global fetch (node 18+). Read-only.

// ---- keccak-256 (Ethereum variant: 0x01 .. 0x80 padding) -------------------
const MASK = (1n << 64n) - 1n;
const RC = [
  0x0000000000000001n, 0x0000000000008082n, 0x800000000000808an, 0x8000000080008000n,
  0x000000000000808bn, 0x0000000080000001n, 0x8000000080008081n, 0x8000000000008009n,
  0x000000000000008an, 0x0000000000000088n, 0x0000000080008009n, 0x000000008000000an,
  0x000000008000808bn, 0x800000000000008bn, 0x8000000000008089n, 0x8000000000008003n,
  0x8000000000008002n, 0x8000000000000080n, 0x000000000000800an, 0x800000008000000an,
  0x8000000080008081n, 0x8000000000008080n, 0x0000000080000001n, 0x8000000080008008n,
];
const ROT = [
  [0, 36, 3, 41, 18],
  [1, 44, 10, 45, 2],
  [62, 6, 43, 15, 61],
  [28, 55, 25, 21, 56],
  [27, 20, 39, 8, 14],
];
const rol = (x, n) => ((x << BigInt(n)) | (x >> BigInt(64 - n))) & MASK;

function keccakF(A) {
  for (let round = 0; round < 24; round++) {
    const C = new Array(5);
    for (let x = 0; x < 5; x++) C[x] = A[x][0] ^ A[x][1] ^ A[x][2] ^ A[x][3] ^ A[x][4];
    const D = new Array(5);
    for (let x = 0; x < 5; x++) D[x] = C[(x + 4) % 5] ^ rol(C[(x + 1) % 5], 1);
    for (let x = 0; x < 5; x++) for (let y = 0; y < 5; y++) A[x][y] ^= D[x];
    const B = [[], [], [], [], []];
    for (let x = 0; x < 5; x++)
      for (let y = 0; y < 5; y++) B[y][(2 * x + 3 * y) % 5] = rol(A[x][y], ROT[x][y]);
    for (let x = 0; x < 5; x++)
      for (let y = 0; y < 5; y++) A[x][y] = B[x][y] ^ (~B[(x + 1) % 5][y] & B[(x + 2) % 5][y] & MASK);
    A[0][0] ^= RC[round];
  }
}

function keccak256(bytes) {
  const rate = 136; // 1088 bits
  const A = Array.from({ length: 5 }, () => Array.from({ length: 5 }, () => 0n));
  // pad10*1 with Keccak domain: append 0x01, then 0x80 on last block byte.
  const padded = new Uint8Array(Math.ceil((bytes.length + 1) / rate) * rate);
  padded.set(bytes);
  padded[bytes.length] ^= 0x01;
  padded[padded.length - 1] ^= 0x80;
  for (let off = 0; off < padded.length; off += rate) {
    for (let i = 0; i < rate / 8; i++) {
      let lane = 0n;
      for (let b = 0; b < 8; b++) lane |= BigInt(padded[off + i * 8 + b]) << BigInt(8 * b);
      const x = i % 5,
        y = Math.floor(i / 5);
      A[x][y] ^= lane;
    }
    keccakF(A);
  }
  const out = new Uint8Array(32);
  for (let i = 0; i < 4; i++) {
    const x = i % 5,
      y = Math.floor(i / 5);
    let lane = A[x][y];
    for (let b = 0; b < 8; b++) out[i * 8 + b] = Number((lane >> BigInt(8 * b)) & 0xffn);
  }
  return out;
}

const enc = (s) => new TextEncoder().encode(s);
const hex = (u8) => [...u8].map((b) => b.toString(16).padStart(2, '0')).join('');
const selector = (sig) => hex(keccak256(enc(sig)).slice(0, 4));

// ---- self-verify before trusting any selector ------------------------------
const v1 = hex(keccak256(enc('')));
const v2 = hex(keccak256(enc('abc')));
if (v1 !== 'c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470')
  throw new Error('keccak self-test failed (empty): ' + v1);
if (v2 !== '4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45')
  throw new Error('keccak self-test failed (abc): ' + v2);
console.error('keccak256 self-test OK');

// ---- ABI helpers -----------------------------------------------------------
const SEL_COUNT = selector('feedbackCount()');
const SEL_RANGE = selector('feedbackRange(uint256,uint256)');
const word = (n) => BigInt(n).toString(16).padStart(64, '0');

async function ethCall(rpc, to, data) {
  const res = await fetch(rpc, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method: 'eth_call', params: [{ to, data: '0x' + data }, 'latest'] }),
  });
  const j = await res.json();
  if (j.error) throw new Error(JSON.stringify(j.error));
  return j.result.replace(/^0x/, '');
}

const readWord = (h, off) => BigInt('0x' + h.slice(off * 2, off * 2 + 64));

// decode feedbackRange return: (address[] senders, uint64[] ts, string[] texts)
function decodeRange(h) {
  const offS = Number(readWord(h, 0));
  const offT = Number(readWord(h, 32));
  const offX = Number(readWord(h, 64));
  const nS = Number(readWord(h, offS));
  const senders = [];
  for (let k = 0; k < nS; k++) senders.push('0x' + h.slice((offS + 32 + k * 32) * 2 + 24, (offS + 32 + k * 32) * 2 + 64));
  const nT = Number(readWord(h, offT));
  const tss = [];
  for (let k = 0; k < nT; k++) tss.push(Number(readWord(h, offT + 32 + k * 32)));
  const nX = Number(readWord(h, offX));
  const base = offX + 32; // element offsets are relative to here
  const texts = [];
  for (let k = 0; k < nX; k++) {
    const elemOff = base + Number(readWord(h, base + k * 32));
    const strLen = Number(readWord(h, elemOff));
    const bytes = h.slice((elemOff + 32) * 2, (elemOff + 32) * 2 + strLen * 2);
    texts.push(Buffer.from(bytes, 'hex').toString('utf8'));
  }
  return senders.map((s, k) => ({ sender: s.toLowerCase(), timestamp: tss[k], text: texts[k] }));
}

import { readFileSync, existsSync } from 'node:fs';
// Resolved-index skip set (testnet epoch). First whitespace token of each
// non-comment, non-blank line is the resolved on-chain index.
const RESOLVED = new Set();
const rp = new URL('../docs/feedback-resolved.txt', import.meta.url);
if (existsSync(rp)) {
  for (const line of readFileSync(rp, 'utf8').split('\n')) {
    const t = line.trim();
    if (!t || t.startsWith('#')) continue;
    const first = t.split(/\s+/)[0];
    if (/^\d+$/.test(first)) RESOLVED.add(Number(first));
  }
}
const ONLY_OPEN = process.argv.includes('--open');

const CHAINS = [
  { name: 'TESTNET (Moderato 42431)', rpc: 'https://rpc.moderato.tempo.xyz', diamond: '0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c' },
  { name: 'MAINNET (Tempo 4217)', rpc: 'https://rpc.tempo.xyz', diamond: '0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77' },
];

for (const c of CHAINS) {
  console.log('\n================ ' + c.name + ' ================');
  console.log(c.diamond);
  let count;
  try {
    const r = await ethCall(c.rpc, c.diamond, SEL_COUNT);
    count = Number(BigInt('0x' + r));
  } catch (e) {
    console.log('  feedbackCount() reverted/failed → FeedbackFacet likely NOT cut here: ' + e.message);
    continue;
  }
  console.log('  feedbackCount = ' + count);
  if (count === 0) continue;
  const all = [];
  const PAGE = 40;
  for (let start = 0; start < count; start += PAGE) {
    const data = SEL_RANGE + word(start) + word(Math.min(PAGE, count - start));
    const h = await ethCall(c.rpc, c.diamond, data);
    all.push(...decodeRange(h));
  }
  const isTestnet = c.name.startsWith('TESTNET');
  let open = 0;
  for (let i = 0; i < all.length; i++) {
    const resolved = isTestnet && RESOLVED.has(i);
    if (ONLY_OPEN && resolved) continue;
    const e = all[i];
    const when = new Date(e.timestamp * 1000).toISOString().replace('T', ' ').slice(0, 16);
    const tag = resolved ? '[RESOLVED]' : '[OPEN]';
    if (!resolved) open++;
    console.log(`\n[#${i}] ${tag} ${when}  ${e.sender}`);
    console.log('  ' + e.text.replace(/\n/g, '\n  '));
  }
  console.log(`\n  --- ${c.name}: ${all.length} total, ${open} OPEN (unresolved) ---`);
}
