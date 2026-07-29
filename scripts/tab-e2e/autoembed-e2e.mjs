// LIVE auto-embed tab-E2E: the coverage gap cartridge-e2e.mjs names honestly
// — "the LIVE auto-embed at the tool-success path (chat::stream_turn →
// launch_pending_embed) needs a real model turn" — closed with the scripted
// fake model: the fake answers with a run_cartridge functionCall, the REAL
// closure tool compiles the source in-browser (no chain, no network), the
// shared auto-embed predicate fires at the tool-SUCCESS site, and the
// cartridge must end up PLAYING inline in the tool card.
//
// Run: node scripts/tab-e2e/autoembed-e2e.mjs   (zero spend)
import { readFileSync } from "node:fs";
import { join } from "node:path";
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor, REPO_ROOT } from "./lib.mjs";
import { startScriptedGemini, textTurn, functionCallTurn } from "./fake-gemini-scripted.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8799);
const FAKE_PORT = PORT + 1;
const ORIGIN = `http://teste2e.localharness.xyz:${PORT}`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("live auto-embed tab-E2E");

const FIXTURE = readFileSync(join(REPO_ROOT, "examples", "cartridges", "bouncing_ball.rl"), "utf8");

// req 1: user turn → run_cartridge call · req 2: tool-result follow-up →
// closing text · req 3: run_send auto-continue → final text.
const script = [
  functionCallTurn("run_cartridge", { source: FIXTURE }),
  textTurn("The bouncing ball is running inline above."),
  textTurn("The bouncing ball is running inline above."),
];

const server = await serve(ROOT, PORT);
const { server: fake, requests } = await startScriptedGemini(FAKE_PORT, script);
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

  await page.setRequestInterception(true);
  page.on("request", (req) => {
    const u = req.url();
    if (u.includes("streamGenerateContent")) {
      req.continue({ url: `http://127.0.0.1:${FAKE_PORT}/fake-stream` }).catch(() => {});
      return;
    }
    if (u.startsWith(ORIGIN) || u.startsWith(`http://127.0.0.1:${FAKE_PORT}`) || u.startsWith("data:")) {
      req.continue().catch(() => {});
      return;
    }
    req.abort().catch(() => {});
  });

  // Tenant-sim boot + BYOK-fake (the shared dance).
  await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  await sleep(2500);
  await page.evaluate(async () => {
    const root = await navigator.storage.getDirectory();
    const fh = await root.getFileHandle(".lh_owner", { create: true });
    const w = await fh.createWritable();
    await w.write("0x1111111111111111111111111111111111111111");
    await w.close();
    localStorage.setItem("lh_model_access", "byok");
    sessionStorage.setItem("lh_seed_pull_tried", "1");
  });
  await page.reload({ waitUntil: "domcontentloaded" });
  const ready = await waitFor(() => page.evaluate(() =>
    document.documentElement.hasAttribute("data-lh-ready") && !!document.getElementById("prompt")), 30000);
  check("boot: studio painted (tenant-sim)", !!ready);
  await waitFor(() => page.evaluate(() => {
    const s = document.getElementById("system-status")?.textContent || "";
    return s.includes("verify failed") ? s : null;
  }), 25000);
  await sleep(800);
  await page.evaluate(() => {
    sessionStorage.setItem("gemini_api_key", "AIza-throwaway-e2e-embed");
    document.getElementById("api-key-modal")?.remove();
  });

  await page.evaluate(() => {
    const ta = document.getElementById("prompt");
    ta.value = "!run the bouncing ball";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    document.getElementById("terminal-send").click();
  });

  // THE LIVE PATH: tool success → inline cartridge card → worker launch into
  // THAT card's canvas — all decided by the shared predicate at the
  // stream_turn ToolResult site, no replay seam involved.
  const card = await waitFor(() => page.evaluate(() =>
    !!document.querySelector("#transcript .tc-card-slot canvas") || null), 20000);
  check("auto-embed: tool-success painted an inline cartridge card", !!card);

  const trace = await waitFor(() => page.evaluate(() => {
    const t = globalThis.__lhEmbedTrace;
    return t && t.startsWith("launched into #tool-") ? t : null;
  }), 15000);
  check("auto-embed: cartridge launched into THIS card's canvas (trace)", !!trace, JSON.stringify(trace));

  const worker = await waitFor(() => (page.workers().length >= 1 ? true : null), 10000);
  check("auto-embed: a live Web Worker runs the cartridge", !!worker);

  // Two canvas snapshots must differ — the ball is really animating.
  const snap = (sel) => page.evaluate((s) => {
    const c = document.querySelector(s);
    return c ? c.toDataURL() : null;
  }, sel);
  const a = await snap("#transcript .tc-card-slot canvas");
  await sleep(400);
  const b = await snap("#transcript .tc-card-slot canvas");
  check("auto-embed: the embedded cartridge ANIMATES", !!a && !!b && a !== b);

  const closed = await waitFor(() => page.evaluate(() =>
    (document.getElementById("transcript")?.textContent || "").includes("running inline above") ? true : null), 20000);
  check("auto-embed: run closed on the final answer", !!closed);
  check("auto-embed: 3 model requests (call → result follow-up → continuation)",
    requests.length === 3, `requests=${requests.length}`);

  console.log("\npage errors:", pageErrors.length ? pageErrors : "(none)");
  check("no uncaught page errors", pageErrors.length === 0, pageErrors.join(" | "));
} finally {
  await browser.close();
  server.close();
  fake.close();
}
finish();
