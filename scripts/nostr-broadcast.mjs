#!/usr/bin/env node
// nostr-broadcast.mjs — self-sovereign Nostr broadcaster for localharness.
//
// ZERO npm deps (project rule). Uses only Node built-ins: crypto, tls, fs.
// Node 20 has no global WebSocket, so this ships a minimal RFC-6455 client over
// node:tls, plus a from-scratch BIP-340 Schnorr / secp256k1 signer and bech32
// (NIP-19) codec. NIP-01 kind-1 events; verifies acceptance with a read-back REQ.
//
// Commands:
//   node scripts/nostr-broadcast.mjs gen                 # create identity -> .nostr_identity
//   node scripts/nostr-broadcast.mjs keys                # print npub/pubkey for saved identity
//   node scripts/nostr-broadcast.mjs post "<text>"       # sign + publish kind-1 + verify
//   node scripts/nostr-broadcast.mjs fetch <event-id>    # re-fetch an event from relays
//
// The nsec lives in .nostr_identity (gitignored) and is NEVER printed in full by
// `post`/`keys` unless you pass `--show-nsec`.

import crypto from 'node:crypto';
import tls from 'node:tls';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { EventEmitter } from 'node:events';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = path.resolve(__dirname, '..');
const IDENTITY_FILE = path.join(REPO_ROOT, '.nostr_identity');

export const DEFAULT_RELAYS = [
  'wss://relay.damus.io',
  'wss://relay.nostr.band',
  'wss://relay.primal.net',
  'wss://nos.lol',
];

// ---------------------------------------------------------------------------
// secp256k1 (affine) + BIP-340 Schnorr — from scratch, BigInt.
// ---------------------------------------------------------------------------
const P  = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2Fn;
const N  = 0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141n;
const Gx = 0x79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798n;
const Gy = 0x483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8n;
const G  = [Gx, Gy];

const mod = (a, m) => ((a % m) + m) % m;

function modinv(a, m) {
  a = mod(a, m);
  let [oldR, r] = [a, m];
  let [oldS, s] = [1n, 0n];
  while (r !== 0n) {
    const q = oldR / r;
    [oldR, r] = [r, oldR - q * r];
    [oldS, s] = [s, oldS - q * s];
  }
  return mod(oldS, m);
}

function modpow(b, e, m) {
  b = mod(b, m);
  let r = 1n;
  while (e > 0n) {
    if (e & 1n) r = (r * b) % m;
    b = (b * b) % m;
    e >>= 1n;
  }
  return r;
}

function ptAdd(Pa, Qa) {
  if (Pa === null) return Qa;
  if (Qa === null) return Pa;
  const [x1, y1] = Pa, [x2, y2] = Qa;
  if (x1 === x2 && mod(y1 + y2, P) === 0n) return null; // P + (-P)
  let m;
  if (x1 === x2 && y1 === y2) {
    m = mod(3n * x1 * x1 * modinv(2n * y1, P), P); // a = 0
  } else {
    m = mod((y2 - y1) * modinv(mod(x2 - x1, P), P), P);
  }
  const x3 = mod(m * m - x1 - x2, P);
  const y3 = mod(m * (x1 - x3) - y1, P);
  return [x3, y3];
}

function ptMul(k, Pa) {
  let R = null, A = Pa;
  k = mod(k, N);
  while (k > 0n) {
    if (k & 1n) R = ptAdd(R, A);
    A = ptAdd(A, A);
    k >>= 1n;
  }
  return R;
}

// lift_x per BIP-340 (P ≡ 3 mod 4 so sqrt via (P+1)/4 exponent).
function liftX(x) {
  if (x <= 0n || x >= P) return null;
  const c = mod(modpow(x, 3n, P) + 7n, P);
  const y = modpow(c, (P + 1n) / 4n, P);
  if (mod(y * y - c, P) !== 0n) return null;
  return [x, (y & 1n) === 0n ? y : P - y];
}

const sha256 = (buf) => crypto.createHash('sha256').update(buf).digest();

function taggedHash(tag, msg) {
  const t = sha256(Buffer.from(tag, 'utf8'));
  return sha256(Buffer.concat([t, t, msg]));
}

const b2big = (buf) => BigInt('0x' + Buffer.from(buf).toString('hex'));
function big2buf32(n) {
  let h = mod(n, 2n ** 256n).toString(16).padStart(64, '0');
  return Buffer.from(h, 'hex');
}

export function getXOnlyPubkey(sk32) {
  const d = b2big(sk32);
  if (d <= 0n || d >= N) throw new Error('private key out of range');
  const Pp = ptMul(d, G);
  return big2buf32(Pp[0]); // x-only, 32 bytes
}

// BIP-340 Schnorr sign over 32-byte message.
function schnorrSign(msg32, sk32, aux32) {
  const d0 = b2big(sk32);
  if (d0 <= 0n || d0 >= N) throw new Error('private key out of range');
  const Pp = ptMul(d0, G);
  const d = (Pp[1] & 1n) === 0n ? d0 : N - d0;
  const pBytes = big2buf32(Pp[0]);
  const tHash = taggedHash('BIP0340/aux', aux32);
  const t = Buffer.alloc(32);
  const dBytes = big2buf32(d);
  for (let i = 0; i < 32; i++) t[i] = dBytes[i] ^ tHash[i];
  const rand = taggedHash('BIP0340/nonce', Buffer.concat([t, pBytes, msg32]));
  const k0 = mod(b2big(rand), N);
  if (k0 === 0n) throw new Error('nonce is zero');
  const R = ptMul(k0, G);
  const k = (R[1] & 1n) === 0n ? k0 : N - k0;
  const rBytes = big2buf32(R[0]);
  const e = mod(b2big(taggedHash('BIP0340/challenge', Buffer.concat([rBytes, pBytes, msg32]))), N);
  const sig = Buffer.concat([rBytes, big2buf32(mod(k + e * d, N))]);
  return { sig, pubkey: pBytes };
}

// BIP-340 verify — self-check so we never publish a bad sig.
function schnorrVerify(msg32, pubkey32, sig64) {
  const Px = b2big(pubkey32);
  const Pp = liftX(Px);
  if (!Pp) return false;
  const r = b2big(sig64.subarray(0, 32));
  const s = b2big(sig64.subarray(32, 64));
  if (r >= P || s >= N) return false;
  const e = mod(b2big(taggedHash('BIP0340/challenge',
    Buffer.concat([sig64.subarray(0, 32), pubkey32, msg32]))), N);
  const R = ptAdd(ptMul(s, G), ptMul(N - e, Pp));
  if (R === null) return false;
  if ((R[1] & 1n) !== 0n) return false; // has_even_y(R)
  return R[0] === r;
}

// ---------------------------------------------------------------------------
// bech32 (NIP-19) — npub / nsec
// ---------------------------------------------------------------------------
const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3];

function polymod(values) {
  let chk = 1;
  for (const v of values) {
    const b = chk >> 25;
    chk = ((chk & 0x1ffffff) << 5) ^ v;
    for (let i = 0; i < 5; i++) if ((b >> i) & 1) chk ^= GEN[i];
  }
  return chk >>> 0;
}
function hrpExpand(hrp) {
  const out = [];
  for (let i = 0; i < hrp.length; i++) out.push(hrp.charCodeAt(i) >> 5);
  out.push(0);
  for (let i = 0; i < hrp.length; i++) out.push(hrp.charCodeAt(i) & 31);
  return out;
}
function createChecksum(hrp, data) {
  const values = hrpExpand(hrp).concat(data, [0, 0, 0, 0, 0, 0]);
  const m = polymod(values) ^ 1;
  const out = [];
  for (let i = 0; i < 6; i++) out.push((m >> (5 * (5 - i))) & 31);
  return out;
}
function convertBits(data, from, to, pad) {
  let acc = 0, bits = 0; const out = []; const maxv = (1 << to) - 1;
  for (const value of data) {
    acc = (acc << from) | value; bits += from;
    while (bits >= to) { bits -= to; out.push((acc >> bits) & maxv); }
  }
  if (pad && bits > 0) out.push((acc << (to - bits)) & maxv);
  return out;
}
export function bech32Encode(hrp, bytes) {
  const data = convertBits([...bytes], 8, 5, true);
  const combined = data.concat(createChecksum(hrp, data));
  let s = hrp + '1';
  for (const d of combined) s += CHARSET[d];
  return s;
}
export function bech32Decode(str) {
  const lower = str.toLowerCase();
  const pos = lower.lastIndexOf('1');
  const hrp = lower.slice(0, pos);
  const data = [];
  for (const c of lower.slice(pos + 1)) {
    const d = CHARSET.indexOf(c);
    if (d === -1) throw new Error('bad bech32 char');
    data.push(d);
  }
  if (polymod(hrpExpand(hrp).concat(data)) !== 1) throw new Error('bad bech32 checksum');
  const payload = convertBits(data.slice(0, -6), 5, 8, false);
  return { hrp, bytes: Buffer.from(payload) };
}

// ---------------------------------------------------------------------------
// NIP-01 event
// ---------------------------------------------------------------------------
export function buildEvent(sk32, content, kind = 1, tags = []) {
  const pubkey = getXOnlyPubkey(sk32).toString('hex');
  const created_at = Math.floor(Date.now() / 1000);
  const serial = JSON.stringify([0, pubkey, created_at, kind, tags, content]);
  const id = sha256(Buffer.from(serial, 'utf8')).toString('hex');
  const idBytes = Buffer.from(id, 'hex');
  const { sig } = schnorrSign(idBytes, sk32, crypto.randomBytes(32));
  const ev = { id, pubkey, created_at, kind, tags, content, sig: sig.toString('hex') };
  if (!schnorrVerify(idBytes, Buffer.from(pubkey, 'hex'), sig)) {
    throw new Error('self-verify FAILED — refusing to publish');
  }
  return ev;
}

// ---------------------------------------------------------------------------
// Minimal RFC-6455 WebSocket client over node:tls (no deps; Node 20 has no WS).
// ---------------------------------------------------------------------------
const WS_GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

export class WSClient extends EventEmitter {
  constructor(socket) {
    super();
    this.socket = socket;
    this.buf = Buffer.alloc(0);
    this._frag = [];
    this._fragOp = 0;
    this.closed = false;
  }
  feed(chunk) {
    this.buf = Buffer.concat([this.buf, chunk]);
    this._parse();
  }
  _parse() {
    while (this.buf.length >= 2) {
      const b0 = this.buf[0], b1 = this.buf[1];
      const fin = (b0 & 0x80) !== 0;
      const opcode = b0 & 0x0f;
      const masked = (b1 & 0x80) !== 0;
      let len = b1 & 0x7f;
      let off = 2;
      if (len === 126) { if (this.buf.length < 4) return; len = this.buf.readUInt16BE(2); off = 4; }
      else if (len === 127) { if (this.buf.length < 10) return; len = Number(this.buf.readBigUInt64BE(2)); off = 10; }
      let maskKey = null;
      if (masked) { if (this.buf.length < off + 4) return; maskKey = this.buf.subarray(off, off + 4); off += 4; }
      if (this.buf.length < off + len) return;
      let payload = Buffer.from(this.buf.subarray(off, off + len));
      if (masked) for (let i = 0; i < payload.length; i++) payload[i] ^= maskKey[i % 4];
      this.buf = this.buf.subarray(off + len);
      this._frame(fin, opcode, payload);
    }
  }
  _frame(fin, opcode, payload) {
    if (opcode === 0x0 || opcode === 0x1 || opcode === 0x2) {
      if (opcode !== 0x0) { this._fragOp = opcode; this._frag = []; }
      this._frag.push(payload);
      if (fin) {
        const full = Buffer.concat(this._frag);
        this._frag = [];
        if (this._fragOp === 0x1) this.emit('message', full.toString('utf8'));
        else this.emit('binary', full);
      }
    } else if (opcode === 0x8) { this.emit('close'); this._destroy(); }
    else if (opcode === 0x9) { this._send(0xA, payload); }   // ping -> pong
    // 0xA pong: ignore
  }
  send(str) { this._send(0x1, Buffer.from(str, 'utf8')); }
  _send(opcode, payload) {
    if (this.closed) return;
    const mask = crypto.randomBytes(4);
    const len = payload.length;
    let header;
    if (len < 126) { header = Buffer.alloc(2); header[1] = 0x80 | len; }
    else if (len < 65536) { header = Buffer.alloc(4); header[1] = 0x80 | 126; header.writeUInt16BE(len, 2); }
    else { header = Buffer.alloc(10); header[1] = 0x80 | 127; header.writeBigUInt64BE(BigInt(len), 2); }
    header[0] = 0x80 | opcode;
    const out = Buffer.from(payload);
    for (let i = 0; i < out.length; i++) out[i] ^= mask[i % 4];
    try { this.socket.write(Buffer.concat([header, mask, out])); } catch { /* closed */ }
  }
  close() { if (!this.closed) { this._send(0x8, Buffer.alloc(0)); } this._destroy(); }
  _destroy() { if (this.closed) return; this.closed = true; try { this.socket.end(); } catch {} }
}

export function wsConnect(url, timeout = 12000) {
  return new Promise((resolve, reject) => {
    const u = new URL(url);
    const host = u.hostname;
    const port = u.port ? Number(u.port) : (u.protocol === 'wss:' ? 443 : 80);
    const reqPath = (u.pathname || '/') + (u.search || '');
    const key = crypto.randomBytes(16).toString('base64');
    const accept = crypto.createHash('sha1').update(key + WS_GUID).digest('base64');
    const socket = tls.connect({ host, port, servername: host }, () => {
      socket.write([
        `GET ${reqPath} HTTP/1.1`,
        `Host: ${host}`,
        'Upgrade: websocket',
        'Connection: Upgrade',
        `Sec-WebSocket-Key: ${key}`,
        'Sec-WebSocket-Version: 13',
        'Origin: https://localharness.xyz',
        'User-Agent: localharness-nostr/1.0',
        '', '',
      ].join('\r\n'));
    });
    let settled = false;
    const ws = new WSClient(socket);
    let handshakeDone = false;
    let pre = Buffer.alloc(0);
    const tmr = setTimeout(() => {
      if (!settled) { settled = true; socket.destroy(); reject(new Error('connect timeout')); }
    }, timeout);
    socket.on('data', (chunk) => {
      if (handshakeDone) { ws.feed(chunk); return; }
      pre = Buffer.concat([pre, chunk]);
      const idx = pre.indexOf('\r\n\r\n');
      if (idx === -1) return;
      const head = pre.slice(0, idx).toString('utf8');
      const statusOk = /^HTTP\/1\.1 101/.test(head);
      if (!statusOk || !head.includes(accept)) {
        if (!settled) { settled = true; clearTimeout(tmr); socket.destroy(); reject(new Error('handshake failed: ' + head.split('\r\n')[0])); }
        return;
      }
      handshakeDone = true;
      clearTimeout(tmr);
      const rest = pre.subarray(idx + 4);
      if (!settled) { settled = true; resolve(ws); }
      if (rest.length) ws.feed(rest);
    });
    socket.on('error', (e) => { if (!settled) { settled = true; clearTimeout(tmr); reject(e); } else ws.emit('error', e); });
    socket.on('close', () => { clearTimeout(tmr); ws.emit('close'); });
    socket.setTimeout(timeout, () => socket.destroy());
  });
}

// Publish an event to one relay and verify acceptance via a read-back REQ.
export async function publishToRelay(url, event, timeout = 12000) {
  const result = { relay: url, connected: false, ok: null, message: '', readback: false };
  let ws;
  try {
    ws = await wsConnect(url, timeout);
  } catch (e) {
    result.message = 'connect: ' + (e.message || e);
    return result;
  }
  result.connected = true;
  return await new Promise((resolve) => {
    const subid = 'lh-verify-' + crypto.randomBytes(4).toString('hex');
    let stage = 'publish';
    let finished = false;
    const finish = () => {
      if (finished) return; finished = true;
      clearTimeout(timer);
      try { ws.close(); } catch {}
      resolve(result);
    };
    const timer = setTimeout(() => { result.message ||= 'timeout@' + stage; finish(); }, timeout);
    ws.on('message', (msg) => {
      let arr; try { arr = JSON.parse(msg); } catch { return; }
      const type = arr[0];
      if (type === 'OK' && arr[1] === event.id) {
        result.ok = !!arr[2];
        result.message = arr[3] || (result.ok ? 'accepted' : 'rejected');
        if (result.ok) { stage = 'verify'; ws.send(JSON.stringify(['REQ', subid, { ids: [event.id] }])); }
        else finish();
      } else if (type === 'EVENT' && arr[1] === subid) {
        if (arr[2] && arr[2].id === event.id) result.readback = true;
      } else if (type === 'EOSE' && arr[1] === subid) {
        finish();
      } else if (type === 'NOTICE') {
        result.notice = arr[1];
      } else if (type === 'CLOSED' && arr[1] === subid) {
        result.message ||= 'REQ closed: ' + (arr[2] || '');
        finish();
      }
    });
    ws.on('error', (e) => { result.message ||= 'error: ' + (e.message || e); finish(); });
    ws.on('close', () => { if (result.ok === null) result.message ||= 'closed before OK'; finish(); });
    ws.send(JSON.stringify(['EVENT', event]));
  });
}

// Fetch an event id back from relays (for the `fetch` command / later verification).
export async function fetchFromRelay(url, eventId, timeout = 10000) {
  const result = { relay: url, connected: false, found: false, message: '' };
  let ws;
  try { ws = await wsConnect(url, timeout); } catch (e) { result.message = 'connect: ' + (e.message || e); return result; }
  result.connected = true;
  return await new Promise((resolve) => {
    const subid = 'lh-fetch-' + crypto.randomBytes(4).toString('hex');
    let finished = false;
    const finish = () => { if (finished) return; finished = true; clearTimeout(timer); try { ws.close(); } catch {} resolve(result); };
    const timer = setTimeout(() => { result.message ||= 'timeout'; finish(); }, timeout);
    ws.on('message', (msg) => {
      let arr; try { arr = JSON.parse(msg); } catch { return; }
      if (arr[0] === 'EVENT' && arr[1] === subid && arr[2] && arr[2].id === eventId) { result.found = true; result.event = arr[2]; }
      else if (arr[0] === 'EOSE' && arr[1] === subid) finish();
      else if (arr[0] === 'CLOSED' && arr[1] === subid) finish();
    });
    ws.on('error', () => finish());
    ws.on('close', () => finish());
    ws.send(JSON.stringify(['REQ', subid, { ids: [eventId] }]));
  });
}

// ---------------------------------------------------------------------------
// identity persistence
// ---------------------------------------------------------------------------
function genIdentity() {
  let sk;
  do { sk = crypto.randomBytes(32); } while (b2big(sk) === 0n || b2big(sk) >= N);
  const pubkey = getXOnlyPubkey(sk).toString('hex');
  const npub = bech32Encode('npub', Buffer.from(pubkey, 'hex'));
  const nsec = bech32Encode('nsec', sk);
  return { sk, privkey: sk.toString('hex'), pubkey, npub, nsec };
}

function saveIdentity(id) {
  const record = {
    npub: id.npub,
    nsec: id.nsec,
    pubkey_hex: id.pubkey,
    privkey_hex: id.privkey,
    created_at: new Date().toISOString(),
    note: 'localharness Nostr broadcast identity. SECRET — never commit. nsec/privkey grant full control.',
  };
  fs.writeFileSync(IDENTITY_FILE, JSON.stringify(record, null, 2) + '\n', { mode: 0o600 });
}

export function loadIdentity() {
  if (!fs.existsSync(IDENTITY_FILE)) {
    throw new Error(`no identity at ${IDENTITY_FILE} — run: node scripts/nostr-broadcast.mjs gen`);
  }
  const rec = JSON.parse(fs.readFileSync(IDENTITY_FILE, 'utf8'));
  // Prefer nsec as source of truth; fall back to privkey hex.
  let sk;
  if (rec.nsec) { const d = bech32Decode(rec.nsec); if (d.hrp !== 'nsec') throw new Error('not an nsec'); sk = d.bytes; }
  else if (rec.privkey_hex) sk = Buffer.from(rec.privkey_hex, 'hex');
  else throw new Error('identity file missing nsec/privkey');
  const pubkey = getXOnlyPubkey(sk).toString('hex');
  return { sk, pubkey, npub: bech32Encode('npub', Buffer.from(pubkey, 'hex')), rec };
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------
async function main() {
  const [, , cmd, ...rest] = process.argv;
  const showNsec = rest.includes('--show-nsec');
  const args = rest.filter((a) => a !== '--show-nsec');

  if (cmd === 'gen') {
    if (fs.existsSync(IDENTITY_FILE)) {
      const cur = loadIdentity();
      console.error('Identity already exists; refusing to overwrite.');
      console.log('npub:  ' + cur.npub);
      console.log('file:  ' + IDENTITY_FILE);
      process.exit(2);
    }
    const id = genIdentity();
    saveIdentity(id);
    console.log('Generated Nostr identity -> ' + IDENTITY_FILE + ' (chmod 600, gitignored)');
    console.log('npub:    ' + id.npub);
    console.log('pubkey:  ' + id.pubkey);
    if (showNsec) console.log('nsec:    ' + id.nsec);
    else console.log('nsec:    (hidden — pass --show-nsec to reveal; stored in ' + path.basename(IDENTITY_FILE) + ')');
    return;
  }

  if (cmd === 'keys') {
    const id = loadIdentity();
    console.log('npub:    ' + id.npub);
    console.log('pubkey:  ' + id.pubkey);
    if (showNsec) console.log('nsec:    ' + id.rec.nsec);
    return;
  }

  if (cmd === 'post') {
    const content = args[0];
    if (!content) { console.error('usage: post "<text>"'); process.exit(1); }
    const id = loadIdentity();
    const event = buildEvent(id.sk, content, 1, []);
    console.log('npub:       ' + id.npub);
    console.log('event id:   ' + event.id);
    console.log('content:    ' + JSON.stringify(content));
    console.log('self-verify: PASS (BIP-340 Schnorr)');
    console.log('publishing to ' + DEFAULT_RELAYS.length + ' relays...\n');
    const results = await Promise.all(DEFAULT_RELAYS.map((r) => publishToRelay(r, event)));
    let accepted = 0, readback = 0;
    for (const r of results) {
      const verdict = r.ok === true ? 'ACCEPTED' : r.ok === false ? 'REJECTED' : 'NO-OK';
      const rb = r.readback ? ' | read-back OK' : '';
      console.log(`  ${r.relay.padEnd(26)} ${verdict.padEnd(9)} ${r.message || ''}${rb}`);
      if (r.ok === true) accepted++;
      if (r.readback) readback++;
    }
    console.log('');
    console.log(`accepted by ${accepted}/${DEFAULT_RELAYS.length} relays; read-back confirmed on ${readback}.`);
    console.log('view: https://njump.me/' + event.id);
    console.log('view: https://primal.net/e/' + event.id);
    // machine-readable tail for the doc/automation
    console.log('\nJSON ' + JSON.stringify({ id: event.id, npub: id.npub, accepted, readback,
      relays: results.map((r) => ({ relay: r.relay, ok: r.ok, readback: r.readback, message: r.message })) }));
    process.exit(accepted > 0 ? 0 : 3);
  }

  if (cmd === 'fetch') {
    const eid = args[0];
    if (!eid) { console.error('usage: fetch <event-id-hex>'); process.exit(1); }
    const results = await Promise.all(DEFAULT_RELAYS.map((r) => fetchFromRelay(r, eid)));
    for (const r of results) console.log(`  ${r.relay.padEnd(26)} ${r.found ? 'FOUND' : 'not found'} ${r.message || ''}`);
    const any = results.some((r) => r.found);
    console.log('\n' + (any ? 'event is live on the network.' : 'event not found on these relays right now.'));
    process.exit(any ? 0 : 3);
  }

  if (cmd === 'selftest') {
    // Official BIP-340 test vectors (index 0 and 1) — proves Schnorr + pubkey.
    const vectors = [
      {
        sk: '0000000000000000000000000000000000000000000000000000000000000003',
        pk: 'F9308A019258C31049344F85F89D5229B531C845836F99B08601F113BCE036F9',
        aux: '0000000000000000000000000000000000000000000000000000000000000000',
        msg: '0000000000000000000000000000000000000000000000000000000000000000',
        sig: 'E907831F80848D1069A5371B402410364BDF1C5F8307B0084C55F1CE2DCA821525F66A4A85EA8B71E482A74F382D2CE5EBEEE8FDB2172F477DF4900D310536C0',
      },
      {
        sk: 'B7E151628AED2A6ABF7158809CF4F3C762E7160F38B4DA56A784D9045190CFEF',
        pk: 'DFF1D77F2A671C5F36183726DB2341BE58FEAE1DA2DECED843240F7B502BA659',
        aux: '0000000000000000000000000000000000000000000000000000000000000001',
        msg: '243F6A8885A308D313198A2E03707344A4093822299F31D0082EFA98EC4E6C89',
        sig: '6896BD60EEAE296DB48A229FF71DFE071BDE413E6D43F917DC8DCF8C78DE33418906D11AC976ABCCB20B091292BFF4EA897EFCB639EA871CFA95F6DE339E4B0A',
      },
    ];
    let pass = 0;
    for (let i = 0; i < vectors.length; i++) {
      const v = vectors[i];
      const sk = Buffer.from(v.sk, 'hex');
      const pk = getXOnlyPubkey(sk).toString('hex').toUpperCase();
      const msg = Buffer.from(v.msg, 'hex');
      const { sig } = schnorrSign(msg, sk, Buffer.from(v.aux, 'hex'));
      const sigHex = sig.toString('hex').toUpperCase();
      const verifyOk = schnorrVerify(msg, Buffer.from(v.pk, 'hex'), sig);
      const pkOk = pk === v.pk;
      const sigOk = sigHex === v.sig;
      console.log(`vector ${i}: pubkey ${pkOk ? 'OK' : 'FAIL'} | sig ${sigOk ? 'OK' : 'FAIL'} | verify ${verifyOk ? 'OK' : 'FAIL'}`);
      if (!pkOk) console.log(`  expected pk ${v.pk}\n  got      pk ${pk}`);
      if (!sigOk) console.log(`  expected sig ${v.sig}\n  got      sig ${sigHex}`);
      if (pkOk && sigOk && verifyOk) pass++;
    }
    // random sign->verify round-trips (10x)
    let rtPass = 0;
    for (let i = 0; i < 10; i++) {
      let sk; do { sk = crypto.randomBytes(32); } while (b2big(sk) === 0n || b2big(sk) >= N);
      const m = crypto.randomBytes(32);
      const { sig, pubkey } = schnorrSign(m, sk, crypto.randomBytes(32));
      if (schnorrVerify(m, pubkey, sig)) rtPass++;
    }
    console.log(`sign/verify round-trips: ${rtPass}/10 OK`);
    // bech32 round-trip
    const probe = crypto.randomBytes(32);
    const np = bech32Encode('npub', probe);
    const rt = bech32Decode(np).bytes.toString('hex') === probe.toString('hex');
    console.log('bech32 round-trip: ' + (rt ? 'OK' : 'FAIL'));
    const allOk = pass === vectors.length && rt && rtPass === 10;
    console.log(allOk ? '\nALL SELFTESTS PASS' : '\nSELFTEST FAILURE');
    process.exit(allOk ? 0 : 1);
  }

  console.error('commands: gen | keys | post "<text>" | fetch <event-id> | selftest   [--show-nsec]');
  process.exit(1);
}

// Only run the CLI when invoked directly (`node nostr-broadcast.mjs ...`), not when
// imported as a module (e.g. by nostr-seti.mjs, which reuses the sign/WS primitives).
const INVOKED_DIRECTLY =
  process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (INVOKED_DIRECTLY) {
  main().catch((e) => { console.error('fatal:', e.message || e); process.exit(1); });
}
