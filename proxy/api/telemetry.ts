// localharness telemetry — off-chain AUTO ERROR REPORTS + rich feedback.
//
// POST /api/telemetry: the browser app submits an already-REDACTED report (the
// device strips the seed / api keys / private 0x material BEFORE it leaves —
// the proxy never sees secrets) and this route files it as a GitHub issue in
// the private telemetry repo via the compusophy-bot collaborator token. This is
// the rich, off-chain counterpart to the short, public on-chain FeedbackFacet
// (design/telemetry-and-global-lessons.md). Auth = the same personal-sign token
// as gemini.ts (no new auth surface). Identities are free to mint (auth only
// proves SOME keypair signed), so the per-ADDRESS window is backed by a GLOBAL
// per-isolate cap — keypair rotation can't turn this into a spam cannon against
// the shared GitHub PAT. Dedup is the CLIENT's job (per-session signature set)
// — the proxy just files what it's given.

import { verifyAuthToken } from './_stripe';
import { SlidingWindow } from './_ratelimit';

export const config = { runtime: 'edge' };

const TELEMETRY_REPO = process.env.LH_TELEMETRY_REPO ?? 'compusophy/localharness-telemetry';
const GH_TOKEN = process.env.GH_TELEMETRY_TOKEN ?? '';
// Generous body cap — a report carries a few turns of context + a stack. GitHub
// allows ~64KB; 24KB keeps issues readable and bounds the GitHub call.
const MAX_BODY_BYTES = 24_576;
const PER_ADDR_PER_HOUR = Number(process.env.LH_TELEMETRY_RATE ?? '20');
const senderWindow = new SlidingWindow(PER_ADDR_PER_HOUR, 3_600_000);
// Global per-isolate backstop. verifyAuthToken only proves SOME secp256k1
// keypair signed a fresh message — it does NO on-chain presence check, and
// minting a fresh keypair per request is free + offline, so the per-ADDRESS
// window above is defeated by rotation. This caps TOTAL reports filed from one
// warm isolate across ALL addresses, so a rotation flood can't exhaust the
// shared GitHub PAT that publish.ts / _jobstore.ts fall back to
// (GH_TELEMETRY_TOKEN). PER-ISOLATE only (see api/_ratelimit.ts) — a determined
// attacker spread across isolates dilutes it, but it kills the single-isolate
// spam cannon the header used to (falsely) claim per-address alone prevented.
const GLOBAL_PER_HOUR = Number(process.env.LH_TELEMETRY_GLOBAL_RATE ?? '200');
const globalWindow = new SlidingWindow(GLOBAL_PER_HOUR, 3_600_000);

// --- CORS (same policy as gemini.ts / notify.ts) -----------------------------
const ALLOWED_ORIGIN_SUFFIX = '.localharness.xyz';
const ALLOWED_ORIGIN_EXACT = 'https://localharness.xyz';

function isAllowedOrigin(origin: string): boolean {
  if (origin === ALLOWED_ORIGIN_EXACT || origin.endsWith(ALLOWED_ORIGIN_SUFFIX)) return true;
  try {
    const u = new URL(origin);
    return u.protocol === 'http:' && (u.hostname === 'localhost' || u.hostname === '127.0.0.1');
  } catch {
    return false;
  }
}
function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
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

function clean(s: unknown, max: number): string {
  return String(s ?? '').trim().slice(0, max);
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: corsHeaders(origin) });
  if (req.method !== 'POST') return json({ error: 'POST only' }, 405, origin);
  if (!GH_TOKEN) return json({ error: 'telemetry not configured' }, 503, origin);

  // Auth — personal-sign token (address:ts:sig), 300s freshness.
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
  let addr: string;
  try {
    addr = verifyAuthToken(token);
  } catch (e) {
    return json({ error: 'auth: ' + (e as Error).message }, 401, origin);
  }

  // Rate limit per authenticated address (a report is cheap but GitHub isn't).
  const wait = senderWindow.hit(addr);
  if (wait > 0) {
    return json({ error: `rate limited: ${PER_ADDR_PER_HOUR} reports/hour`, retryAfterSeconds: wait }, 429, origin);
  }
  // Global per-isolate cap — the real backstop against free keypair rotation
  // (the per-address window above is bypassed by minting a fresh address per
  // request). Bounds total GitHub filings/hour so a flood can't trip the shared
  // PAT's secondary rate limit and break app publishing / off-chain scheduling.
  const gwait = globalWindow.hit('telemetry');
  if (gwait > 0) {
    return json({ error: 'telemetry rate limited (global backstop)', retryAfterSeconds: gwait }, 429, origin);
  }

  let payload: Record<string, unknown>;
  try {
    payload = await req.json();
  } catch {
    return json({ error: 'bad json' }, 400, origin);
  }

  const kind = clean(payload.kind, 24).replace(/[^a-z-]/g, '') || 'error';
  const title = clean(payload.title, 180);
  const sig = clean(payload.signature, 24).replace(/[^a-zA-Z0-9_-]/g, '');
  const body = clean(payload.body, MAX_BODY_BYTES);
  if (!title) return json({ error: 'empty title' }, 400, origin);

  const issueTitle = `[${kind}] ${title}${sig ? ` (${sig})` : ''}`;
  const issueBody =
    `Auto-submitted from \`${addr}\` — REDACTED on-device (no seed/keys).\n\n` +
    `${body}\n\n---\n*localharness telemetry · design/telemetry-and-global-lessons.md*`;

  const ghHeaders = {
    authorization: `Bearer ${GH_TOKEN}`,
    accept: 'application/vnd.github+json',
    'content-type': 'application/json',
    'user-agent': 'localharness-telemetry',
  };

  // Dedup: if an OPEN issue already carries this exact signature in its title,
  // skip instead of opening a duplicate. This is what kills the "23 identical
  // 429 issues" spam — one issue per (code+)signature. Best-effort: any search
  // hiccup just falls through to a normal create.
  if (sig) {
    try {
      const q = encodeURIComponent(`repo:${TELEMETRY_REPO} is:issue is:open in:title ${sig}`);
      const sres = await fetch(`https://api.github.com/search/issues?q=${q}&per_page=5`, {
        headers: ghHeaders,
      });
      if (sres.ok) {
        const found = (await sres.json()) as {
          items?: Array<{ number: number; html_url: string; title: string }>;
        };
        const hit = found.items?.find((i) => i.title.includes(`(${sig})`));
        if (hit) {
          return json({ filed: true, deduped: true, url: hit.html_url, number: hit.number }, 200, origin);
        }
      }
    } catch {
      /* fall through to create */
    }
  }

  const res = await fetch(`https://api.github.com/repos/${TELEMETRY_REPO}/issues`, {
    method: 'POST',
    headers: ghHeaders,
    // `labels:[kind]` self-sorts reports (error / feedback / cartridge); GitHub
    // auto-creates the label on first use.
    body: JSON.stringify({ title: issueTitle, body: issueBody, labels: [kind] }),
  });
  if (!res.ok) {
    const detail = (await res.text()).slice(0, 200);
    return json({ error: 'github filing failed', status: res.status, detail }, 502, origin);
  }
  const issue = (await res.json()) as { html_url?: string; number?: number };
  return json({ filed: true, url: issue.html_url, number: issue.number }, 200, origin);
}
