// /api/publish — OFF-CHAIN cartridge publish (the "app store" WRITE path).
//
// localharness apps (compiled rustlite cartridges) live OFF-CHAIN now: the
// blockchain keeps only OWNERSHIP/provenance (the name NFT + signature proof),
// while the app BYTES go to GitHub — exactly the model telemetry/feedback use
// (free, no gas, no sponsor drain). Publishing a cartridge on-chain via
// `setMetadata` cost ~$0.32–$2.80 each and drained the mainnet gas sponsor; this
// kills that cost entirely (see the off-chain-apps pivot).
//
// Auth = the SAME personal-sign token as gemini.ts/telemetry.ts (no new auth
// surface): `address:timestamp:signature` in `x-goog-api-key`, 300s freshness.
// We then require the caller to OWN the name on-chain (ownerOf(idOfName(name)))
// before committing `<name>/app.wasm` (+ `<name>/app.rl` source) to the
// localharness-apps repo via the GitHub Contents API. The serve sibling is
// `app.ts` (GET, CDN-cached). Ownership stays on-chain; only the bytes move.

import { verifyAuthToken } from './_stripe';
import { ethCall, selector, isAllowedOrigin } from './_auth';
import { hexToBytes, bytesToHex } from '@noble/hashes/utils';

export const config = { runtime: 'edge' };

const APPSTORE_REPO = process.env.GH_APPSTORE_REPO ?? 'compusophy/localharness-apps';
// A SEPARATE PAT from telemetry's (different repo, separation of concerns), but
// fall back to GH_TELEMETRY_TOKEN if it is a repo-scoped classic PAT that can
// also write localharness-apps — so the store works the moment this ships,
// before a dedicated token is provisioned.
const GH_TOKEN = process.env.GH_APPSTORE_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '';
// Off-chain storage has no gas cap. GitHub's Contents API has FULL feature support
// up to 1 MB (1–100 MB needs the raw media type / Git Data API; >100 MB
// unsupported), so bound publishes at 1 MB. A top-level public face may use it
// all; a cartridge embedded as a host::compose CHILD stays bounded by the compose
// budget (16 KB/child) enforced at spawn. Mirrors registry::APP_STORE_MAX_WASM_BYTES.
const MAX_WASM_BYTES = 1_048_576;
const MAX_SOURCE_BYTES = 1_048_576;
const MAX_HTML_BYTES = 1_048_576;

// --- CORS (same policy as telemetry.ts / gemini.ts) --------------------------
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

// --- on-chain ownership reads (the diamond, via _auth's ethCall) -------------

/** ABI-encode a single `string` arg (offset 0x20 | length | utf8 padded). */
function encodeStringArg(value: string): string {
  const bytes = new TextEncoder().encode(value);
  const len = bytes.length;
  const padded = Math.ceil(len / 32) * 32;
  const buf = new Uint8Array(32 + 32 + padded);
  buf[31] = 0x20;
  let x = len;
  for (let i = 63; i >= 32 && x > 0; i--) {
    buf[i] = x & 0xff;
    x = Math.floor(x / 256);
  }
  buf.set(bytes, 64);
  return bytesToHex(buf);
}

/** `idOfName(string) -> uint256`. 0 = unregistered. */
async function idOfName(name: string): Promise<bigint> {
  const res = await ethCall('0x' + selector('idOfName(string)') + encodeStringArg(name));
  try {
    return BigInt(res);
  } catch {
    return 0n;
  }
}

/** `ownerOf(uint256) -> address` (ERC721). null for a zero/short result. */
async function ownerOfToken(tokenId: bigint): Promise<string | null> {
  const word = tokenId.toString(16).padStart(64, '0');
  const res = await ethCall('0x' + selector('ownerOf(uint256)') + word);
  const h = res.replace(/^0x/, '');
  if (h.length < 64) return null;
  const addr = '0x' + h.slice(-40);
  return /^0x0+$/.test(addr) ? null : addr.toLowerCase();
}

// --- GitHub Contents API (commit a file; update needs the prior blob sha) -----

function ghHeaders(): Record<string, string> {
  return {
    authorization: `Bearer ${GH_TOKEN}`,
    accept: 'application/vnd.github+json',
    'content-type': 'application/json',
    'user-agent': 'localharness-appstore',
  };
}

async function ghGetSha(path: string): Promise<string | null> {
  const res = await fetch(
    `https://api.github.com/repos/${APPSTORE_REPO}/contents/${path}?ref=main`,
    { headers: ghHeaders() },
  );
  if (res.status === 404) return null;
  if (!res.ok) throw new Error(`get ${path}: ${res.status}`);
  const j = (await res.json()) as { sha?: string };
  return j.sha ?? null;
}

async function ghPut(path: string, contentB64: string, message: string): Promise<void> {
  const sha = await ghGetSha(path); // updating an existing file needs its sha
  const res = await fetch(`https://api.github.com/repos/${APPSTORE_REPO}/contents/${path}`, {
    method: 'PUT',
    headers: ghHeaders(),
    body: JSON.stringify({ message, content: contentB64, branch: 'main', ...(sha ? { sha } : {}) }),
  });
  if (!res.ok) {
    const d = await res.text();
    throw new Error(`put ${path}: ${res.status} ${d.slice(0, 200)}`);
  }
}

/** Base64 of raw bytes (chunked so a large cartridge doesn't blow the arg cap). */
function bytesToBase64(b: Uint8Array): string {
  let s = '';
  const chunk = 0x8000;
  for (let i = 0; i < b.length; i += chunk) {
    s += String.fromCharCode(...b.subarray(i, i + chunk));
  }
  return btoa(s);
}

export default async function handler(req: Request): Promise<Response> {
  const origin = req.headers.get('origin');
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: corsHeaders(origin) });
  if (req.method !== 'POST') return json({ error: 'POST only' }, 405, origin);
  if (!GH_TOKEN) return json({ error: 'appstore not configured (no GitHub token)' }, 503, origin);

  // Auth — personal-sign token (address:ts:sig), 300s freshness.
  const token = req.headers.get('x-goog-api-key') ?? req.headers.get('x-api-key') ?? '';
  let addr: string;
  try {
    // Route-bind the token to this endpoint (audit L9).
    addr = verifyAuthToken(token, 'publish');
  } catch (e) {
    return json({ error: 'auth: ' + (e as Error).message }, 401, origin);
  }

  let payload: Record<string, unknown>;
  try {
    payload = await req.json();
  } catch {
    return json({ error: 'bad json' }, 400, origin);
  }

  const name = String(payload.name ?? '').trim().toLowerCase();
  if (!/^[a-z0-9-]{1,63}$/.test(name)) return json({ error: 'invalid name' }, 400, origin);

  // Three shapes, ONE auth/ownership gate: an APP cartridge (`wasm_hex`), an
  // HTML page (`html`), or a FACE-ONLY choice write (`face` with no content —
  // how "directory" is picked). The face CHOICE lives in the store too
  // (`<name>/face`): a content publish stamps its own kind so publishing an
  // app/html page IS choosing it — the chain keeps only ownership, and NO
  // public-face path writes on-chain. Build the commit list, then gate, then write.
  const htmlRaw = typeof payload.html === 'string' ? (payload.html as string) : '';
  const isHtml = htmlRaw.trim() !== '';
  const faceRaw = typeof payload.face === 'string' ? (payload.face as string).trim().toLowerCase() : '';
  const hasWasm = String(payload.wasm_hex ?? '').trim() !== '';
  if (faceRaw && !['directory', 'app', 'html'].includes(faceRaw)) {
    return json({ error: 'face must be one of: directory, app, html' }, 400, origin);
  }
  type Commit = { path: string; b64: string; message: string };
  const commits: Commit[] = [];
  let bytes = 0;
  let primaryPath = '';
  let kind = '';

  if (isHtml) {
    const htmlBytes = new TextEncoder().encode(htmlRaw);
    if (htmlBytes.length > MAX_HTML_BYTES) {
      return json({ error: `html too large (${htmlBytes.length} > ${MAX_HTML_BYTES})` }, 413, origin);
    }
    bytes = htmlBytes.length;
    kind = 'html';
    primaryPath = `${name}/index.html`;
    commits.push({ path: primaryPath, b64: bytesToBase64(htmlBytes), message: `publish ${name}/index.html (${bytes} bytes)` });
  } else if (hasWasm) {
    // wasm arrives as hex (the CLI/browser already speak hex; no base64 dep
    // needed client-side). Validate shape + the wasm magic before any write.
    const wasmHex = String(payload.wasm_hex ?? '').replace(/^0x/, '');
    if (wasmHex.length === 0 || wasmHex.length % 2 !== 0 || !/^[0-9a-fA-F]+$/.test(wasmHex)) {
      return json({ error: 'invalid wasm_hex (or supply `html`)' }, 400, origin);
    }
    const wasm = hexToBytes(wasmHex);
    if (wasm.length > MAX_WASM_BYTES) {
      return json({ error: `wasm too large (${wasm.length} > ${MAX_WASM_BYTES})` }, 413, origin);
    }
    if (!(wasm[0] === 0x00 && wasm[1] === 0x61 && wasm[2] === 0x73 && wasm[3] === 0x6d)) {
      return json({ error: 'not a WebAssembly module (bad magic)' }, 400, origin);
    }
    bytes = wasm.length;
    kind = 'app';
    primaryPath = `${name}/app.wasm`;
    commits.push({ path: primaryPath, b64: bytesToBase64(wasm), message: `publish ${name}/app.wasm (${bytes} bytes)` });
    const source = String(payload.source ?? '');
    if (source.length > MAX_SOURCE_BYTES) return json({ error: 'source too large' }, 413, origin);
    if (source.trim()) {
      commits.push({ path: `${name}/app.rl`, b64: bytesToBase64(new TextEncoder().encode(source)), message: `publish ${name}/app.rl` });
    }
  } else if (faceRaw) {
    // Face-only: no bytes, just the choice (e.g. switch back to "directory").
    kind = faceRaw;
    primaryPath = `${name}/face`;
  } else {
    return json({ error: 'supply `wasm_hex`, `html`, or `face`' }, 400, origin);
  }

  // Stamp the face record: a content publish stamps its own kind (last publish
  // wins) unless an explicit `face` overrides it; a face-only POST stamps the
  // asked choice. ONE authed write covers bytes + routing — the "stored but
  // visitors saw the directory" bug (krafto) is structurally impossible here.
  const faceChoice = faceRaw || kind;
  commits.push({
    path: `${name}/face`,
    b64: btoa(faceChoice),
    message: `face ${name} -> ${faceChoice}`,
  });

  // Ownership: the authenticated caller MUST own the name on-chain. This is the
  // single authorization gate — the bytes are public, the NFT is the right to
  // publish them under this name.
  let tokenId: bigint;
  try {
    tokenId = await idOfName(name);
  } catch (e) {
    return json({ error: 'rpc: ' + (e as Error).message }, 502, origin);
  }
  if (tokenId === 0n) return json({ error: `"${name}" is not registered on-chain` }, 404, origin);
  let owner: string | null;
  try {
    owner = await ownerOfToken(tokenId);
  } catch (e) {
    return json({ error: 'rpc: ' + (e as Error).message }, 502, origin);
  }
  if (!owner || owner.toLowerCase() !== addr.toLowerCase()) {
    return json({ error: `"${name}" is owned by ${owner ?? '(none)'}, not ${addr}` }, 403, origin);
  }

  // Commit the asset(s) to the app-store repo. The face stamp is the LAST
  // commit and is what routes visitors, so a transient GitHub failure there
  // would strand fresh bytes behind the OLD face — retry it once before
  // failing, and name the stranded stamp in the error so the client retries
  // the publish (never falls back to a write the store record would shadow).
  try {
    for (const c of commits) {
      try {
        await ghPut(c.path, c.b64, c.message);
      } catch (e) {
        if (!c.path.endsWith('/face')) throw e;
        await ghPut(c.path, c.b64, c.message); // one retry for the routing stamp
      }
    }
  } catch (e) {
    return json({ error: 'github: ' + (e as Error).message }, 502, origin);
  }

  return json(
    { published: true, name, kind, face: faceChoice, bytes, repo: APPSTORE_REPO, path: primaryPath },
    200,
    origin,
  );
}
