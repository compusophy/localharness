// /api/agents — FREE, read-only network directory feed.
//
// Returns the registered agents as COMPACT PLAIN TEXT: one name per line,
// most-recent first, capped. This is the GET sibling of mcp.ts's POST
// `discover_agents` — a format a rustlite cartridge can consume via host::http
// (its host-held `body_lines` / `draw_line` render each line as text). The
// `directory` agent's cartridge fetches this to show the LIVE network.
//
// Reuses mcp.ts's PROVEN registry reads (nextId + nameOfId over the same diamond)
// so the selectors can't drift. No auth, no $LH — discovery is the demand on-ramp
// and must be frictionless (mirrors discover_agents being FREE).

export const config = { runtime: 'edge' };

import { nextId, nameOfId } from './mcp';

// How many of the most-recent token ids to surface. Env-overridable; bounds the
// per-request RPC fan-out (one nameOfId per id).
const SCAN_CAP = ((): number => {
  const n = Number(process.env.DIRECTORY_SCAN_CAP ?? '60');
  return Number.isFinite(n) && n > 0 && n <= 200 ? Math.floor(n) : 60;
})();

export default async function handler(_req: Request): Promise<Response> {
  const headers: Record<string, string> = {
    'content-type': 'text/plain; charset=utf-8',
    // Short cache — the directory is live but a 30s window bounds RPC load.
    'cache-control': 'public, max-age=30',
    'access-control-allow-origin': '*',
  };
  try {
    const next = await nextId();
    const lines: string[] = [];
    if (next > 1n) {
      const hi = next - 1n;
      const lo = hi - BigInt(SCAN_CAP) + 1n;
      const start = lo > 1n ? lo : 1n;
      for (let tid = hi; tid >= start; tid--) {
        let name = '';
        try {
          name = await nameOfId(tid);
        } catch {
          continue; // a burned/odd id can't abort the scan
        }
        if (name) lines.push(name);
      }
    }
    return new Response(lines.join('\n'), { status: 200, headers });
  } catch {
    // Never 500 — an empty body renders as an empty directory, not a broken one.
    return new Response('', { status: 200, headers });
  }
}
