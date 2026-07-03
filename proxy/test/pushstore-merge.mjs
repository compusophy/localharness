#!/usr/bin/env node
// Unit test for _pushstore.ts mergeSubIntoList — the OFF-CHAIN push-sub store's
// pure upsert (mirrors the retired src/registry/push.rs::merge_push_sub
// semantics: upsert by stable `dev` id, else endpoint; newest first; idempotent
// no-write when the exact sub is already stored; capped device list).

import pushstoreMod from '../.ttest/_pushstore.js';
const { mergeSubIntoList, MAX_DEVICE_SUBS } = pushstoreMod;

let failed = false;
function ok(name, cond, detail = '') {
  if (cond) console.log(`ok   ${name}`);
  else {
    console.error(`FAIL ${name} ${detail}`);
    failed = true;
  }
}

const sub = (ep, dev, key = 'k') => ({
  endpoint: 'https://push.example/' + ep,
  keys: { p256dh: key, auth: 'a' },
  ...(dev ? { dev } : {}),
});

// --- exact sub already stored -> null (no write; idempotent per-load enroll) --
{
  const list = [sub('phone', 'dev-1')];
  ok('idempotent: exact match -> null', mergeSubIntoList(list, sub('phone', 'dev-1')) === null);
}

// --- same dev, new endpoint -> replaces (R5: one entry per physical device) --
{
  const merged = mergeSubIntoList([sub('origin-a', 'dev-1')], sub('origin-b', 'dev-1'));
  ok(
    'same dev collapses to one entry (newest)',
    merged.length === 1 && merged[0].endpoint === 'https://push.example/origin-b',
    JSON.stringify(merged),
  );
}

// --- same endpoint, churned keys -> replaces (reinstall/cleared site data) ---
{
  const merged = mergeSubIntoList([sub('phone', undefined, 'OLD')], sub('phone', undefined, 'NEW'));
  ok(
    'same endpoint replaced with new keys',
    merged.length === 1 && merged[0].keys.p256dh === 'NEW',
    JSON.stringify(merged),
  );
}

// --- distinct devices coexist, newest first ----------------------------------
{
  const merged = mergeSubIntoList([sub('desktop', 'dev-desktop')], sub('phone', 'dev-phone'));
  ok(
    'distinct devs coexist newest-first',
    merged.length === 2 && merged[0].dev === 'dev-phone' && merged[1].dev === 'dev-desktop',
    JSON.stringify(merged),
  );
}

// --- legacy (no-dev) entry with the same endpoint replaced by dev-tagged -----
{
  const merged = mergeSubIntoList([sub('phone', undefined, 'OLD')], sub('phone', 'dev-1', 'NEW'));
  ok(
    'dev-tagged sub replaces legacy same-endpoint entry',
    merged.length === 1 && merged[0].keys.p256dh === 'NEW' && merged[0].dev === 'dev-1',
    JSON.stringify(merged),
  );
}

// --- cap: oldest evicted, newest always kept ---------------------------------
{
  let list = [];
  for (let i = 0; i < MAX_DEVICE_SUBS + 4; i++) {
    const m = mergeSubIntoList(list, sub(`d${i}`, `dev-${i}`));
    if (m) list = m;
  }
  ok(
    `cap holds at ${MAX_DEVICE_SUBS}, newest survives`,
    list.length === MAX_DEVICE_SUBS && list[0].dev === `dev-${MAX_DEVICE_SUBS + 3}`,
    JSON.stringify(list.map((s) => s.dev)),
  );
}

if (failed) {
  console.error('\npushstore-merge test FAILED');
  process.exit(1);
}
console.log('\nall pushstore-merge cases pass');
