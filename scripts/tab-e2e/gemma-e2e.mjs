// GEMMA WEBGPU PROOF driver — serves the browser-app-local bundle, drives
// admin cog -> download local model -> select gemma-3-270m -> send a prompt,
// and captures the generated text. NOT part of the checked-in suite.
//   node scripts/tab-e2e/gemma-e2e.mjs [webRoot] [--headful]
import puppeteer from "puppeteer-core";
import { appendFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, sleep, waitFor } from "./lib.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8799);
const PAGE_URL = `http://localhost:${PORT}/`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const FORCE_HEADFUL = process.argv.includes("--headful");
const LOG = join(dirname(fileURLToPath(import.meta.url)), "gemma-e2e-console.log");
writeFileSync(LOG, "");
const clog = (line) => { try { appendFileSync(LOG, line + "\n"); } catch {} };
const say = (line) => console.log(`[${new Date().toISOString().slice(11, 19)}] ${line}`);

const GPU_ARGS = ["--no-first-run", "--disable-extensions", "--enable-unsafe-webgpu", "--enable-features=Vulkan"];
// Persistent profile so OPFS (the 550MB weights) survives a killed run.
const PROFILE = process.env.LH_E2E_PROFILE || "C:/Users/kyle/AppData/Local/Temp/claude/gemma-e2e-profile";

async function probeGpu(headless) {
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: headless ? "new" : false,
    args: GPU_ARGS,
    userDataDir: PROFILE,
    protocolTimeout: 900_000,
  });
  const page = await browser.newPage();
  // Probe on the localhost origin: navigator.gpu is [SecureContext]-gated and
  // about:blank's opaque origin does not qualify.
  await page.goto(PAGE_URL + "llms.txt", { waitUntil: "domcontentloaded" });
  const probe = await page.evaluate(async () => {
    if (!("gpu" in navigator)) return { gpu: false, adapter: false, secure: window.isSecureContext };
    try {
      const a = await navigator.gpu.requestAdapter();
      return { gpu: true, adapter: !!a, secure: window.isSecureContext, info: a ? JSON.stringify({ f: [...a.features].slice(0, 4) }) : "" };
    } catch (e) {
      return { gpu: true, adapter: false, secure: window.isSecureContext, err: String(e) };
    }
  });
  return { browser, page, probe };
}

// ── phase 1: WebGPU probe (headless first, headful fallback) ────────────────
const server = await serve(ROOT, PORT);
let mode = FORCE_HEADFUL ? "headful" : "headless";
let { browser, page, probe } = await probeGpu(mode === "headless");
say(`probe (${mode}): navigator.gpu=${probe.gpu} adapter=${probe.adapter} secure=${probe.secure} ${probe.err || ""}`);
if (!probe.adapter && mode === "headless") {
  await browser.close();
  mode = "headful";
  ({ browser, page, probe } = await probeGpu(false));
  say(`probe (headful): navigator.gpu=${probe.gpu} adapter=${probe.adapter} ${probe.err || ""}`);
}
if (!probe.adapter) {
  say("FATAL: no WebGPU adapter in either mode");
  await browser.close();
  process.exit(3);
}

// ── phase 2: load the app ───────────────────────────────────────────────────
page.on("console", (m) => {
  const t = m.text();
  clog(`console.${m.type()}: ${t}`);
  // Live progress: the local backend's per-token lines + any wasm panic.
  if (t.includes("[lh-local]")) say(t);
  if (t.includes("panicked at")) say(`PANIC: ${t.split("\n")[0]}`);
});
page.on("pageerror", (e) => { clog(`pageerror: ${e.message}`); say(`pageerror: ${e.message}`); });
await page.goto(PAGE_URL, { waitUntil: "domcontentloaded" });
const prompted = await waitFor(() => page.evaluate(() => !!document.getElementById("prompt")), 30_000, 250);
if (!prompted) { say("FATAL: #prompt never appeared (app failed to mount)"); process.exit(4); }
say("app mounted (#prompt present)");

// ── phase 3: open admin, start the model download ───────────────────────────
// Idempotent: if a previous (killed) run already landed the files in OPFS,
// skip the download entirely.
const already = await page.evaluate(async () => {
  try {
    const root = await navigator.storage.getDirectory();
    const fh = await root.getFileHandle(".lh_local_model.safetensors");
    const f = await fh.getFile();
    const th = await root.getFileHandle(".lh_local_tokenizer.json");
    const t = await th.getFile();
    return { w: f.size, t: t.size };
  } catch { return null; }
});
if (already && already.w > 100_000_000 && already.t > 1_000_000) {
  say(`weights already in OPFS (${(already.w / 1e6).toFixed(0)} MB) — skipping download`);
}
await page.evaluate(() => document.querySelector('[data-action="header-admin-toggle"]')?.click());
const dlBtn = await waitFor(
  () => page.evaluate(() => !!document.querySelector('[data-action="download-local-model"]')),
  10_000, 200,
);
if (!dlBtn) { say("FATAL: download-local-model button not found (bundle lacks `local`?)"); process.exit(5); }
const skipDownload = already && already.w > 100_000_000 && already.t > 1_000_000;
if (!skipDownload) {
  await page.evaluate(() => document.querySelector('[data-action="download-local-model"]').click());
  say("download clicked");
}

// Poll #local-model-msg; abort only after 10 min of ZERO progress.
let lastMsg = "", lastChange = Date.now(), ready = skipDownload;
for (; !ready;) {
  const msg = await page.evaluate(() => document.getElementById("local-model-msg")?.textContent || "");
  if (msg !== lastMsg) {
    lastMsg = msg;
    lastChange = Date.now();
    if (/\((\d0|100|\d?[05])%\)|ready|error|HTTP|save|fetch/i.test(msg)) say(`dl: ${msg}`);
  }
  if (/ready/i.test(msg)) { ready = true; break; }
  if (/error|HTTP \d|failed/i.test(msg) && !/downloading/i.test(msg)) { say(`dl FAILED: ${msg}`); break; }
  if (Date.now() - lastChange > 600_000) { say(`dl STALLED 10min at: ${msg}`); break; }
  await sleep(2000);
}
if (!ready) { await browser.close(); server.close(); process.exit(6); }
say(skipDownload ? "download skipped (cached)" : `download complete: ${lastMsg}`);

// ── phase 4: select the local model ──────────────────────────────────────────
await page.evaluate(() => document.querySelector('#model-selector-row button[data-arg="gemma-3-270m"]')?.click());
const modelMsg = await waitFor(
  () => page.evaluate(() => document.getElementById("model-msg")?.textContent || null),
  10_000, 200,
);
say(`model selected: ${modelMsg}`);
// close the admin sheet (second cog tap)
await page.evaluate(() => document.querySelector('[data-action="header-admin-toggle"]')?.click());

// ── phase 5: send the prompt, wait for generated text ───────────────────────
const PROMPT = process.env.LH_E2E_PROMPT || "The capital of France is";
// Stale history from earlier proof runs seeds the local session's prompt
// (render_prompt replays every turn) — clear it, then reload so the app
// starts this session with an empty transcript.
await page.evaluate(async () => {
  try {
    const root = await navigator.storage.getDirectory();
    await root.removeEntry(".lh_history.json");
  } catch {}
});
await page.goto(PAGE_URL, { waitUntil: "domcontentloaded" });
await waitFor(() => page.evaluate(() => !!document.getElementById("prompt")), 30_000, 250);
await page.evaluate((p) => {
  const ta = document.getElementById("prompt");
  ta.value = p;
  ta.dispatchEvent(new Event("input", { bubbles: true }));
}, PROMPT);
await page.evaluate(() => document.getElementById("terminal-send")?.click());
say("prompt sent — waiting for the local session boot (weights -> GPU) + tokens");

// Turn is over when #terminal-send is back (stop button swaps in during a run).
// Model load can take minutes; budget stays under the caller's 10-min ceiling
// so this process always exits itself and prints what it has.
let lastLen = 0;
const t0 = Date.now();
let done = false;
while (Date.now() - t0 < Number(process.env.LH_E2E_GEN_BUDGET_MS || 450_000)) {
  const s = await page.evaluate(() => ({
    sendBack: !!document.getElementById("terminal-send"),
    stopUp: !!document.getElementById("terminal-stop"),
    text: document.getElementById("transcript")?.innerText || "",
    status: document.getElementById("system-status")?.textContent || "",
  }));
  if (s.text.length > lastLen) {
    lastLen = s.text.length;
    say(`transcript ${s.text.length} chars; status="${s.status.slice(0, 90)}"`);
  }
  if (s.sendBack && !s.stopUp && Date.now() - t0 > 5000) { done = true; break; }
  await sleep(3000);
}
const finalText = await page.evaluate(() => document.getElementById("transcript")?.innerText || "");
const finalStatus = await page.evaluate(() => document.getElementById("system-status")?.textContent || "");
say(`turn ${done ? "completed" : "TIMED OUT"}; status="${finalStatus}"`);
console.log("──── TRANSCRIPT ────");
console.log(finalText);
console.log("────────────────────");
await browser.close();
server.close();
process.exit(done ? 0 : 7);
