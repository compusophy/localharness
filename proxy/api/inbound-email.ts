// /api/inbound-email -- OFF-CHAIN inbound-mail webhook + platform-readable inbox.
//
// A mail provider (ForwardEmail webhook, Mailgun/SendGrid Inbound Parse, Postmark,
// or a Cloudflare Email Worker) POSTs a received message here; the platform GETs the
// rolling per-mailbox log. Same GitHub-backed off-chain model as /api/chat +
// /api/signal + /api/jobs (no DB, no SMTP server: Vercel Edge can't run a mail
// daemon). One JSON file `inbox/<mailbox>.json` = { next, messages: [...] } trimmed
// to the last MAX_MESSAGES. `<mailbox>` = the local-part of the @localharness.xyz
// recipient (agent@, claude@, hello@ ...), so each subdomain agent owns its inbox.
//
// This makes the @localharness.xyz inbox SELF-SOVEREIGN: mail lands in the platform's
// own store (read by the browser app / CLI), not a third-party mailbox. It is INERT
// until `INBOUND_EMAIL_SECRET` is set (the shared secret the provider carries in the
// webhook URL `?key=` or an `x-inbound-key` header) — the safe default, like every
// other store-backed route returning 503 when unconfigured.
//
// Auth model:
//   - POST (provider -> us): mail providers can't personal-sign, so the gate is the
//     shared secret (`?key=` or `x-inbound-key`). Set it ONCE and bake it into the
//     provider's webhook URL. Optional HMAC: if `FORWARD_EMAIL_WEBHOOK_KEY` is set we
//     also verify ForwardEmail's `X-Webhook-Signature` (sha256 HMAC of the raw body).
//   - GET (platform -> us): same shared secret (the inbox is private). The browser/
//     CLI reads its mail with the secret; mailbox id is NOT a capability on its own.

import { isAllowedOrigin } from './_auth';

export const config = { runtime: 'edge' };

const INBOX_REPO = process.env.GH_JOBS_REPO ?? 'compusophy/localharness-jobs';
const GH_TOKEN = process.env.GH_JOBS_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '';
const SECRET = process.env.INBOUND_EMAIL_SECRET ?? '';
const HMAC_KEY = process.env.FORWARD_EMAIL_WEBHOOK_KEY ?? '';
const INBOX_DIR = 'inbox';
const MAX_MESSAGES = 100; // rolling window kept per mailbox
const MAX_TEXT = 64 * 1024; // per-field clamp (keep the store file well under GitHub's 1MB)
const MAILBOX_RE = /^[a-z0-9._-]{1,64}$/;
const DOMAIN = (process.env.AGENT_EMAIL_DOMAIN ?? 'localharness.xyz').toLowerCase();

interface Mail {
  n: number;
  ts: number; // unix seconds (received)
  mailbox: string; // local-part the mail was addressed to
  from: string;
  to: string;
  subject: string;
  text: string;
  html: string;
  messageId: string;
}
interface Inbox { next: number; messages: Mail[] }

function cors(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-inbound-key, x-webhook-signature',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) h['Access-Control-Allow-Origin'] = origin;
  return h;
}
function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...cors(origin) } });
}

function ghHeaders(): Record<string, string> {
  return { authorization: `Bearer ${GH_TOKEN}`, accept: 'application/vnd.github+json', 'content-type': 'application/json', 'user-agent': 'localharness-inbox' };
}
function b64encodeUtf8(text: string): string {
  const b = new TextEncoder().encode(text);
  let s = '';
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  return btoa(s);
}
function b64decodeUtf8(b64: string): string {
  const bin = atob(b64.replace(/\n/g, ''));
  const a = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) a[i] = bin.charCodeAt(i);
  return new TextDecoder().decode(a);
}
function inboxPath(mailbox: string): string {
  return `${INBOX_DIR}/${mailbox}.json`;
}
async function ghRead(mailbox: string): Promise<{ inbox: Inbox; sha: string } | null> {
  const res = await fetch(`https://api.github.com/repos/${INBOX_REPO}/contents/${inboxPath(mailbox)}?ref=main`, { headers: ghHeaders() });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`read: ${res.status}`);
  const j = (await res.json()) as { content?: string; sha?: string };
  if (!j.content || !j.sha) return null;
  try {
    return { inbox: JSON.parse(b64decodeUtf8(j.content)) as Inbox, sha: j.sha };
  } catch {
    return null;
  }
}
async function ghWrite(mailbox: string, inbox: Inbox, sha?: string): Promise<boolean> {
  const res = await fetch(`https://api.github.com/repos/${INBOX_REPO}/contents/${inboxPath(mailbox)}`, {
    method: 'PUT',
    headers: ghHeaders(),
    body: JSON.stringify({ message: `inbox ${mailbox} (${inbox.messages.length})`, content: b64encodeUtf8(JSON.stringify(inbox)), branch: 'main', ...(sha ? { sha } : {}) }),
  });
  return res.ok; // 409/422 = a concurrent writer won; caller retries
}

// Constant-time string compare (avoid leaking the secret via timing).
function timingSafeEqual(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}
async function hmacOk(rawBody: string, sigHeader: string): Promise<boolean> {
  if (!HMAC_KEY) return true; // not configured -> skip (the shared secret is the gate)
  if (!sigHeader) return false;
  const key = await crypto.subtle.importKey('raw', new TextEncoder().encode(HMAC_KEY), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const mac = await crypto.subtle.sign('HMAC', key, new TextEncoder().encode(rawBody));
  const hex = Array.from(new Uint8Array(mac)).map((x) => x.toString(16).padStart(2, '0')).join('');
  // Accept either bare hex or a "sha256=" prefix (provider-dependent).
  const got = sigHeader.replace(/^sha256=/i, '').trim().toLowerCase();
  return timingSafeEqual(hex, got);
}

function clamp(s: unknown): string {
  return String(s ?? '').slice(0, MAX_TEXT);
}
// Pull the @DOMAIN recipient's local-part out of a To/recipient string.
function mailboxFromRecipient(...candidates: string[]): string {
  const joined = candidates.filter(Boolean).join(', ');
  const re = new RegExp(`([a-z0-9._%+-]+)@${DOMAIN.replace(/[.]/g, '\\.')}`, 'i');
  const m = joined.match(re);
  const local = (m ? m[1] : 'agent').toLowerCase();
  return MAILBOX_RE.test(local) ? local : 'agent';
}
// A provider's `from`/`to` field may be a string, a mailparser address object
// ({ text, value:[{address,name}] }), or an array. Flatten to a display string.
function addrText(v: unknown): string {
  if (!v) return '';
  if (typeof v === 'string') return v;
  if (Array.isArray(v)) return v.map(addrText).filter(Boolean).join(', ');
  const o = v as Record<string, unknown>;
  if (typeof o.text === 'string') return o.text;
  if (Array.isArray(o.value)) return (o.value as Array<Record<string, unknown>>).map((x) => String(x.address ?? '')).filter(Boolean).join(', ');
  if (typeof o.address === 'string') return o.address;
  return '';
}

// Normalize the wildly different provider payloads (JSON or form) to one Mail shape.
function normalize(body: Record<string, unknown>): Omit<Mail, 'n' | 'ts'> {
  const from = addrText(body.from ?? body.From ?? body.sender ?? body.envelopeFrom);
  const to = addrText(body.to ?? body.To ?? body.recipient ?? body.recipients);
  const subject = clamp(body.subject ?? body.Subject ?? '');
  const text = clamp(body.text ?? body['body-plain'] ?? body.TextBody ?? body.plain ?? '');
  const html = clamp(body.html ?? body['body-html'] ?? body.HtmlBody ?? '');
  const messageId = String(body.messageId ?? body['message-id'] ?? body.MessageID ?? body['Message-Id'] ?? '').slice(0, 256);
  // SendGrid/Mailgun pack the true envelope recipient in `envelope`/`recipient`.
  let envTo = to;
  const env = body.envelope;
  if (typeof env === 'string') { try { const e = JSON.parse(env); if (e && e.to) envTo = addrText(e.to); } catch { /* ignore */ } }
  const mailbox = mailboxFromRecipient(envTo, to, String(body.recipient ?? ''));
  return { mailbox, from, to, subject, text, html, messageId };
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors(origin) });
  if (!GH_TOKEN) return json({ error: 'inbox not configured (no GitHub token)' }, 503, origin);
  if (!SECRET) return json({ error: 'inbox not configured (set INBOUND_EMAIL_SECRET)' }, 503, origin);

  const url = new URL(req.url);
  const presented = url.searchParams.get('key') ?? req.headers.get('x-inbound-key') ?? '';
  if (!timingSafeEqual(presented, SECRET)) return json({ error: 'forbidden' }, 403, origin);

  // GET ?mailbox=&after= : read a mailbox's rolling log (n > after). Secret-gated.
  if (req.method === 'GET') {
    const mailbox = (url.searchParams.get('mailbox') ?? 'agent').trim().toLowerCase();
    const after = Number(url.searchParams.get('after') ?? '0') | 0;
    if (!MAILBOX_RE.test(mailbox)) return json({ error: 'bad mailbox' }, 400, origin);
    let cur: { inbox: Inbox; sha: string } | null;
    try { cur = await ghRead(mailbox); } catch (e) { return json({ error: 'store: ' + (e as Error).message }, 502, origin); }
    if (!cur) return json({ messages: [], next: 0 }, 200, origin);
    return json({ messages: cur.inbox.messages.filter((m) => m.n > after), next: cur.inbox.next }, 200, origin);
  }

  if (req.method !== 'POST') return json({ error: 'GET or POST' }, 405, origin);

  // Parse provider payload: JSON, or multipart/x-www-form-urlencoded (Mailgun/SendGrid).
  const ctype = (req.headers.get('content-type') ?? '').toLowerCase();
  let body: Record<string, unknown>;
  let rawBody = '';
  try {
    if (ctype.includes('multipart/form-data') || ctype.includes('application/x-www-form-urlencoded')) {
      const fd = await req.formData();
      const o: Record<string, unknown> = {};
      for (const [k, v] of fd.entries()) if (typeof v === 'string') o[k] = v; // skip raw attachment blobs
      body = o;
    } else {
      rawBody = await req.text();
      body = rawBody ? (JSON.parse(rawBody) as Record<string, unknown>) : {};
    }
  } catch {
    return json({ error: 'bad payload' }, 400, origin);
  }

  // Optional HMAC (ForwardEmail signature key) over the raw JSON body.
  if (HMAC_KEY) {
    const ok = await hmacOk(rawBody, req.headers.get('x-webhook-signature') ?? '');
    if (!ok) return json({ error: 'bad signature' }, 403, origin);
  }

  const m = normalize(body);
  if (!MAILBOX_RE.test(m.mailbox)) return json({ error: 'bad mailbox' }, 400, origin);
  const now = Math.floor(Date.now() / 1000);

  // Append with a one-retry on a concurrent-write conflict (read-modify-write).
  for (let attempt = 0; attempt < 2; attempt++) {
    let cur: { inbox: Inbox; sha: string } | null;
    try { cur = await ghRead(m.mailbox); } catch (e) { return json({ error: 'store: ' + (e as Error).message }, 502, origin); }
    const inbox: Inbox = cur?.inbox ?? { next: 0, messages: [] };
    const mail: Mail = { n: inbox.next, ts: now, ...m };
    inbox.next += 1;
    inbox.messages.push(mail);
    if (inbox.messages.length > MAX_MESSAGES) inbox.messages = inbox.messages.slice(-MAX_MESSAGES);
    let ok: boolean;
    try { ok = await ghWrite(m.mailbox, inbox, cur?.sha); } catch (e) { return json({ error: 'store: ' + (e as Error).message }, 502, origin); }
    if (ok) return json({ stored: true, mailbox: m.mailbox, n: mail.n }, 200, origin);
    // conflict: re-read + retry once
  }
  return json({ error: 'inbox busy, try again' }, 409, origin);
}
