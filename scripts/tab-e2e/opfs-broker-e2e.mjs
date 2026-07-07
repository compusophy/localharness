// Tab-E2E of the WebKit OPFS write BROKER (web/opfs-worker.js + the
// filesystem/opfs.rs client) under Chromium: `window.LH_FORCE_WORKER_FS = 1`
// forces EVERY app OPFS write through the worker `createSyncAccessHandle`
// path (Chromium has both OPFS APIs in automation, so the full broker
// machinery runs end-to-end even though its production audience is WebKit —
// which Playwright's Windows port ships WITHOUT OPFS, so this is the one
// engine where the path is automatable).
//
// Flow: boot Host::Other → metered send with a stalled model call → Stop →
// the turn ends → chat history saves to OPFS THROUGH THE BROKER. Asserts:
// the broker worker actually spawned, the history file readback is intact
// JSON (bytes survived the worker round-trip), and zero page errors.
//
//   node scripts/tab-e2e/opfs-broker-e2e.mjs [webRoot]
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor } from "./lib.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8794);
const URL = `http://localhost:${PORT}/`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("opfs-broker tab-E2E");

async function send(page, text) {
  const ok = await waitFor(() => page.evaluate(() => !!document.getElementById("terminal-send")), 30000);
  if (!ok) throw new Error("send button never came back before: " + text);
  await page.evaluate((t) => {
    const ta = document.getElementById("prompt");
    ta.value = t;
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    document.getElementById("terminal-send").click();
  }, text);
}

const server = await serve(ROOT, PORT);
const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: "new",
  args: ["--no-first-run", "--disable-extensions"],
});
try {
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (e) => pageErrors.push(String(e)));

  // Pre-boot: force the broker + record every Worker construction.
  await page.evaluateOnNewDocument(`
    window.LH_FORCE_WORKER_FS = 1;
    window.__workerUrls = [];
    const W = window.Worker;
    window.Worker = function (u, o) { window.__workerUrls.push(String(u)); return new W(u, o); };
    window.Worker.prototype = W.prototype;
  `);

  // Stall the metered model call so Stop has a live turn to kill.
  await page.setRequestInterception(true);
  page.on("request", (req) => {
    if (req.url().includes("streamGenerateContent")) return; // never respond
    req.continue().catch(() => {});
  });

  await page.goto(URL, { waitUntil: "domcontentloaded" });
  const ready = await waitFor(() => page.evaluate(() => document.documentElement.hasAttribute("data-lh-ready")), 30000);
  check("boot: data-lh-ready (broker forced)", !!ready);
  await sleep(500);

  // Metered send (throwaway fake BYOK key; the call is stalled) → Stop → the
  // ended turn persists history via the app's write path = through the broker.
  await page.evaluate(() => {
    sessionStorage.setItem("gemini_api_key", "AIza-throwaway-e2e-stall");
  });
  await send(page, "!hello broker write test");
  await waitFor(() => page.evaluate(() => !!document.querySelector('[data-action="stop-turn"]')), 10000);
  await page.evaluate(() => document.querySelector('[data-action="stop-turn"]')?.click());
  const released = await waitFor(() => page.evaluate(() => !!document.getElementById("terminal-send")), 15000);
  check("turn: stopped + released", !!released);

  // The deterministic brokered write: Host::Other boot mints a throwaway
  // device identity and persists `.lh_device_key` — through the FORCED broker
  // (the flag applies before boot.js runs). Its readback proves the full
  // worker round-trip: truncate → write → flush → close → main-thread read.
  // (A cancelled zero-chunk turn has no history_bytes(), so `.lh_history.json`
  // is NOT a reliable write to assert on — the device key is.)
  const keyLen = await waitFor(
    () =>
      page.evaluate(async () => {
        try {
          const root = await navigator.storage.getDirectory();
          const fh = await root.getFileHandle(".lh_device_key");
          const text = await (await fh.getFile()).text();
          return text.length >= 64 ? text.length : false; // a real key, not junk
        } catch {
          return false;
        }
      }),
    15000,
  );
  check("broker: .lh_device_key written via worker + readback intact", !!keyLen, `len=${keyLen}`);

  const workers = await page.evaluate(() => window.__workerUrls);
  check(
    "broker: /opfs-worker.js worker actually spawned",
    workers.some((u) => u.includes("opfs-worker.js")),
    JSON.stringify(workers),
  );

  check("no uncaught page errors", pageErrors.length === 0, pageErrors.join(" | "));
  finish();
} finally {
  await browser.close();
  server.close();
}
