// _env.ts — fail-LOUD env assertions (road-to-v1 step 2: the proxy is the SPOF
// and metering was correct-by-env-only). Each handler asserts ITS critical vars
// up front and returns a named 503 `LH_PROXY_MISCONFIG` instead of silently
// degrading (a missing meter key = served-but-unbilled inference; a missing
// mainnet sponsor key = the relay signing with the committed public testnet
// key). Optional-BY-DESIGN vars (TURN_*, VAPID_*, LH_METER_PAYEE, GEMINI_API_KEYS
// pool, …) are feature toggles, NOT misconfigs — never assert those.

/**
 * The upstream Gemini key for ONE request. `GEMINI_API_KEYS` (comma-separated)
 * spreads Google's per-key quota across N keys (#23); unset falls back to the
 * single `GEMINI_API_KEY` and behaves exactly as before the pool.
 *
 * ⛔ This is the ONE key selector for every inference path — browser chat
 * (`gemini.ts`), the cron job worker (`scheduler.ts`) and MCP-over-HTTP
 * (`mcp.ts`). It lives here because those three used to read the env
 * themselves: rotating `GEMINI_API_KEYS` fixed chat while scheduled jobs and
 * MCP stayed pointed at the dead single key, which is a silent half-outage.
 * New inference callers MUST come through here.
 *
 * Honest limits: the pick is RANDOM, not round-robin — a serverless invocation
 * has no shared cursor to advance — and there is NO per-key health tracking or
 * failover, so one suspended key in a pool of N fails ~1/N of requests until it
 * is removed from the env.
 */
export function geminiUpstreamKey(): string | undefined {
  const pool = (process.env.GEMINI_API_KEYS ?? '')
    .split(',')
    .map((k) => k.trim())
    .filter((k) => k.length > 0);
  if (pool.length === 0) return process.env.GEMINI_API_KEY;
  return pool[Math.floor(Math.random() * pool.length)];
}

/** Missing/empty names out of `required`, plus any `anyOf` group where NO
 * member is set (reported as `"A|B"`). Reads process.env at call time. */
export function missingEnv(required: string[], anyOf: string[][] = []): string[] {
  const unset = (k: string) => !(process.env[k] ?? '').trim();
  const missing = required.filter(unset);
  for (const group of anyOf) {
    if (group.every(unset)) missing.push(group.join('|'));
  }
  return missing;
}

/** 503 `LH_PROXY_MISCONFIG` response naming the missing vars, or null when all
 * are present (the handler proceeds). `extraHeaders` merges the route's CORS. */
export function envGuard(
  route: string,
  required: string[],
  anyOf: string[][] = [],
  extraHeaders: Record<string, string> = {},
): Response | null {
  const missing = missingEnv(required, anyOf);
  if (missing.length === 0) return null;
  console.error(`[${route}] LH_PROXY_MISCONFIG: missing env ${missing.join(', ')}`);
  return new Response(
    JSON.stringify({
      error: `proxy misconfigured: missing ${missing.join(', ')}`,
      code: 'LH_PROXY_MISCONFIG',
    }),
    { status: 503, headers: { 'content-type': 'application/json', ...extraHeaders } },
  );
}
