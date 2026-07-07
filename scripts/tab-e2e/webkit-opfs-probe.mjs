// WebKit OPFS capability probe (Playwright WebKit). Answers, on THIS host's
// Playwright-WebKit build: does the page have navigator.storage/getDirectory,
// does the main-thread createWritable path work, and does the worker
// createSyncAccessHandle path (the iOS write-broker route) work? Each step is
// timeout-bounded so an engine stall reports instead of hanging (the exact
// failure mode that gated iOS off in June).
//
// Ground truth (established 2026-07-07, current-web-verified): Playwright's
// bundled WebKit CANNOT gate the OPFS broker on ANY platform — Windows ships
// no navigator.storage at all (observed with this probe), and on macOS/Linux
// getDirectory() fails because WebKit disables OPFS on the ephemeral contexts
// Playwright launches (playwright#18235 open since 2022; cypress#30270;
// launchPersistentContext broken on WebKit, playwright#5642). The REAL WebKit
// gate is the iOS Simulator on a macOS runner —
// .github/workflows/ios-webkit-e2e.yml. This probe stays as the local
// diagnostic that documents/re-checks the refutation.
//
//   node scripts/tab-e2e/webkit-opfs-probe.mjs [url]
//
// Needs `playwright` + its webkit browser (npx playwright install webkit).
// Exit code 0 = probe RAN (results printed); non-zero = probe itself broke.
import { webkit } from 'playwright';

const URL = process.argv[2] || 'https://localharness.xyz/skill.md'; // real https origin — OPFS needs a secure context

const browser = await webkit.launch();
const page = await (await browser.newContext()).newPage();
await page.goto(URL, { waitUntil: 'domcontentloaded' });
console.log('probe target:', URL);
console.log('UA:', await page.evaluate(() => navigator.userAgent));
console.log('vendor:', await page.evaluate(() => navigator.vendor));

const surface = await page.evaluate(() => ({
  storage: typeof navigator.storage,
  getDirectory: navigator.storage ? typeof navigator.storage.getDirectory : 'n/a',
  persist: navigator.storage ? typeof navigator.storage.persist : 'n/a',
  FileSystemFileHandle: typeof FileSystemFileHandle,
  Worker: typeof Worker,
}));
console.log('surface:', JSON.stringify(surface));

if (surface.getDirectory !== 'function') {
  console.log('VERDICT: NO-OPFS — this Playwright-WebKit build has no navigator.storage.getDirectory');
  await browser.close();
  process.exit(0);
}

// Main-thread createWritable path (what non-broker engines use).
const main = await page.evaluate(async () => {
  const T = (ms, p, tag) =>
    Promise.race([
      p.then(() => ({ tag, ok: true })).catch((e) => ({ tag, ok: false, err: String(e) })),
      new Promise((r) => setTimeout(() => r({ tag, ok: false, err: `TIMEOUT ${ms}ms` }), ms)),
    ]);
  const out = [];
  const root = await navigator.storage.getDirectory();
  const fh = await root.getFileHandle('lh_probe_main.txt', { create: true });
  out.push({ tag: 'createWritable exists', ok: typeof fh.createWritable === 'function' });
  if (typeof fh.createWritable !== 'function') return out;
  out.push(await T(5000, fh.createWritable(), 'createWritable()'));
  if (!out[1].ok) return out;
  const w = await fh.createWritable();
  out.push(await T(5000, w.write(new TextEncoder().encode('probe-data')), 'write()'));
  out.push(await T(5000, w.close(), 'close()'));
  const f = await fh.getFile();
  out.push({ tag: 'readback', ok: (await f.text()) === 'probe-data' });
  return out;
});
console.log('MAIN-THREAD createWritable path:');
main.forEach((s) => console.log(`  ${s.ok ? 'PASS' : 'FAIL'} ${s.tag}${s.err ? ' — ' + s.err : ''}`));

// Worker createSyncAccessHandle path (the iOS write-broker route).
const worker = await page.evaluate(async () => {
  const src = `self.onmessage = async () => {
    try {
      const root = await navigator.storage.getDirectory();
      const fh = await root.getFileHandle('lh_probe_worker.txt', { create: true });
      if (typeof fh.createSyncAccessHandle !== 'function') { self.postMessage({ ok: false, err: 'no createSyncAccessHandle' }); return; }
      const h = await fh.createSyncAccessHandle();
      try { h.truncate(0); h.write(new TextEncoder().encode('worker-data'), { at: 0 }); h.flush(); } finally { h.close(); }
      const f = await fh.getFile();
      self.postMessage({ ok: (await f.text()) === 'worker-data' });
    } catch (err) { self.postMessage({ ok: false, err: String(err) }); }
  };`;
  const w = new Worker(URL.createObjectURL(new Blob([src], { type: 'text/javascript' })));
  return await Promise.race([
    new Promise((r) => { w.onmessage = (e) => r(e.data); w.postMessage(1); }),
    new Promise((r) => setTimeout(() => r({ ok: false, err: 'TIMEOUT 8000ms' }), 8000)),
  ]);
});
console.log('WORKER createSyncAccessHandle path:', worker.ok ? 'PASS (write+flush+readback)' : `FAIL — ${worker.err}`);
console.log(
  'VERDICT:',
  worker.ok ? 'BROKER-TESTABLE — this build can gate the OPFS broker' : 'broker path NOT testable on this build',
);
await browser.close();
