// GEMMA STREAMING PROOF driver — same flow as gemma-e2e.mjs (serve bundle,
// persistent profile, download weights, select gemma-3-270m) plus three
// streaming-era assertions: (A) transcript grows INCREMENTALLY during a turn,
// (B) tok/s from the [lh-local] console lines, (C) Stop mid-generation halts
// promptly and ends the turn cleanly. NOT part of the checked-in suite.
//   node scripts/tab-e2e/gemma-stream-e2e.mjs [webRoot]
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
const LOG = join(dirname(fileURLToPath(import.meta.url)), "gemma-stream-e2e-console.log");
writeFileSync(LOG, "");
const clog = (line) => { try { appendFileSync(LOG, line + "\n"); } catch {} };
const say = (line) => console.log(`[${new Date().toISOString().slice(11, 19)}] ${line}`);

const GPU_ARGS = ["--no-first-run", "--disable-extensions", "--enable-unsafe-webgpu", "--enable-features=Vulkan"];
const PROFILE = process.env.LH_E2E_PROFILE || "C:/Users/kyle/AppData/Local/Temp/claude/gemma-e2e-profile";

// [lh-local] token telemetry, per generation. A "generate:" line opens a gen.
const gens = [];
function onLocalLine(t) {
  const g = t.match(/\[lh-local\] generate: prompt=(\d+) tokens, max_new=(\d+)/);
  if (g) { gens.push({ prompt: +g[1], maxNew: +g[2], toks: [] }); return; }
  const m = t.match(/\[lh-local\] tok (\d+)\/(\d+) \(([\d.]+) tok\/s\)/);
  if (m && gens.length) gens[gens.length - 1].toks.push({ t: Date.now(), n: +m[1], tps: +m[3] });
}

async function probeGpu(headless) {
  const browser = await puppeteer.launch({
    executablePath: CHROME,
    headless: headless ? "new" : false,
    args: GPU_ARGS,
    userDataDir: PROFILE,
    protocolTimeout: 900_000,
  });
  const page = await browser.newPage();
  await page.goto(PAGE_URL + "llms.txt", { waitUntil: "domcontentloaded" });
  const probe = await page.evaluate(async () => {
    if (!("gpu" in navigator)) return { gpu: false, adapter: false, secure: window.isSecureContext };
    try {
      const a = await navigator.gpu.requestAdapter();
      return { gpu: true, adapter: !!a, secure: window.isSecureContext };
    } catch (e) {
      return { gpu: true, adapter: false, secure: window.isSecureContext, err: String(e) };
    }
  });
  return { browser, page, probe };
}

// ── phase 1: WebGPU probe ────────────────────────────────────────────────────
const server = await serve(ROOT, PORT);
let mode = "headless";
let { browser, page, probe } = await probeGpu(true);
say(`probe (${mode}): navigator.gpu=${probe.gpu} adapter=${probe.adapter} secure=${probe.secure} ${probe.err || ""}`);
if (!probe.adapter) {
  await browser.close();
  mode = "headful";
  ({ browser, page, probe } = await probeGpu(false));
  say(`probe (headful): navigator.gpu=${probe.gpu} adapter=${probe.adapter} ${probe.err || ""}`);
}
if (!probe.adapter) { say("FATAL: no WebGPU adapter in either mode"); await browser.close(); process.exit(3); }

// ── phase 2: load the app ────────────────────────────────────────────────────
page.on("console", (m) => {
  const t = m.text();
  clog(`console.${m.type()}: ${t}`);
  if (t.includes("[lh-local]")) { onLocalLine(t); if (!/ tok \d/.test(t) || /tok (\d*[05])\//.test(t)) say(t); }
  if (t.includes("panicked at")) say(`PANIC: ${t.split("\n")[0]}`);
});
page.on("pageerror", (e) => { clog(`pageerror: ${e.message}`); say(`pageerror: ${e.message}`); });
await page.goto(PAGE_URL, { waitUntil: "domcontentloaded" });
if (!(await waitFor(() => page.evaluate(() => !!document.getElementById("prompt")), 30_000, 250))) {
  say("FATAL: #prompt never appeared"); process.exit(4);
}
say("app mounted");

// ── phase 3: model download (idempotent) ─────────────────────────────────────
const already = await page.evaluate(async () => {
  try {
    const root = await navigator.storage.getDirectory();
    const f = await (await root.getFileHandle(".lh_local_model.safetensors")).getFile();
    const t = await (await root.getFileHandle(".lh_local_tokenizer.json")).getFile();
    return { w: f.size, t: t.size };
  } catch { return null; }
});
const skipDownload = already && already.w > 100_000_000 && already.t > 1_000_000;
if (skipDownload) say(`weights already in OPFS (${(already.w / 1e6).toFixed(0)} MB) — skipping download`);
await page.evaluate(() => document.querySelector('[data-action="header-admin-toggle"]')?.click());
const dlBtn = await waitFor(
  () => page.evaluate(() => !!document.querySelector('[data-action="download-local-model"]')),
  10_000, 200,
);
if (!dlBtn) { say("FATAL: download-local-model button not found (bundle lacks `local`?)"); process.exit(5); }
if (!skipDownload) {
  await page.evaluate(() => document.querySelector('[data-action="download-local-model"]').click());
  say("download clicked");
}
let lastMsg = "", lastChange = Date.now(), ready = skipDownload;
const tDl = Date.now();
for (; !ready;) {
  const msg = await page.evaluate(() => document.getElementById("local-model-msg")?.textContent || "");
  if (msg !== lastMsg) {
    lastMsg = msg; lastChange = Date.now();
    if (/\((\d0|100|\d?[05])%\)|ready|error|HTTP|save|fetch/i.test(msg)) say(`dl: ${msg}`);
  }
  if (/ready/i.test(msg)) { ready = true; break; }
  if (/error|HTTP \d|failed/i.test(msg) && !/downloading/i.test(msg)) { say(`dl FAILED: ${msg}`); break; }
  if (Date.now() - lastChange > 300_000) { say(`dl STALLED at: ${msg}`); break; }
  await sleep(2000);
}
if (!ready) { await browser.close(); server.close(); process.exit(6); }
say(skipDownload ? "download skipped (cached)" : `download complete in ${((Date.now() - tDl) / 1000).toFixed(0)}s: ${lastMsg}`);

// ── phase 4: select local model ──────────────────────────────────────────────
await page.evaluate(() => document.querySelector('#model-selector-row button[data-arg="gemma-3-270m"]')?.click());
const modelMsg = await waitFor(() => page.evaluate(() => document.getElementById("model-msg")?.textContent || null), 10_000, 200);
say(`model selected: ${modelMsg}`);
await page.evaluate(() => document.querySelector('[data-action="header-admin-toggle"]')?.click());

// clean transcript: stale history seeds the local prompt otherwise
await page.evaluate(async () => {
  try { await (await navigator.storage.getDirectory()).removeEntry(".lh_history.json"); } catch {}
});
await page.goto(PAGE_URL, { waitUntil: "domcontentloaded" });
await waitFor(() => page.evaluate(() => !!document.getElementById("prompt")), 30_000, 250);

const sendPrompt = async (p) => {
  await page.evaluate((v) => {
    const ta = document.getElementById("prompt");
    ta.value = v;
    ta.dispatchEvent(new Event("input", { bubbles: true }));
  }, p);
  await page.evaluate(() => document.getElementById("terminal-send")?.click());
};
const snap = async () => page.evaluate(() => ({
  sendBack: !!document.getElementById("terminal-send"),
  stopUp: !!document.getElementById("terminal-stop"),
  len: (document.getElementById("transcript")?.innerText || "").length,
}));

// ── phase 5: PROMPT 1 — incremental paint + throughput ──────────────────────
const P1 = "Write a long detailed story about a robot exploring a vast ocean world.";
await sendPrompt(P1);
say(`prompt 1 sent (${JSON.stringify(P1)}) — waiting for session boot + tokens`);
const samples = [];   // {t, len} taken while the stop button is up
const t0 = Date.now();
let done1 = false;
while (Date.now() - t0 < Number(process.env.LH_E2E_GEN_BUDGET_MS || 300_000)) {
  const s = await snap();
  if (s.stopUp) samples.push({ t: Date.now() - t0, len: s.len });
  if (s.sendBack && !s.stopUp && Date.now() - t0 > 5000) { done1 = true; break; }
  await sleep(400);
}
const reply1 = await page.evaluate(() => document.getElementById("transcript")?.innerText || "");
say(`turn 1 ${done1 ? "completed" : "TIMED OUT"} after ${((Date.now() - t0) / 1000).toFixed(0)}s`);

// (A) incremental paint: distinct GROWING lengths across in-turn samples
const lens = samples.map((s) => s.len);
const growth = [...new Set(lens)].length;
let increasing = 0;
for (let i = 1; i < lens.length; i++) if (lens[i] > lens[i - 1]) increasing++;
const passA = done1 && growth >= 3 && increasing >= 2;
say(`A incremental-paint: ${passA ? "PASS" : "FAIL"} — ${samples.length} in-turn samples, ${growth} distinct lengths, ${increasing} growth steps`);
clog(`samples: ${JSON.stringify(samples)}`);

// (B) throughput from [lh-local] tok lines (gen 1): cumulative + steady-state
// marginal rate over the back half (excludes first-token/pipeline warmup).
const g1 = gens[0];
const tpsFinal1 = g1 && g1.toks.length ? g1.toks[g1.toks.length - 1].tps : null;
let marginal1 = null;
if (g1 && g1.toks.length >= 8) {
  const a = g1.toks[Math.floor(g1.toks.length / 2)], b = g1.toks[g1.toks.length - 1];
  if (b.t > a.t) marginal1 = ((b.n - a.n) / ((b.t - a.t) / 1000)).toFixed(2);
}
const passB = tpsFinal1 !== null;
say(`B throughput: gen1 prompt=${g1?.prompt} tokens, ${g1?.toks.length ?? 0} generated, cumulative ${tpsFinal1} tok/s, steady-state (back half) ${marginal1 ?? "n/a"} tok/s`);

// ── phase 6: PROMPT 2 — Stop mid-generation ──────────────────────────────────
const P2 = "Tell me everything you know about the history of Paris, in great detail.";
await sendPrompt(P2);
say("prompt 2 sent — waiting for streaming to start");
const lenBefore2 = (await snap()).len;
const streaming = await waitFor(async () => {
  const s = await snap();
  return s.stopUp && s.len > lenBefore2 + 20 && gens.length >= 2 && gens[1].toks.length >= 3 ? s : null;
}, 240_000, 300);
let passC = false, haltMs = null, tailToks = null, lenFrozen = null;
if (!streaming) {
  say("C stop: FAIL — second generation never started streaming");
} else {
  const toksAtStop = gens[1].toks.length;
  const tStop = Date.now();
  await page.evaluate(() => document.getElementById("terminal-stop")?.click());
  say(`stop clicked at ${toksAtStop} tokens into gen 2`);
  const ended = await waitFor(async () => {
    const s = await snap();
    return s.sendBack && !s.stopUp ? s : null;
  }, 20_000, 250);
  haltMs = Date.now() - tStop;
  if (!ended) {
    say(`C stop: FAIL — send button not back ${haltMs}ms after stop`);
  } else {
    const lenAtEnd = ended.len;
    await sleep(4000);
    const after = await snap();
    tailToks = gens[1].toks.filter((k) => k.t > tStop + 2000).length;
    lenFrozen = after.len === lenAtEnd && after.sendBack;
    passC = haltMs < 15_000 && tailToks === 0 && lenFrozen;
    say(`C stop: ${passC ? "PASS" : "FAIL"} — turn ended ${haltMs}ms after click; tokens >2s after stop: ${tailToks}; transcript frozen+idle 4s later: ${lenFrozen}`);
  }
}

// ── report ───────────────────────────────────────────────────────────────────
console.log("──── RESULTS ────");
console.log(`A incremental paint : ${passA ? "PASS" : "FAIL"}`);
console.log(`B throughput        : ${passB ? `${tpsFinal1} tok/s cumulative, ${marginal1 ?? "n/a"} tok/s steady-state (${g1?.toks.length} toks, prompt=${g1?.prompt})` : "FAIL"}`);
console.log(`C stop mid-stream   : ${passC ? "PASS" : "FAIL"} (halt ${haltMs}ms, tail toks ${tailToks})`);
if (gens[1]) {
  const last2 = gens[1].toks[gens[1].toks.length - 1];
  console.log(`gen2: prompt=${gens[1].prompt} tokens, ${gens[1].toks.length} generated before stop, ${last2 ? last2.tps : "?"} tok/s`);
}
console.log("──── TRANSCRIPT after prompt 1 ────");
console.log(reply1);
console.log("────────────────────");
await browser.close();
server.close();
process.exit(passA && passB && passC ? 0 : 7);
