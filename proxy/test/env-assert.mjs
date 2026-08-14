#!/usr/bin/env node
// _env.ts — the fail-LOUD env assertion helper. Proves: a missing critical var
// → a named 503 LH_PROXY_MISCONFIG; all present → null pass-through; anyOf
// groups need only one member; the route's CORS headers merge into the 503.

const { missingEnv, envGuard, geminiUpstreamKey } = await import('../.ttest/_env.js');

let failed = 0;
function ok(cond, label) {
  console.log(`${cond ? 'ok  ' : 'FAIL'} ${label}`);
  if (!cond) failed++;
}

delete process.env.LH_TEST_A;
delete process.env.LH_TEST_B;
delete process.env.LH_TEST_C;

// missing var is reported; whitespace-only counts as missing
process.env.LH_TEST_B = '  ';
ok(
  JSON.stringify(missingEnv(['LH_TEST_A', 'LH_TEST_B'])) === '["LH_TEST_A","LH_TEST_B"]',
  'missingEnv reports unset + whitespace-only vars',
);

// anyOf group: none set → reported as "A|B"; one set → satisfied
ok(
  JSON.stringify(missingEnv([], [['LH_TEST_A', 'LH_TEST_C']])) === '["LH_TEST_A|LH_TEST_C"]',
  'anyOf group with no member set is reported joined',
);
process.env.LH_TEST_C = 'x';
ok(missingEnv([], [['LH_TEST_A', 'LH_TEST_C']]).length === 0, 'anyOf satisfied by one member');

// envGuard: missing → named 503 with the var in the error + merged headers
{
  const res = envGuard('test-route', ['LH_TEST_A'], [], { 'Access-Control-Allow-Origin': 'https://x.localharness.xyz' });
  const body = await res.json();
  ok(res.status === 503, `missing var 503s (${res.status})`);
  ok(body.code === 'LH_PROXY_MISCONFIG', 'named code LH_PROXY_MISCONFIG');
  ok(body.error.includes('LH_TEST_A'), 'error names the missing var');
  ok(
    res.headers.get('Access-Control-Allow-Origin') === 'https://x.localharness.xyz',
    'extra (CORS) headers merged',
  );
}

// envGuard: all present → null (handler proceeds)
process.env.LH_TEST_A = 'set';
ok(envGuard('test-route', ['LH_TEST_A'], [['LH_TEST_C']]) === null, 'all present → null');

// geminiUpstreamKey: THE single key selector for every inference path (chat,
// scheduler, mcp). A rotation that only reaches one of them is a silent
// half-outage, which is why these three now share this function.
{
  const savedSingle = process.env.GEMINI_API_KEY;
  const savedPool = process.env.GEMINI_API_KEYS;

  delete process.env.GEMINI_API_KEYS;
  process.env.GEMINI_API_KEY = 'single-key';
  ok(geminiUpstreamKey() === 'single-key', 'unset pool falls back to GEMINI_API_KEY');

  process.env.GEMINI_API_KEYS = '  ,   ,';
  ok(geminiUpstreamKey() === 'single-key', 'whitespace/empty-only pool falls back too');

  process.env.GEMINI_API_KEYS = ' k1 , k2 ,k3';
  const picks = new Set();
  for (let i = 0; i < 200; i++) picks.add(geminiUpstreamKey());
  ok([...picks].every((k) => ['k1', 'k2', 'k3'].includes(k)), 'pool picks are trimmed members');
  ok(picks.size > 1, 'pool actually spreads across keys');

  delete process.env.GEMINI_API_KEY;
  delete process.env.GEMINI_API_KEYS;
  ok(geminiUpstreamKey() === undefined, 'nothing configured → undefined (caller 503s)');

  if (savedSingle === undefined) delete process.env.GEMINI_API_KEY;
  else process.env.GEMINI_API_KEY = savedSingle;
  if (savedPool === undefined) delete process.env.GEMINI_API_KEYS;
  else process.env.GEMINI_API_KEYS = savedPool;
}

process.exit(failed ? 1 : 0);
