// /api/app?name=<name> — OFF-CHAIN cartridge SERVE path (the "app store" read).
//
// Returns the raw WebAssembly module bytes a `<name>` published off-chain (via
// publish.ts → GitHub), as `application/wasm`. The browser fetches this by name
// (registry::app_wasm_from_store), gets an ArrayBuffer, and hands it straight to
// WebAssembly.compile — no hex, no wrapping. CDN-cached (s-maxage) so repeat
// views hit the edge cache, not this function or GitHub — the "don't hammer the
// proxy" property without standing up a separate service.
//
// We fetch the bytes from raw.githubusercontent (the repo is PUBLIC, so no token
// is needed for reads) and re-serve with CORS — raw.githubusercontent itself
// sends no CORS header, which is exactly why the browser can't fetch it directly
// and this thin proxy exists. 404 => the name has published no app (the caller
// falls back to the directory face).

export const config = { runtime: 'edge' };

const APPSTORE_REPO = process.env.GH_APPSTORE_REPO ?? 'compusophy/localharness-apps';

const CORS: Record<string, string> = {
  'access-control-allow-origin': '*',
  'access-control-allow-methods': 'GET, OPTIONS',
};

export default async function handler(req: Request): Promise<Response> {
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: CORS });

  const url = new URL(req.url);
  const name = (url.searchParams.get('name') ?? '').toLowerCase().replace(/[^a-z0-9-]/g, '');
  if (!name) return new Response('missing name', { status: 400, headers: CORS });

  // kind=html serves the published HTML page face; default = the app cartridge.
  const isHtml = (url.searchParams.get('kind') ?? '') === 'html';
  const file = isHtml ? 'index.html' : 'app.wasm';
  const contentType = isHtml ? 'text/html; charset=utf-8' : 'application/wasm';

  const raw = `https://raw.githubusercontent.com/${APPSTORE_REPO}/main/${name}/${file}`;
  let res: Response;
  try {
    res = await fetch(raw, { headers: { 'user-agent': 'localharness-appstore' } });
  } catch (e) {
    return new Response('upstream error: ' + (e as Error).message, { status: 502, headers: CORS });
  }
  if (res.status === 404) {
    // Short cache: a name with no asset yet may publish one soon.
    return new Response('not found', {
      status: 404,
      headers: { ...CORS, 'cache-control': 'public, max-age=30' },
    });
  }
  if (!res.ok) return new Response('upstream ' + res.status, { status: 502, headers: CORS });

  const buf = await res.arrayBuffer();
  return new Response(buf, {
    status: 200,
    headers: {
      ...CORS,
      'content-type': contentType,
      // Edge + browser cache 5 min — repeat views never re-hit GitHub.
      'cache-control': 'public, max-age=300, s-maxage=300',
    },
  });
}
