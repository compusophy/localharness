#!/usr/bin/env node
// _health.ts — the hourly self-check decision logic (pure parts) + the deduped
// alert path over a stubbed GitHub. Proves: float headroom warns at 10x the
// breaker floor (not at death), the dedupe signature is order-free, one
// incident = one issue (open-issue hit → no create), no token → issue not
// filed (console is the alert), and the owner push is best-effort + optional.

const { floatHealth, envHealth, healthSignature, alertHealth } = await import(
  '../.ttest/_health.js'
);

let failed = 0;
function ok(cond, label) {
  console.log(`${cond ? 'ok  ' : 'FAIL'} ${label}`);
  if (!cond) failed++;
}

// ---- floatHealth (pure) ------------------------------------------------------
ok(!floatHealth(499_999n, 50_000n).ok, 'float below 10x breaker floor warns');
ok(floatHealth(500_000n, 50_000n).ok, 'float at 10x breaker floor passes');
ok(floatHealth(0n, 0n).ok, 'breaker disabled (minFloat 0) → nothing to warn on');
ok(
  floatHealth(1n, 50_000n).detail.includes('top up'),
  'failing float detail says what to do',
);

// ---- envHealth ---------------------------------------------------------------
delete process.env.LH_HTEST_X;
ok(!envHealth(['LH_HTEST_X']).ok, 'envHealth flags a missing var');
ok(envHealth(['LH_HTEST_X']).detail.includes('LH_HTEST_X'), 'detail names it');
process.env.LH_HTEST_X = '1';
ok(envHealth(['LH_HTEST_X']).ok, 'envHealth passes when present');

// ---- healthSignature (stable + order-free) ------------------------------------
const a = { name: 'env', ok: false, detail: '' };
const b = { name: 'sponsor-float', ok: false, detail: '' };
ok(healthSignature([b, a]) === healthSignature([a, b]), 'signature is order-free');
ok(
  healthSignature([a, b]) === 'proxy-health-env-sponsor-float',
  `signature is the sorted name join (${healthSignature([a, b])})`,
);

// ---- alertHealth over a stubbed GitHub -----------------------------------------
let searches = 0;
let creates = [];
let searchHit = false;
globalThis.fetch = async (url, init) => {
  if (String(url).includes('/search/issues')) {
    searches++;
    const items = searchHit
      ? [{ number: 7, html_url: 'https://x/7', title: `[health] … (${healthSignature([a, b])})` }]
      : [];
    return new Response(JSON.stringify({ items }), { status: 200 });
  }
  creates.push(JSON.parse(init.body));
  return new Response(JSON.stringify({ html_url: 'https://x/1', number: 1 }), { status: 201 });
};

// first incident → files ONE issue carrying the signature + the health label
{
  const pushes = [];
  const r = await alertHealth([a, b], {
    repo: 'o/telemetry',
    token: 't',
    pushOwner: '0xabc',
    push: async (owner, title, body) => (pushes.push({ owner, title, body }), true),
  });
  ok(r.filed === true && !r.deduped, `first incident files (${JSON.stringify(r)})`);
  ok(creates.length === 1, 'exactly one create POST');
  ok(creates[0].title.includes(`(${healthSignature([a, b])})`), 'title carries the signature');
  ok(JSON.stringify(creates[0].labels) === '["health"]', 'labeled health');
  ok(pushes.length === 1 && pushes[0].owner === '0xabc', 'owner push sent when mapped');
}

// same failing set again → deduped against the open issue, NO second create
{
  searchHit = true;
  creates = [];
  const r = await alertHealth([b, a], { repo: 'o/telemetry', token: 't' });
  ok(r.filed === true && r.deduped === true, 'repeat incident dedupes to the open issue');
  ok(creates.length === 0, 'no duplicate create');
}

// no token → issue not filed (console is the alert), no GitHub call, no throw
{
  searches = 0;
  creates = [];
  const r = await alertHealth([a], { repo: 'o/telemetry', token: '' });
  ok(r.filed === false && searches === 0 && creates.length === 0, 'no token → no GitHub call');
}

// a throwing push is swallowed (best-effort)
{
  searchHit = false;
  const r = await alertHealth([a], {
    repo: 'o/telemetry',
    token: 't',
    pushOwner: '0xabc',
    push: async () => {
      throw new Error('push service down');
    },
  });
  ok(r.filed === true, 'push failure never blocks the issue');
}

process.exit(failed ? 1 : 0);
