// Stop-button tab-E2E against the shipped bundle, MID-STREAM. A Host::Other
// tab can never reach a streaming turn (the fail-closed pricing gate errors
// every metered send when pricing_wei is unset), so this simulates a TENANT:
// map teste2e.localharness.xyz → 127.0.0.1, pre-write the .lh_owner hint into
// that origin's OPFS (synthetic all-1s test address — local fixture only),
// and BLOCK the chain RPC so verification fails TRANSIENTLY (Failed = no
// demotion; the optimistic studio stays and pricing_wei := Some(0) local
// value). Then BYOK with a throwaway fake key + the local fake-Gemini SSE
// endpoint (one chunk, then silence) gives a genuinely LIVE mid-stream turn
// to stop. Zero real network: RPC/proxy/apex aborted, the model call never
// leaves the machine, nothing is spent.
//
//   node scripts/tab-e2e/stop-e2e.mjs [webRoot]
//
// Needs a built bundle (./scripts/build-web.sh) + puppeteer-core (README.md).
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { startFakeGemini } from "./fake-gemini.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor } from "./lib.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8792);
const FAKE_PORT = Number(process.env.LH_E2E_FAKE_PORT || 8793);
const ORIGIN = `http://teste2e.localharness.xyz:${PORT}`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("stop-button tab-E2E");

const server = await serve(ROOT, PORT);
const fake = await startFakeGemini(FAKE_PORT);
const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: "new",
  args: [
    "--no-first-run", "--disable-extensions",
    "--host-resolver-rules=MAP teste2e.localharness.xyz 127.0.0.1",
    "--unsafely-treat-insecure-origin-as-secure=" + ORIGIN,
  ],
});
try {
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (e) => pageErrors.push(String(e)));

  const stalled = [];
  await page.setRequestInterception(true);
  page.on("request", (req) => {
    const u = req.url();
    if (u.includes("streamGenerateContent")) {
      // reroute the model call to the local fake-Gemini SSE endpoint (one
      // chunk, then silence) — a REAL mid-stream state, nothing leaves the box
      stalled.push(u.slice(0, 90));
      req.continue({ url: `http://127.0.0.1:${FAKE_PORT}/fake-stream` }).catch(() => {});
      return;
    }
    if (u.startsWith(ORIGIN) || u.startsWith(`http://127.0.0.1:${FAKE_PORT}`) || u.startsWith("data:")) {
      req.continue().catch(() => {});
      return;
    }
    req.abort().catch(() => {}); // RPC / proxy / apex iframe / fonts: fail fast, nothing real leaves
  });

  // Load once (claim paint), plant the .lh_owner hint in THIS origin's OPFS,
  // then reload so mount paints the optimistic studio off the hint.
  await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  await sleep(2500);
  await page.evaluate(async () => {
    const root = await navigator.storage.getDirectory();
    const fh = await root.getFileHandle(".lh_owner", { create: true });
    const w = await fh.createWritable();
    await w.write("0x1111111111111111111111111111111111111111"); // synthetic local fixture
    await w.close();
    localStorage.setItem("lh_model_access", "byok");
    // pre-arm the seed-pull one-shot guard so the hint-without-seed mount
    // does NOT navigate the tab to apex (seed_pull::maybe_auto_kick).
    sessionStorage.setItem("lh_seed_pull_tried", "1");
  });
  await page.reload({ waitUntil: "domcontentloaded" });
  const ready = await waitFor(() => page.evaluate(() =>
    document.documentElement.hasAttribute("data-lh-ready") && !!document.getElementById("prompt")), 30000);
  check("tenant-sim: studio (full chat) painted off the .lh_owner hint", !!ready);

  // Wait for kick_verification to land Failed (RPC aborted → transient, no
  // demotion; status line paints "verify failed: …") — pricing_wei := Some(0)
  // is stashed right after, unblocking the payment gate.
  const vf = await waitFor(() => page.evaluate(() => {
    const s = document.getElementById("system-status")?.textContent || "";
    return s.includes("verify failed") ? s : null;
  }), 25000);
  check("tenant-sim: verification failed transiently (studio kept)", !!vf, JSON.stringify(vf));
  await sleep(800);
  await page.evaluate(() => {
    sessionStorage.setItem("gemini_api_key", "AIza-throwaway-e2e-stall"); // fake; request never leaves the box
    document.getElementById("api-key-modal")?.remove();
  });

  // ── the LIVE turn: '!' forces the metered path; the model call streams
  // one local fake chunk then stalls ──
  await page.evaluate(() => {
    const ta = document.getElementById("prompt");
    ta.value = "!hello, please write a long story";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    document.getElementById("terminal-send").click();
  });
  const engaged = await waitFor(() => page.evaluate(() =>
    document.getElementById("terminal-stop") ? true : null), 20000);
  check("stop: metered turn engaged (stop button up)", !!engaged);
  // the fake SSE chunk must PAINT — proof we are genuinely MID-STREAM
  const streamed = await waitFor(() => page.evaluate(() =>
    (document.getElementById("transcript")?.textContent || "").includes("Once upon a time") ? true : null), 15000);
  check("stop: first streamed chunk painted (genuine mid-stream state)", !!streamed, "model call: " + (stalled[0] || "none"));

  if (engaged && streamed) {
    const ack = await page.evaluate(async () => {
      const t0 = performance.now();
      document.getElementById("terminal-stop").click();
      const instantStatus = document.getElementById("system-status")?.textContent || "";
      let ackMs = null, how = null;
      for (;;) {
        const st = document.getElementById("system-status")?.textContent || "";
        if (st.includes("stopping")) { ackMs = performance.now() - t0; how = "status=" + JSON.stringify(st); break; }
        if (!document.getElementById("terminal-stop")) { ackMs = performance.now() - t0; how = "button-swapped"; break; }
        if (performance.now() - t0 > 2000) { how = "no ack in 2s; status=" + JSON.stringify(st); break; }
        await new Promise((r) => setTimeout(r, 15));
      }
      return { instantStatus, ackMs, how };
    });
    check("stop: first click acks within 300ms",
      ack.ackMs !== null && ack.ackMs <= 300,
      `ackMs=${ack.ackMs === null ? "none" : ack.ackMs.toFixed(1)} via ${ack.how} instant=${JSON.stringify(ack.instantStatus)}`);

    const restored = await waitFor(() => page.evaluate(() =>
      document.getElementById("terminal-send") ? true : null), 10000);
    check("stop: TURN_ACTIVE released — send button restored", !!restored);
    const tail = await page.evaluate(() =>
      (document.getElementById("transcript")?.textContent || "").slice(-260));
    check("stop: transcript reconciles with a Stopped note", /[Ss]topped/.test(tail), JSON.stringify(tail.slice(-140)));

    // second message goes through (free-routed → no network needed)
    await page.evaluate(() => {
      const ta = document.getElementById("prompt");
      ta.value = "/router status";
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      document.getElementById("terminal-send").click();
    });
    const st2 = await waitFor(() => page.evaluate(() => {
      const s = document.getElementById("system-status")?.textContent || "";
      return s.includes("intent router") ? s : null;
    }), 8000);
    check("stop: a second message sends fine after the stop", !!st2, JSON.stringify(st2));
  }

  console.log("page errors:", pageErrors.length ? pageErrors : "(none)");
} finally {
  await browser.close();
  server.close();
  fake.close();
}
finish();
