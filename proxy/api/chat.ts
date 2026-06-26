// /api/chat -- OFF-CHAIN open chatroom relay (one append-only message log/room).
//
// A `host::chat` cartridge (e.g. groupchat.localharness.xyz) POSTs a message and
// GETs the rolling log. The room is the cartridge's subdomain. Same GitHub-backed
// off-chain model as /api/signal + /api/jobs (no DB, no WebSocket server: Vercel
// Edge can't hold persistent sockets). A room is one JSON file `chat/<room>.json`
// = { next, messages: [{n, name, text, ts}] } trimmed to the last MAX_MESSAGES.
//
// Auth: POST is personal-sign (anti-spam); the sender's short address IS the name
// (no name-entry UI in a cartridge). GET is open (the room id is the capability).

import { verifyAuthToken, isAllowedOrigin } from './_auth';
import { SlidingWindow, claimedAddress } from './_ratelimit';

export const config = { runtime: 'edge' };

const CHAT_REPO = process.env.GH_JOBS_REPO ?? 'compusophy/localharness-jobs';
const GH_TOKEN = process.env.GH_JOBS_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '';
const CHAT_DIR = 'chat';
const MAX_MESSAGES = 80; // rolling window kept per room
const MAX_TEXT = 280;
const ROOM_RE = /^[a-z0-9-]{1,63}$/;
// Per-sender flood cap (best-effort, per-isolate — see api/_ratelimit.ts). The
// GET poll path is open + uncapped; this guards only the GitHub-store WRITE rate
// so chat spam from throwaway wallets can't exhaust the shared token.
const CHAT_PER_MIN = 30;
const senderWindow = new SlidingWindow(CHAT_PER_MIN, 60_000);

interface Msg { n: number; name: string; text: string; ts: number }
interface Log { next: number; messages: Msg[] }

function cors(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) h['Access-Control-Allow-Origin'] = origin;
  return h;
}
function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json', ...cors(origin) } });
}

function ghHeaders(): Record<string, string> {
  return { authorization: `Bearer ${GH_TOKEN}`, accept: 'application/vnd.github+json', 'content-type': 'application/json', 'user-agent': 'localharness-chat' };
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
function logPath(room: string): string {
  return `${CHAT_DIR}/${room}.json`;
}
async function ghRead(room: string): Promise<{ log: Log; sha: string } | null> {
  const res = await fetch(`https://api.github.com/repos/${CHAT_REPO}/contents/${logPath(room)}?ref=main`, { headers: ghHeaders() });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`read: ${res.status}`);
  const j = (await res.json()) as { content?: string; sha?: string };
  if (!j.content || !j.sha) return null;
  try {
    return { log: JSON.parse(b64decodeUtf8(j.content)) as Log, sha: j.sha };
  } catch {
    return null;
  }
}
async function ghWrite(room: string, log: Log, sha?: string): Promise<boolean> {
  const res = await fetch(`https://api.github.com/repos/${CHAT_REPO}/contents/${logPath(room)}`, {
    method: 'PUT',
    headers: ghHeaders(),
    body: JSON.stringify({ message: `chat ${room} (${log.messages.length})`, content: b64encodeUtf8(JSON.stringify(log)), branch: 'main', ...(sha ? { sha } : {}) }),
  });
  return res.ok; // 409/422 = a concurrent writer won; caller retries
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors(origin) });
  if (!GH_TOKEN) return json({ error: 'chat not configured (no GitHub token)' }, 503, origin);

  // GET ?room=&after= : poll the log for messages with n > after. Open.
  if (req.method === 'GET') {
    const u = new URL(req.url);
    const room = (u.searchParams.get('room') ?? '').trim().toLowerCase();
    const after = Number(u.searchParams.get('after') ?? '0') | 0;
    if (!ROOM_RE.test(room)) return json({ error: 'bad room' }, 400, origin);
    let cur: { log: Log; sha: string } | null;
    try {
      cur = await ghRead(room);
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    if (!cur) return json({ messages: [], next: 0 }, 200, origin);
    return json({ messages: cur.log.messages.filter((m) => m.n > after), next: cur.log.next }, 200, origin);
  }

  if (req.method !== 'POST') return json({ error: 'GET or POST' }, 405, origin);

  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
  // Rate limit per CLAIMED address BEFORE the signature check — a flood must not
  // cost a curve recovery per request, and this caps the GitHub-store write rate
  // so chat spam can't exhaust the shared token (see api/_ratelimit.ts; the
  // window gates nothing of value, the debit/auth happen downstream).
  const claimed = claimedAddress(token);
  if (claimed) {
    const wait = senderWindow.hit(claimed);
    if (wait > 0) {
      return json(
        { error: `rate limited: at most ${CHAT_PER_MIN} messages per 60s`, retryAfterSeconds: wait },
        429,
        origin,
      );
    }
  }
  const now = Math.floor(Date.now() / 1000);
  const auth = verifyAuthToken(token, now);
  if (!auth.ok) return json({ error: 'auth: ' + auth.error }, auth.status, origin);
  // The sender's short address is the display name (no name-entry in a cartridge).
  const name = auth.address.slice(2, 6).toLowerCase();

  let payload: Record<string, unknown>;
  try {
    payload = await req.json();
  } catch {
    return json({ error: 'bad json' }, 400, origin);
  }
  const room = String(payload.room ?? '').trim().toLowerCase();
  if (!ROOM_RE.test(room)) return json({ error: 'bad room' }, 400, origin);
  // Collapse all whitespace (newlines/tabs) so a message is one clean line; cap.
  const text = String(payload.text ?? '').split(/\s+/).join(' ').trim().slice(0, MAX_TEXT);
  if (!text) return json({ error: 'empty message' }, 400, origin);

  // Append with a one-retry on a concurrent-write conflict (read-modify-write).
  for (let attempt = 0; attempt < 2; attempt++) {
    let cur: { log: Log; sha: string } | null;
    try {
      cur = await ghRead(room);
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    const log: Log = cur?.log ?? { next: 0, messages: [] };
    const msg: Msg = { n: log.next, name, text, ts: now };
    log.next += 1;
    log.messages.push(msg);
    if (log.messages.length > MAX_MESSAGES) log.messages = log.messages.slice(-MAX_MESSAGES);
    let ok: boolean;
    try {
      ok = await ghWrite(room, log, cur?.sha);
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    if (ok) return json({ posted: true, n: msg.n }, 200, origin);
    // conflict: re-read + retry once
  }
  return json({ error: 'chat busy, try again' }, 409, origin);
}
