// localharness credit proxy — WEB FETCH grounding route (Edge).
//
// POST /api/fetch { url } → fetches a LIVE external HTTPS resource on the
// caller's behalf and returns its text body as JSON. This exists because the
// browser-resident agent cannot fetch arbitrary origins itself (CORS), and the
// proxy is the platform's ONE accepted off-chain component — so it does the
// fetching. The agent uses this to GROUND itself: GitHub READMEs, docs pages,
// JSON APIs (GitHub issue #27, on-chain feedback #57/58).
//
// AUTH + BILLING are byte-compatible with api/gemini.ts: the caller sends a
// localharness auth token `<address>:<timestamp>:<signature>` (an Ethereum
// personal-sign over `localharness-proxy:<address>:<timestamp>`) in
// `x-goog-api-key` (or `x-api-key`), the proxy recovers the signer, gates on an
// active SessionFacet session OR a CreditMeterFacet balance, and debits the
// SAME flat per-request cost a Gemini model call costs — a paid capability
// like any other proxied call.
//
// GUARDS (all checked BEFORE the $LH debit — nothing proxy-side may fail
// after the caller is charged except the upstream fetch itself):
//   • https only;
//   • DENY private/internal targets by HOSTNAME PATTERN: localhost /
//     *.localhost, 127.*, 10.*, 172.16-31.*, 192.168.*, 169.254.*, 0.*,
//     *.internal, IPv6 literals, bare-numeric hosts (decimal/hex IP forms),
//     and the proxy's own host;
//   • 15s total timeout; at most 3 redirects, each hop's target re-checked
//     through the same hostname guard;
//   • response capped at 200KB (truncated with a marker, never an error);
//   • only textual content-types (text/*, application/json, application/xml,
//     and their +json/+xml structured-syntax suffixes) return a body —
//     anything else returns { status, contentType, note: "binary skipped" }.
//
// KNOWN LIMIT (documented, accepted for the testnet): the Edge runtime cannot
// resolve DNS, so the private-target denylist filters on hostname STRING
// PATTERNS only. A public DNS name that resolves to a private IP (DNS
// rebinding) is NOT caught. Mitigations: the proxy holds no private network
// (Vercel Edge egress is the public internet), responses are size-capped, and
// every request is signed + metered, so probing costs real $LH per attempt.

export const config = { runtime: 'edge' };

// ---- constants (mirror api/gemini.ts) ---------------------------------------

// Auth + metering primitives (CORS allow-check, personal-sign recovery +
// freshness, creditOf/sessionExpiryOf reads, the meter debit) are SHARED in
// `_auth.ts` (§5 dedup) — byte-for-byte the logic that used to be inlined here.
import {
  isAllowedOrigin,
  verifyAuthToken,
  sessionExpiryOf,
  creditOf,
  meterDebit,
  InsufficientCreditError,
} from './_auth';

// SAME per-request price as a (Gemini) model call — the platform FLOOR price for
// any paid capability. Single source of truth: `_prices.ts` (default 1 $LH,
// env-overridable via COST_PER_REQUEST_WEI). web_fetch is a paid capability like
// a model turn.
import { COST_PER_REQUEST_WEI } from './_prices';

// Fetch behaviour.
const FETCH_TIMEOUT_MS = 15_000; // total budget across all redirect hops
const MAX_REDIRECTS = 3;
const MAX_RESPONSE_BYTES = 204_800; // 200KB
const TRUNCATION_MARKER = '\n…[truncated at 200KB]';
const MAX_REQUEST_BODY_BYTES = 16_384; // { url } is tiny

// ---- CORS (same policy as gemini.ts; isAllowedOrigin shared via _auth.ts) -----

function corsHeaders(origin: string | null): Record<string, string> {
  const h: Record<string, string> = {
    'Access-Control-Allow-Methods': 'POST, OPTIONS',
    'Access-Control-Allow-Headers': 'content-type, x-goog-api-key, x-api-key',
    'Vary': 'Origin',
  };
  if (origin && isAllowedOrigin(origin)) {
    h['Access-Control-Allow-Origin'] = origin;
  }
  return h;
}

function json(body: unknown, status: number, origin: string | null): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', ...corsHeaders(origin) },
  });
}

// ---- target validation -------------------------------------------------------

/**
 * Why `null` (allowed) / a reason string (denied): private/internal network
 * targets must never be fetchable through the proxy. Hostname STRING patterns
 * only — Edge cannot resolve DNS, so a public name resolving to a private IP
 * (DNS rebinding) is not caught here (documented at the top of this file).
 */
function denyReason(hostname: string, ownHost: string): string | null {
  // Normalize: lowercase, strip one trailing dot (FQDN form), strip brackets.
  let h = hostname.toLowerCase();
  if (h.endsWith('.')) h = h.slice(0, -1);
  if (h === '') return 'empty hostname';
  // IPv6 literals (URL.hostname keeps the brackets) — denied wholesale: every
  // textual-web target we care about is reachable by name or IPv4.
  if (h.startsWith('[') || h.includes(':')) return 'IPv6 literals are not allowed';
  if (h === 'localhost' || h.endsWith('.localhost')) return 'localhost is not allowed';
  if (h.endsWith('.internal')) return '*.internal is not allowed';
  if (ownHost && h === ownHost.toLowerCase()) return 'the proxy itself is not a valid target';
  // Dotted-quad IPv4 → check private/reserved ranges.
  const quad = h.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (quad) {
    const [a, b] = [Number(quad[1]), Number(quad[2])];
    if (a === 127) return 'loopback address';
    if (a === 10) return 'private address (10.0.0.0/8)';
    if (a === 0) return 'reserved address (0.0.0.0/8)';
    if (a === 169 && b === 254) return 'link-local address (169.254.0.0/16)';
    if (a === 192 && b === 168) return 'private address (192.168.0.0/16)';
    if (a === 172 && b >= 16 && b <= 31) return 'private address (172.16.0.0/12)';
    return null; // a public IPv4 is fine
  }
  // A bare-numeric host that ISN'T a dotted quad is an exotic IP encoding
  // (decimal `2130706433`, hex `0x7f000001`, mixed octal) — fetch would
  // happily decode it to an address our range checks above never saw.
  if (/^[0-9]+$/.test(h) || /^0x[0-9a-f]+$/.test(h) || /^[0-9.]+$/.test(h)) {
    return 'numeric host encodings are not allowed';
  }
  return null;
}

/** Parse + guard a target URL. Returns the URL or throws with a clean reason. */
function guardTarget(raw: string | URL, base: URL | null, ownHost: string): URL {
  let u: URL;
  try {
    u = base ? new URL(raw, base) : new URL(raw);
  } catch {
    throw new Error('invalid url');
  }
  if (u.protocol !== 'https:') {
    throw new Error('https only');
  }
  const reason = denyReason(u.hostname, ownHost);
  if (reason) throw new Error('denied target: ' + reason);
  return u;
}

// ---- content handling ----------------------------------------------------------

/** Textual content-types we return a body for: text/*, application/json,
 * application/xml, and the +json/+xml structured-syntax suffixes (still
 * text — e.g. application/ld+json, application/atom+xml). */
function isTextual(mime: string): boolean {
  if (mime.startsWith('text/')) return true;
  if (mime === 'application/json' || mime === 'application/xml') return true;
  if (mime.startsWith('application/') && (mime.endsWith('+json') || mime.endsWith('+xml'))) {
    return true;
  }
  return false;
}

/** Read up to MAX_RESPONSE_BYTES of `resp`'s body as UTF-8 text. Truncation is
 * NOT an error — the reader cancels at the cap and the caller gets a marker. */
async function readBodyCapped(resp: Response): Promise<{ body: string; truncated: boolean }> {
  const stream = resp.body;
  if (!stream) return { body: '', truncated: false };
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  let truncated = false;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    if (value) {
      if (total + value.length > MAX_RESPONSE_BYTES) {
        chunks.push(value.slice(0, MAX_RESPONSE_BYTES - total));
        total = MAX_RESPONSE_BYTES;
        truncated = true;
        await reader.cancel().catch(() => {});
        break;
      }
      chunks.push(value);
      total += value.length;
    }
  }
  const merged = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    merged.set(c, off);
    off += c.length;
  }
  // Non-fatal decode: a truncation point mid-multibyte-codepoint must not throw.
  let body = new TextDecoder('utf-8', { fatal: false }).decode(merged);
  if (truncated) body += TRUNCATION_MARKER;
  return { body, truncated };
}

// ---- handler -------------------------------------------------------------------

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');

  if (req.method === 'OPTIONS') {
    return new Response(null, { status: 204, headers: corsHeaders(origin) });
  }
  if (req.method !== 'POST') {
    return json({ error: 'method not allowed' }, 405, origin);
  }

  try {
    // ---- request body: { url } -------------------------------------------------
    const declaredLen = Number(req.headers.get('content-length') ?? '0');
    if (Number.isFinite(declaredLen) && declaredLen > MAX_REQUEST_BODY_BYTES) {
      return json({ error: 'request body too large' }, 413, origin);
    }
    let rawUrl: string;
    try {
      const parsed = (await req.json()) as { url?: unknown };
      rawUrl = typeof parsed.url === 'string' ? parsed.url.trim() : '';
    } catch {
      return json({ error: 'invalid JSON body' }, 400, origin);
    }
    if (!rawUrl) {
      return json({ error: 'missing url' }, 400, origin);
    }

    // ---- target guard (BEFORE auth/debit — a denied target must cost nothing) --
    const ownHost = new URL(req.url).hostname;
    let target: URL;
    try {
      target = guardTarget(rawUrl, null, ownHost);
    } catch (e) {
      return json({ error: (e as Error).message }, 400, origin);
    }

    // ---- AUTH — same token scheme + headers as api/gemini.ts (verifyAuthToken
    // in _auth.ts is byte-for-byte the prior inlined parse/freshness/recovery) --
    const token =
      req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
    const now = Math.floor(Date.now() / 1000);
    // Route-bind the token to this endpoint (audit L9).
    const auth = verifyAuthToken(token, now, 'fetch');
    if (!auth.ok) {
      return json({ error: auth.error }, auth.status, origin);
    }
    const address = auth.address;

    // ---- credit gate + meter debit — same model as a Gemini model call ---------
    const cost = COST_PER_REQUEST_WEI;
    const [expiry, credit] = await Promise.all([
      sessionExpiryOf(address),
      creditOf(address),
    ]);
    const hasSession = expiry > BigInt(now);
    const hasCredit = credit >= cost;
    if (!hasSession && !hasCredit) {
      return json(
        {
          error:
            'no $LH credit or active session for this identity — fund the per-request meter (localharness redeem / send / topup) or open a session explicitly (localharness session). See https://localharness.xyz/llms.txt',
        },
        402,
        origin,
      );
    }
    // Prefer per-request metering over a lingering free session (gemini.ts
    // rationale: a funded meter means the caller opted into per-call billing).
    if (hasCredit) {
      try {
        await meterDebit(address, cost);
      } catch (e) {
        if (e instanceof InsufficientCreditError) {
          if (!hasSession) {
            return json(
              {
                error:
                  'insufficient $LH credit — the on-chain debit reverted (balance changed since the gate read)',
              },
              402,
              origin,
            );
          }
          // else: covered by an active session — fall through and serve.
        } else {
          return json({ error: 'metering failed: ' + (e as Error).message }, 502, origin);
        }
      }
    }

    // ---- the fetch itself — the one failure a debited caller pays for ----------
    // One AbortController = ONE 15s budget across every redirect hop AND the
    // body read, so neither a slow-redirect chain nor a trickling body can
    // multiply the timeout.
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), FETCH_TIMEOUT_MS);
    try {
      let upstream: Response;
      let current = target;
      for (let hop = 0; ; hop++) {
        const resp = await fetch(current.toString(), {
          method: 'GET',
          redirect: 'manual',
          signal: ctrl.signal,
          headers: {
            accept: 'text/html, text/plain, application/json, application/xml, text/*;q=0.9, */*;q=0.1',
            'user-agent': 'localharness-webfetch/1.0 (+https://localharness.xyz)',
          },
        });
        const loc = resp.headers.get('location');
        if (resp.status >= 300 && resp.status < 400 && loc) {
          await resp.body?.cancel().catch(() => {});
          if (hop >= MAX_REDIRECTS) {
            return json({ error: `too many redirects (max ${MAX_REDIRECTS})` }, 502, origin);
          }
          // EVERY hop goes back through the full guard: https-only + the
          // private-target denylist — an allowed public host must not be able
          // to bounce the proxy into a denied one.
          try {
            current = guardTarget(loc, current, ownHost);
          } catch (e) {
            return json(
              { error: 'redirect ' + (e as Error).message + ': ' + loc },
              400,
              origin,
            );
          }
          continue;
        }
        upstream = resp;
        break;
      }

      const contentType = upstream.headers.get('content-type') ?? '';
      const mime = contentType.split(';')[0].trim().toLowerCase();
      if (!isTextual(mime)) {
        await upstream.body?.cancel().catch(() => {});
        return json(
          { status: upstream.status, contentType, note: 'binary skipped' },
          200,
          origin,
        );
      }

      const { body, truncated } = await readBodyCapped(upstream);
      return json(
        { status: upstream.status, contentType, truncated, body },
        200,
        origin,
      );
    } catch (e) {
      if ((e as Error).name === 'AbortError') {
        return json(
          { error: `fetch timed out after ${FETCH_TIMEOUT_MS / 1000}s` },
          504,
          origin,
        );
      }
      return json({ error: 'fetch failed: ' + (e as Error).message }, 502, origin);
    } finally {
      clearTimeout(timer);
    }
  } catch (e) {
    return json({ error: (e as Error).message }, 500, origin);
  }
}
