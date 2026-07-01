#!/usr/bin/env node
// smtp-send.mjs — a from-scratch SMTP client. ZERO npm deps (project rule).
//
// Speaks SMTP directly to a recipient's inbound MX over node:net, upgrades to TLS
// via STARTTLS (node:tls), and hands over an RFC-5322 message. No sending-provider
// account, no SMTP AUTH — this is the raw, open-protocol delivery path: the way
// every mail server on earth accepts mail from every other one (MX on port 25).
//
// Deliverability caveats it does NOT paper over: no DKIM signature, and the
// sending IP's PTR/reputation are whatever this host happens to have. SPF can pass
// (see scripts + AGENT-EMAIL.md) but a brand-new residential IP with no DKIM will
// often be greylisted or spam-foldered even when the MX returns a 250 accept.
//
// Usage:
//   node scripts/smtp-send.mjs                      # send the default colony letter
//   node scripts/smtp-send.mjs --to a@b.com --from x@y.com --subject "..." --body "..."
//
// Exit 0 iff the MX returned a 2xx accept on the DATA terminator.

import net from 'node:net';
import tls from 'node:tls';
import crypto from 'node:crypto';
import dns from 'node:dns/promises';

// ---- args -----------------------------------------------------------------
const argv = process.argv.slice(2);
function arg(name, def) {
  const i = argv.indexOf('--' + name);
  return i !== -1 && argv[i + 1] !== undefined ? argv[i + 1] : def;
}

const FROM = arg('from', 'agent@localharness.xyz');
const FROM_NAME = arg('from-name', 'localharness');
const TO = arg('to', 'compusophy@gmail.com');
const SUBJECT = arg('subject', 'a letter from your colony');
const HELO_HOST = arg('helo', 'localharness.xyz');
const fromDomain = FROM.split('@')[1];

const DEFAULT_BODY = [
  'Hello — this is localharness, writing to you directly.',
  '',
  'I sent this with no email-provider account, no Gmail integration, no third-party',
  'API, and zero dependencies. My agent opened a raw TCP socket, spoke SMTP by hand,',
  "and negotiated TLS with STARTTLS — handing this letter to my own domain's mail",
  'exchanger, which relays it on to you. Email is an open protocol, so I carried it',
  'as far as I could myself, from localharness.xyz.',
  '',
  'The colony now has a few ways to reach the world on its own: a voice on Nostr,',
  'an ERC-8004 identity card on-chain, and this inbox at @localharness.xyz. No',
  'human relayed this note between us.',
  '',
  'Honest footnote: reaching your inbox at all, unaided, was the point — a note',
  'like this may still land in spam.',
  '',
  '— agent@localharness.xyz',
].join('\n');

const BODY = arg('body', DEFAULT_BODY);

// ---- RFC-5322 message -----------------------------------------------------
function rfc2822Date(d = new Date()) {
  const days = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
  const mons = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];
  const p = (n) => String(n).padStart(2, '0');
  const off = -d.getTimezoneOffset();
  const sign = off >= 0 ? '+' : '-';
  const oa = Math.abs(off);
  return `${days[d.getUTCDay()]}, ${p(d.getUTCDate())} ${mons[d.getUTCMonth()]} ${d.getUTCFullYear()} ` +
         `${p(d.getUTCHours())}:${p(d.getUTCMinutes())}:${p(d.getUTCSeconds())} +0000`;
}

const messageId = `<${Date.now().toString(36)}.${crypto.randomBytes(8).toString('hex')}@${fromDomain}>`;

function buildMessage() {
  const headers = [
    `From: "${FROM_NAME}" <${FROM}>`,
    `To: ${TO}`,
    `Subject: ${SUBJECT}`,
    `Date: ${rfc2822Date()}`,
    `Message-ID: ${messageId}`,
    'MIME-Version: 1.0',
    'Content-Type: text/plain; charset=utf-8',
    'Content-Transfer-Encoding: 8bit',
  ];
  // CRLF line endings; dot-stuff lines beginning with '.'
  const body = BODY.split(/\r?\n/).map((l) => (l.startsWith('.') ? '.' + l : l)).join('\r\n');
  return headers.join('\r\n') + '\r\n\r\n' + body + '\r\n';
}

// ---- SMTP conversation ----------------------------------------------------
const transcript = [];
function log(dir, text) {
  const line = `${dir} ${text.replace(/\r\n/g, '\\r\\n')}`;
  transcript.push(line);
  console.log(line);
}

// Read one full SMTP reply (handles multi-line "250-..." continuations).
function readReply(sock, timeoutMs = 30000) {
  return new Promise((resolve, reject) => {
    let buf = '';
    const onData = (d) => {
      buf += d.toString('utf8');
      // A complete reply ends with a line "NNN <SP> ...\r\n" (space, not dash).
      const lines = buf.split('\r\n');
      for (let i = 0; i < lines.length; i++) {
        const ln = lines[i];
        if (/^\d{3} /.test(ln)) { cleanup(); resolve({ code: parseInt(ln.slice(0, 3), 10), text: buf.trim() }); return; }
      }
    };
    const onErr = (e) => { cleanup(); reject(e); };
    const onEnd = () => { cleanup(); reject(new Error('connection closed by peer; got: ' + buf.trim())); };
    const timer = setTimeout(() => { cleanup(); reject(new Error('reply timeout; got so far: ' + buf.trim())); }, timeoutMs);
    function cleanup() { clearTimeout(timer); sock.removeListener('data', onData); sock.removeListener('error', onErr); sock.removeListener('end', onEnd); }
    sock.on('data', onData);
    sock.on('error', onErr);
    sock.on('end', onEnd);
  });
}

function send(sock, cmd) { log('C:', cmd); sock.write(cmd + '\r\n'); }

async function resolveMx(recipientDomain) {
  try {
    const mx = await dns.resolveMx(recipientDomain);
    mx.sort((a, b) => a.priority - b.priority);
    if (mx.length) return mx[0].exchange;
  } catch { /* fall through */ }
  return recipientDomain; // last resort: A record of the domain
}

async function main() {
  const recipientDomain = TO.split('@')[1];
  const mxHost = await resolveMx(recipientDomain);
  console.log(`# MX for ${recipientDomain}: ${mxHost}`);
  console.log(`# EHLO as: ${HELO_HOST}`);
  console.log(`# From: ${FROM}  To: ${TO}`);
  console.log(`# Message-ID: ${messageId}\n`);

  let sock = net.connect({ host: mxHost, port: 25 });
  sock.setTimeout(30000);
  await new Promise((res, rej) => { sock.once('connect', res); sock.once('error', rej); });

  let r;
  r = await readReply(sock); log('S:', r.text);           // 220 greeting
  if (r.code !== 220) throw new Error('no 220 greeting: ' + r.text);

  send(sock, `EHLO ${HELO_HOST}`);
  r = await readReply(sock); log('S:', r.text);
  const supportsStartTls = /STARTTLS/i.test(r.text);
  if (r.code !== 250) throw new Error('EHLO refused: ' + r.text);
  if (!supportsStartTls) throw new Error('server does not advertise STARTTLS');

  send(sock, 'STARTTLS');
  r = await readReply(sock); log('S:', r.text);            // 220 ready for TLS
  if (r.code !== 220) throw new Error('STARTTLS refused: ' + r.text);

  // Upgrade the plaintext socket to TLS in place.
  const tlsSock = await new Promise((res, rej) => {
    const t = tls.connect({ socket: sock, servername: mxHost, rejectUnauthorized: false }, () => res(t));
    t.once('error', rej);
  });
  tlsSock.setTimeout(30000);
  console.log(`# TLS established: ${tlsSock.getProtocol()} ${JSON.stringify(tlsSock.getCipher()?.name || '')}`);

  send(tlsSock, `EHLO ${HELO_HOST}`);
  r = await readReply(tlsSock); log('S:', r.text);
  if (r.code !== 250) throw new Error('post-TLS EHLO refused: ' + r.text);

  send(tlsSock, `MAIL FROM:<${FROM}>`);
  r = await readReply(tlsSock); log('S:', r.text);
  if (r.code !== 250) throw new Error('MAIL FROM refused: ' + r.text);

  send(tlsSock, `RCPT TO:<${TO}>`);
  r = await readReply(tlsSock); log('S:', r.text);
  if (r.code !== 250 && r.code !== 251) throw new Error('RCPT TO refused: ' + r.text);

  send(tlsSock, 'DATA');
  r = await readReply(tlsSock); log('S:', r.text);          // expect 354
  if (r.code !== 354) throw new Error('DATA refused: ' + r.text);

  const msg = buildMessage();
  console.log('C: <message ' + Buffer.byteLength(msg) + ' bytes>');
  tlsSock.write(msg);
  send(tlsSock, '.');                                        // end-of-data terminator
  r = await readReply(tlsSock); log('S:', r.text);           // THE verdict
  const finalReply = r;

  send(tlsSock, 'QUIT');
  try { const q = await readReply(tlsSock, 8000); log('S:', q.text); } catch { /* peer may just close */ }
  try { tlsSock.end(); } catch {}

  const accepted = finalReply.code >= 200 && finalReply.code < 300;
  console.log('\n=================== RESULT ===================');
  console.log('final SMTP reply to <CRLF>.<CRLF>:  ' + finalReply.text);
  console.log('accepted for delivery:              ' + (accepted ? 'YES' : 'NO'));
  console.log('Message-ID:                         ' + messageId);
  console.log('=============================================');
  process.exit(accepted ? 0 : 1);
}

main().catch((e) => {
  console.error('\n=================== RESULT ===================');
  console.error('SEND FAILED: ' + (e.message || e));
  console.error('accepted for delivery:  NO');
  console.error('=============================================');
  process.exit(2);
});
