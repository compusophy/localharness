// /api/signal — OFF-CHAIN WebRTC signaling rendezvous (multiplayer matchmaking).
//
// Two browser peers establish a WebRTC data channel P2P; the only thing they
// can't do without help is the initial SDP exchange (each is behind NAT and
// doesn't know the other's address). This relay is that rendezvous: peer A PUTs
// its offer under a shared `room` id, peer B GETs it, PUTs its answer, A GETs
// that — then all real-time traffic flows P2P over the data channel (this relay
// never sees it). webrtc.rs uses NON-TRICKLE ICE, so the WHOLE SDP (candidates
// included) rides ONE blob per side — a pairing is just offer + answer.
//
// WHY off-chain (not the on-chain SignalingFacet): each on-chain announce/post
// is a sponsored write (~1.2M gas) — fine for low-churn device sync, the exact
// gas pattern we moved the scheduler/apps off-chain to escape for high-churn
// multiplayer. So signaling rides the SAME free GitHub store (direct PATH reads,
// no directory-listing lag), keyed on the room id.
//
// STORE CAVEAT (honest): GitHub commits per blob = git-history churn + a shared
// rate limit — fine to PROVE multiplayer + light use, wrong at scale. The store
// is isolated to the `gh*` helpers here; swap them for a KV (Upstash, TTL) when
// multiplayer volume warrants it. Blobs are short-lived: a `clear` removes a
// room once connected, and reads ignore blobs past SIGNAL_TTL_SECS.

import { verifyAuthToken, isAllowedOrigin } from './_auth';

export const config = { runtime: 'edge' };

const SIGNAL_REPO = process.env.GH_SIGNAL_REPO ?? process.env.GH_JOBS_REPO ?? 'compusophy/localharness-jobs';
const GH_TOKEN = process.env.GH_JOBS_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '';
const SIGNAL_DIR = 'signal';
// A stale offer/answer (peer vanished mid-handshake) must not connect a new
// peer to a dead one — ignore blobs older than this. WebRTC SDP is only useful
// for a fresh handshake anyway.
const SIGNAL_TTL_SECS = 120;
// `room` + `slot` form the path; constrain both so they can't traverse the repo.
const ID_RE = /^[a-zA-Z0-9_-]{1,128}$/;
// Slots: the legacy 2-peer pair (offer/answer), the N-PEER STAR per-joiner SDP
// slots (offer-{id}/answer-{id}), the `join` roster (the host discovers joiners
// here — the store has no directory listing), and the forward-reserved trickle
// candidate slots (cands-*; handlers land with trickle ICE, naming pinned now so
// it never migrates). The slot namespace IS the signaling protocol.
// + `slots` (the MESH membership blob) and directed-pair SDP slots
// `offer-{a}-{b}`/`answer-{a}-{b}` keyed by the two peers' slot indices (mesh:
// the lower index offers, the higher answers — deterministic, no double-dial).
const SLOT_RE = /^(offer|answer|join|slots|(?:offer|answer)-[a-z0-9]{1,32}|(?:offer|answer)-\d{1,2}-\d{1,2}|cands-(?:host|offerer|answerer|joiner-[a-z0-9]{1,32}))$/;
const JOINER_RE = /^[a-z0-9]{1,32}$/;
const ADDR_RE = /^0x[0-9a-fA-F]{40}$/;
const MESH_SLOTS = 8; // fixed mesh capacity (mirrors host::mp MP_PEERS)

function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key',
    Vary: 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) h['Access-Control-Allow-Origin'] = origin;
  return h;
}
function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
  });
}

// --- GitHub Contents API (direct path; no directory listing) -----------------

function ghHeaders(): Record<string, string> {
  return {
    authorization: `Bearer ${GH_TOKEN}`,
    accept: 'application/vnd.github+json',
    'content-type': 'application/json',
    'user-agent': 'localharness-signal',
  };
}
function pathFor(room: string, slot: string): string {
  return `${SIGNAL_DIR}/${room}__${slot}.json`;
}
function b64encodeUtf8(text: string): string {
  const bytes = new TextEncoder().encode(text);
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}
function b64decodeUtf8(b64: string): string {
  const bin = atob(b64.replace(/\n/g, ''));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

// One MESH membership slot: a peer's short id, its FULL address (the
// anti-spoof key — only the address owner may write its slot), and a
// SERVER-STAMPED ts (skew-free freshness; a stale entry is reclaimable).
interface SlotEntry {
  id: string;
  addr: string;
  ts: number;
}
// SDP slots carry {sdp,ts}; the `join` roster carries {joiners,ts}; the mesh
// `slots` blob carries {slots,ts}. One loose shape covers all (ts drives the TTL).
interface Blob {
  sdp?: string;
  joiners?: string[];
  slots?: (SlotEntry | null)[];
  ts: number;
}

async function ghGet(path: string): Promise<{ blob: Blob; sha: string } | null> {
  const res = await fetch(`https://api.github.com/repos/${SIGNAL_REPO}/contents/${path}?ref=main`, {
    headers: ghHeaders(),
  });
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`get ${path}: ${res.status}`);
  const j = (await res.json()) as { content?: string; sha?: string };
  if (!j.content || !j.sha) return null;
  try {
    return { blob: JSON.parse(b64decodeUtf8(j.content)) as Blob, sha: j.sha };
  } catch {
    return null;
  }
}

async function ghPut(path: string, body: string, message: string, sha?: string): Promise<void> {
  const res = await fetch(`https://api.github.com/repos/${SIGNAL_REPO}/contents/${path}`, {
    method: 'PUT',
    headers: ghHeaders(),
    body: JSON.stringify({ message, content: b64encodeUtf8(body), branch: 'main', ...(sha ? { sha } : {}) }),
  });
  if (!res.ok) {
    const d = await res.text();
    throw new Error(`put ${path}: ${res.status} ${d.slice(0, 160)}`);
  }
}

async function ghDelete(path: string): Promise<void> {
  const meta = await fetch(`https://api.github.com/repos/${SIGNAL_REPO}/contents/${path}?ref=main`, {
    headers: ghHeaders(),
  });
  if (meta.status === 404) return; // already gone — idempotent
  const sha = ((await meta.json()) as { sha?: string }).sha;
  if (!sha) return;
  const res = await fetch(`https://api.github.com/repos/${SIGNAL_REPO}/contents/${path}`, {
    method: 'DELETE',
    headers: ghHeaders(),
    body: JSON.stringify({ message: `clear signal ${path}`, sha, branch: 'main' }),
  });
  if (!res.ok && res.status !== 404 && res.status !== 409) {
    throw new Error(`delete ${path}: ${res.status}`);
  }
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: corsHeaders(origin) });
  if (!GH_TOKEN) return json({ error: 'signaling not configured (no GitHub token)' }, 503, origin);

  // GET ?room=&slot= — poll for a peer's blob. OPEN (the room id is the
  // capability; an SDP offer/answer is only useful to a peer who knows the room
  // and completes the handshake). Returns {sdp} or {} (not posted / stale).
  if (req.method === 'GET') {
    const u = new URL(req.url);
    const room = (u.searchParams.get('room') ?? '').trim();
    const slot = (u.searchParams.get('slot') ?? '').trim();
    if (!ID_RE.test(room) || !SLOT_RE.test(slot)) return json({ error: 'bad room/slot' }, 400, origin);
    let hit: { blob: Blob; sha: string } | null;
    try {
      hit = await ghGet(pathFor(room, slot));
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    const now = Math.floor(Date.now() / 1000);
    // Every GET carries the SERVER `now` so the mesh computes entry freshness on
    // server time (skew-free) + the blob `sha` so it can CAS-write the next update.
    if (!hit) return json({ now }, 200, origin);
    // The mesh `slots` blob is NEVER TTL-hidden: per-entry freshness is computed
    // client-side from `now` - entry.ts, so a quiet-but-alive room stays visible.
    if (slot !== 'slots' && now - hit.blob.ts > SIGNAL_TTL_SECS) return json({ now }, 200, origin);
    return json({ ...hit.blob, now, sha: hit.sha }, 200, origin);
  }

  if (req.method !== 'POST') return json({ error: 'GET or POST' }, 405, origin);

  // POST is personal-sign authed (anti-spam: only identities can fill rooms).
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
  const now = Math.floor(Date.now() / 1000);
  const auth = verifyAuthToken(token, now);
  if (!auth.ok) return json({ error: 'auth: ' + auth.error }, auth.status, origin);

  let payload: Record<string, unknown>;
  try {
    payload = await req.json();
  } catch {
    return json({ error: 'bad json' }, 400, origin);
  }

  const room = String(payload.room ?? '').trim();
  if (!ID_RE.test(room)) return json({ error: 'bad room' }, 400, origin);
  const action = String(payload.action ?? 'post').trim();

  // join: append a joiner id to the room's roster (the host polls this single
  // slot to discover joiners, since the store can't list per-joiner slots).
  // Read-modify-write under sha, one retry on a concurrent-join conflict.
  if (action === 'join') {
    const joiner = String(payload.joiner ?? '').trim();
    if (!JOINER_RE.test(joiner)) return json({ error: 'bad joiner id' }, 400, origin);
    const path = pathFor(room, 'join');
    for (let attempt = 0; attempt < 2; attempt++) {
      try {
        const existing = await ghGet(path).catch(() => null);
        const roster = Array.isArray(existing?.blob?.joiners) ? (existing!.blob.joiners as string[]) : [];
        if (roster.includes(joiner)) return json({ joined: true, room, already: true }, 200, origin);
        roster.push(joiner);
        await ghPut(path, JSON.stringify({ joiners: roster, ts: now }), `signal ${room}/join (${roster.length})`, existing?.sha);
        return json({ joined: true, room, count: roster.length }, 200, origin);
      } catch (e) {
        if (attempt === 1) return json({ error: 'store: ' + (e as Error).message }, 502, origin);
        // conflict (concurrent join won the sha) — re-read + retry once
      }
    }
    return json({ error: 'join busy' }, 409, origin);
  }

  // put-slots: write the MESH membership blob under a CAS sha guard. The caller
  // may only claim/refresh the slot bearing ITS OWN address; the server STAMPS
  // that slot's ts (skew-free freshness). Other slots carry through as-read. A
  // 409 (someone wrote first) returns the live blob so the caller re-applies.
  if (action === 'put-slots') {
    const myIdx = Number(payload.my);
    if (!Number.isInteger(myIdx) || myIdx < 0 || myIdx >= MESH_SLOTS) {
      return json({ error: 'bad slot index' }, 400, origin);
    }
    const slotsIn = Array.isArray(payload.slots) ? (payload.slots as unknown[]) : null;
    if (!slotsIn || slotsIn.length !== MESH_SLOTS) return json({ error: 'bad slots (need 8)' }, 400, origin);
    const clean: (SlotEntry | null)[] = [];
    for (let i = 0; i < MESH_SLOTS; i++) {
      const e = slotsIn[i] as Record<string, unknown> | null;
      if (e && typeof e === 'object' && ADDR_RE.test(String(e.addr)) && JOINER_RE.test(String(e.id))) {
        clean.push({ id: String(e.id), addr: String(e.addr).toLowerCase(), ts: Number(e.ts) || 0 });
      } else {
        clean.push(null);
      }
    }
    // Anti-spoof: my slot MUST carry my authenticated address; the server stamps ts.
    const mine = clean[myIdx];
    if (!mine || mine.addr !== auth.address.toLowerCase()) {
      return json({ error: 'your slot must carry your own address' }, 403, origin);
    }
    clean[myIdx] = { id: mine.id, addr: auth.address.toLowerCase(), ts: now };
    const path = pathFor(room, 'slots');
    const expectSha = typeof payload.sha === 'string' && payload.sha ? (payload.sha as string) : undefined;
    let existing: { blob: Blob; sha: string } | null;
    try {
      existing = await ghGet(path).catch(() => null);
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    if ((existing?.sha ?? '') !== (expectSha ?? '')) {
      return json({ conflict: true, slots: existing?.blob?.slots ?? null, sha: existing?.sha ?? '', now }, 409, origin);
    }
    try {
      await ghPut(path, JSON.stringify({ slots: clean, ts: now }), `mesh ${room}/slots`, existing?.sha);
    } catch (e) {
      return json({ conflict: true, error: (e as Error).message, now }, 409, origin);
    }
    return json({ ok: true, my: myIdx, now }, 200, origin);
  }

  // clear: drop a room's slots once peers have connected (best-effort cleanup;
  // blobs also self-expire past the TTL). The 2-peer pair is always cleared; for
  // the N-peer star the host passes its known joiner ids so their per-joiner
  // offer/answer slots + the roster go too.
  if (action === 'clear') {
    const joiners = Array.isArray(payload.joiners)
      ? (payload.joiners as unknown[]).map(String).filter((s) => JOINER_RE.test(s))
      : [];
    const slots = ['offer', 'answer', 'join', ...joiners.flatMap((id) => [`offer-${id}`, `answer-${id}`])];
    try {
      for (const slot of slots) await ghDelete(pathFor(room, slot));
    } catch (e) {
      return json({ error: 'store: ' + (e as Error).message }, 502, origin);
    }
    return json({ cleared: true, room }, 200, origin);
  }

  // post: write this side's SDP blob under room/slot.
  const slot = String(payload.slot ?? '').trim();
  if (!SLOT_RE.test(slot)) return json({ error: 'bad slot' }, 400, origin);
  const sdp = String(payload.sdp ?? '');
  if (!sdp || sdp.length > 64 * 1024) return json({ error: 'sdp missing or too large' }, 400, origin);

  const path = pathFor(room, slot);
  try {
    // Overwrite any stale blob for this slot (a re-join replaces the old SDP).
    const existing = await ghGet(path).catch(() => null);
    await ghPut(path, JSON.stringify({ sdp, ts: now }), `signal ${room}/${slot}`, existing?.sha);
  } catch (e) {
    return json({ error: 'store: ' + (e as Error).message }, 502, origin);
  }
  return json({ posted: true, room, slot }, 200, origin);
}
