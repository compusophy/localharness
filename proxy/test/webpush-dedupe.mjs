#!/usr/bin/env node
// Unit test for _webpush.ts parsePushSubs + dedupeSubs (R5: per-device dedup).
// Proves two different-endpoint subs sharing one `dev` collapse to ONE delivery,
// while distinct devices and legacy (no-dev) subs are preserved.

import webpushMod from '../.ttest/_webpush.js';
const { parsePushSubs, dedupeSubs } = webpushMod;

let failed = false;
function ok(name, cond, detail = '') {
  if (cond) console.log(`ok   ${name}`);
  else {
    console.error(`FAIL ${name} ${detail}`);
    failed = true;
  }
}

const sub = (ep, dev) => ({
  endpoint: 'https://push.example/' + ep,
  keys: { p256dh: 'k', auth: 'a' },
  ...(dev ? { dev } : {}),
});

// --- parsePushSubs preserves `dev` -------------------------------------------
{
  const parsed = parsePushSubs(JSON.stringify([sub('a', 'device-1')]));
  ok('parse: preserves dev', parsed.length === 1 && parsed[0].dev === 'device-1', JSON.stringify(parsed));
}
{
  const parsed = parsePushSubs(JSON.stringify([sub('a')]));
  ok('parse: legacy sub has no dev', parsed.length === 1 && parsed[0].dev === undefined, JSON.stringify(parsed));
}

// --- two different-endpoint subs, SAME dev -> ONE delivery (R5) --------------
{
  const subs = [sub('origin-a', 'device-1'), sub('origin-b', 'device-1')];
  const deduped = dedupeSubs(subs);
  ok(
    'dedupe: same dev, two endpoints -> one send',
    deduped.length === 1 && deduped[0].endpoint === 'https://push.example/origin-a',
    JSON.stringify(deduped),
  );
}

// --- distinct devices coexist -----------------------------------------------
{
  const deduped = dedupeSubs([sub('a', 'dev-phone'), sub('b', 'dev-desktop')]);
  ok('dedupe: distinct devs preserved', deduped.length === 2, JSON.stringify(deduped));
}

// --- legacy subs (no dev) still endpoint-deduped -----------------------------
{
  const deduped = dedupeSubs([sub('a'), sub('a'), sub('b')]);
  ok('dedupe: legacy endpoint dedup', deduped.length === 2, JSON.stringify(deduped));
}

// --- mixed: a dev-tagged + a legacy with the same endpoint collapse ----------
{
  const deduped = dedupeSubs([sub('a', 'dev-1'), sub('a')]);
  ok('dedupe: same endpoint across dev/legacy collapses', deduped.length === 1, JSON.stringify(deduped));
}

if (failed) {
  console.error('\nwebpush-dedupe test FAILED');
  process.exit(1);
}
console.log('\nall webpush-dedupe cases pass');
