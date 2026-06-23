// /api/apps — FREE, read-only catalog of PUBLISHED apps (the off-chain app store).
//
// Lists the names that have a published cartridge (`<name>/app.wasm`) in the
// localharness-apps repo, one per line — the app-store sibling of `/api/agents`
// (which lists registered agents). Apps live OFF-CHAIN (the chain keeps only
// ownership), so discovery reads GitHub, not the chain. The plain-text, one-name-
// per-line shape is what a rustlite cartridge consumes via host::http
// body_lines/draw_line (the `directory` cartridge pattern), and the CLI
// `localharness apps` prints it. No auth, no $LH — discovery is the demand
// on-ramp and must be frictionless (mirrors `/api/agents`).

export const config = { runtime: 'edge' };

const APPSTORE_REPO = process.env.GH_APPSTORE_REPO ?? 'compusophy/localharness-apps';
// Public repo — the tree read works unauthed; a token (shared with telemetry/
// publish) just buys the 5000/hr rate limit instead of 60/hr. Best-effort.
const GH_TOKEN = process.env.GH_APPSTORE_TOKEN ?? process.env.GH_TELEMETRY_TOKEN ?? '';
const CAP = 200;

export default async function handler(_req: Request): Promise<Response> {
  const headers: Record<string, string> = {
    'content-type': 'text/plain; charset=utf-8',
    // 60s cache — the catalog is live but a window bounds the GitHub tree reads.
    'cache-control': 'public, max-age=60',
    'access-control-allow-origin': '*',
  };
  try {
    const h: Record<string, string> = {
      accept: 'application/vnd.github+json',
      'user-agent': 'localharness-appstore',
    };
    if (GH_TOKEN) h.authorization = `Bearer ${GH_TOKEN}`;
    const res = await fetch(
      `https://api.github.com/repos/${APPSTORE_REPO}/git/trees/main?recursive=1`,
      { headers: h },
    );
    if (!res.ok) return new Response('', { status: 200, headers });
    const body = (await res.json()) as { tree?: Array<{ path: string; type: string }> };
    const names: string[] = [];
    const seen = new Set<string>();
    for (const t of body.tree ?? []) {
      // A published cartridge is `<name>/app.wasm`. (An html-only face has
      // `<name>/index.html` and no app.wasm — not an "app".)
      if (t.type === 'blob' && t.path.endsWith('/app.wasm')) {
        const name = t.path.split('/')[0];
        if (name && !seen.has(name)) {
          seen.add(name);
          names.push(name);
          if (names.length >= CAP) break;
        }
      }
    }
    return new Response(names.join('\n'), { status: 200, headers });
  } catch {
    // Never 500 — an empty body renders as an empty catalog, not a broken one.
    return new Response('', { status: 200, headers });
  }
}
