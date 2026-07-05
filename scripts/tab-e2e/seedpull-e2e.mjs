// Seed-pull round-trip tab-E2E: prove a PURE VISITOR's public face does not
// get torn through a visible `?seed_import=none` repaint (the live-face-sweep
// finding). Serves the built bundle over LOCAL HTTPS on 443 with the REAL
// production URL shapes (https://localharness.xyz apex + a tenant subdomain,
// no ports — seed_pull.rs hardcodes them), host-resolver-mapped to 127.0.0.1
// with a throwaway self-signed cert (--ignore-certificate-errors). The chain
// RPC is a local fake (every read returns one 32-byte word carrying the
// synthetic all-1s owner fixture — same fixture as stop-e2e.mjs; never a real
// address), and the app store / fonts / stripe hosts 404 locally. Zero real
// network, nothing spent. NO request interception — puppeteer's Fetch-domain
// interception disables bfcache, which is exactly the behavior under test.
//
//   node scripts/tab-e2e/seedpull-e2e.mjs [webRoot]      (LH_WEB_ROOT works too)
//
// Methodology (the sweep's): per-document trace in the tenant origin's
// localStorage — document loads (`doc:<search>`), the "setting up this
// device…" interstitial, and the first face paint per document — plus a
// data-stamp on the painted face DOM and pageshow.persisted to distinguish a
// bfcache restore (face element identity preserved) from a clean reload.
//
// Expected BASELINE (pre-fix bundle): round-trip lands on ?seed_import=none,
// paints the interstitial, repaints the face (2 docs / 2 face paints).
// Expected FIXED bundle: no seed_import=none navigation, no interstitial;
// best case the original face survives untouched (bfcache), worst case one
// clean-URL reload. Pass LH_E2E_EXPECT=baseline to assert the former.
import { spawnSync } from "node:child_process";
import { createServer } from "node:https";
import { readFileSync, existsSync, mkdtempSync } from "node:fs";
import { join, extname, resolve } from "node:path";
import { tmpdir } from "node:os";
import puppeteer from "puppeteer-core";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor } from "./lib.mjs";

const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const EXPECT = process.env.LH_E2E_EXPECT === "baseline" ? "baseline" : "fixed";
const TENANT = "teste2e.localharness.xyz";
const { check, finish } = makeChecker(`seed-pull round-trip tab-E2E (${EXPECT})`);

// ── throwaway self-signed TLS cert (content irrelevant under
// --ignore-certificate-errors; git-bash ships openssl on Windows) ──
function makeCert() {
  const dir = mkdtempSync(join(tmpdir(), "lh-e2e-cert-"));
  const key = join(dir, "key.pem"), crt = join(dir, "cert.pem");
  const candidates = ["openssl", "C:\\Program Files\\Git\\usr\\bin\\openssl.exe"];
  for (const bin of candidates) {
    const r = spawnSync(bin, ["req", "-x509", "-newkey", "rsa:2048", "-keyout", key,
      "-out", crt, "-days", "2", "-nodes", "-subj", "/CN=lh-e2e-throwaway"], { stdio: "ignore" });
    if (r.status === 0 && existsSync(key) && existsSync(crt)) {
      return { key: readFileSync(key), cert: readFileSync(crt) };
    }
  }
  console.error("seedpull-e2e: no openssl found to mint a throwaway cert (need it for local https).");
  process.exit(2);
}

const MIME = {
  ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".mjs": "text/javascript",
  ".wasm": "application/wasm", ".css": "text/css", ".json": "application/json",
  ".svg": "image/svg+xml", ".png": "image/png", ".webmanifest": "application/manifest+json",
  ".txt": "text/plain", ".md": "text/plain", ".ts": "text/plain",
};

// Synthetic all-1s owner fixture (stop-e2e.mjs's), as one ABI word. Every
// fake RPC read returns this word: owner_of_name decodes a nonzero owner (the
// visitor flow under test); other reads decode it as garbage and degrade.
const WORD = "0x" + "00".repeat(12) + "11".repeat(20);

const server = createServer(makeCert(), (req, res) => {
  const host = (req.headers.host || "").split(":")[0];
  let body = "";
  req.on("data", (c) => (body += c));
  req.on("end", () => {
    // CORS for the cross-origin RPC fetches from the wasm.
    const cors = {
      "Access-Control-Allow-Origin": "*",
      "Access-Control-Allow-Methods": "POST, GET, OPTIONS",
      "Access-Control-Allow-Headers": "*",
    };
    if (req.method === "OPTIONS") { res.writeHead(204, cors); res.end(); return; }
    if (host === "rpc.tempo.xyz") {
      let out;
      try {
        const q = JSON.parse(body || "{}");
        const one = (r) => ({ jsonrpc: "2.0", id: r.id ?? 1, result: WORD });
        out = Array.isArray(q) ? q.map(one) : one(q);
      } catch { out = { jsonrpc: "2.0", id: 1, result: WORD }; }
      res.writeHead(200, { "content-type": "application/json", ...cors });
      res.end(JSON.stringify(out));
      return;
    }
    if (host === "localharness.xyz" || host.endsWith(".localharness.xyz")) {
      const path = decodeURIComponent(new URL(req.url, "https://x").pathname);
      const rel = path === "/" ? "index.html" : path.replace(/^\/+/, "");
      const rootN = resolve(ROOT);
      const file = resolve(join(rootN, rel));
      if (!file.startsWith(rootN) || !existsSync(file)) { res.writeHead(404); res.end(); return; }
      // Match production Vercel headers (vercel.json): `no-store` here would
      // make every page bfcache-INELIGIBLE (MainResourceHasCacheControlNoStore)
      // and mask the exact restore behavior this suite measures.
      res.writeHead(200, { "content-type": MIME[extname(file)] || "application/octet-stream", "cache-control": "public, max-age=0, must-revalidate" });
      res.end(readFileSync(file));
      return;
    }
    res.writeHead(404, cors); // app store / fonts / stripe: hermetic local miss
    res.end();
  });
});
await new Promise((ok, bad) => { server.on("error", bad); server.listen(443, "127.0.0.1", ok); })
  .catch((e) => { console.error("seedpull-e2e: cannot bind 127.0.0.1:443 (" + e.code + ") — free the port and rerun."); process.exit(2); });

const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: "new",
  acceptInsecureCerts: true, // puppeteer 23.x name for ignoreHTTPSErrors
  args: [
    "--no-first-run", "--disable-extensions",
    "--host-resolver-rules=" + [
      "MAP localharness.xyz 127.0.0.1", "MAP *.localharness.xyz 127.0.0.1",
      "MAP rpc.tempo.xyz 127.0.0.1", "MAP proxy-tau-ten-15.vercel.app 127.0.0.1",
      "MAP fonts.googleapis.com 127.0.0.1", "MAP fonts.gstatic.com 127.0.0.1",
      "MAP js.stripe.com 127.0.0.1",
    ].join(", "),
    "--ignore-certificate-errors",
    "--enable-features=BackForwardCache",
  ],
});
try {
  const page = await browser.newPage();
  const pageErrors = [];
  page.on("pageerror", (e) => pageErrors.push(String(e)));
  if (process.env.LH_E2E_DEBUG) {
    page.on("requestfailed", (r) => console.log("REQFAIL", r.url().slice(0, 90), r.failure()?.errorText));
    page.on("console", (m) => console.log("CON", m.text().slice(0, 140)));
  }
  // Surface WHY a history.back() restore skipped the bfcache (honest-residual
  // reporting; the harness's own CDP attach is a known blocker).
  const cdp = await page.createCDPSession();
  await cdp.send("Page.enable").catch(() => {});
  cdp.on("Page.backForwardCacheNotUsed", (ev) => {
    const why = (ev.notRestoredExplanations || []).map((x) => x.reason).join(",");
    console.log("bfcache not used:", why || "(no explanation)");
  });
  const navs = [];
  const navT = [];
  page.on("framenavigated", (fr) => { if (fr === page.mainFrame()) { navs.push(fr.url()); navT.push(Date.now()); } });

  // Per-document instrumentation (tenant origin only): document loads, the
  // interstitial, first face paint per document — into localStorage so it
  // survives the cross-document round-trip. pageshow.persisted marks a
  // bfcache restore of THIS document.
  await page.evaluateOnNewDocument((tenantHost) => {
    window.addEventListener("pageshow", (e) => { window.__lhPersisted = e.persisted; });
    if (location.hostname !== tenantHost) return;
    const push = (tag) => {
      try {
        const a = JSON.parse(localStorage.getItem("lh_e2e_trace") || "[]");
        a.push(tag);
        localStorage.setItem("lh_e2e_trace", JSON.stringify(a));
      } catch {}
    };
    push("doc:" + (location.search || "(clean)"));
    let interstitial = false, face = false;
    const scan = () => {
      const root = document.getElementById("root");
      if (!root) return;
      const t = root.textContent || "";
      if (!interstitial && t.includes("setting up this device")) { interstitial = true; push("interstitial"); }
      if (!face && t.includes("teste2e") && !t.includes("resolving teste2e")) { face = true; push("facepaint"); }
    };
    // `document` is the observable Node that already exists at
    // evaluateOnNewDocument time (documentElement is still null here).
    new MutationObserver(scan).observe(document, { childList: true, subtree: true });
  }, TENANT);

  // 1. Visitor's first paint.
  await page.goto(`https://${TENANT}/`, { waitUntil: "domcontentloaded" });
  const painted = await waitFor(() => page.evaluate(() => {
    const t = document.getElementById("root")?.textContent || "";
    return t.includes("teste2e") && !t.includes("resolving teste2e") ? true : null;
  }), 30000);
  check("visitor face painted (first paint)", !!painted);
  // Stamp the painted face DOM — element identity across the round-trip.
  await page.evaluate(() => { document.getElementById("root")?.setAttribute("data-lh-face-stamp", "original"); });

  // 2. The auto-kick navigates the top-level tab to the apex export leg.
  const kicked = await waitFor(() => navs.find((u) => u.includes("seed_export=1")) || null, 45000, 200);
  check("auto-kick fired: top-level nav to apex ?seed_export=1", !!kicked, kicked || "(no kick in 45s)");

  // 3. The bounce home. Wait until the tab is back on the tenant origin and
  //    the face is up again (or still up).
  const home = await waitFor(() => page.evaluate((tenantHost) => {
    if (location.hostname !== tenantHost) return null;
    const t = document.getElementById("root")?.textContent || "";
    return t.includes("teste2e") ? location.href : null;
  }, TENANT).catch(() => null), 45000, 200);
  check("tab returned to the tenant with the face up", !!home, home || "(never returned)");
  await sleep(1500); // let any straggler repaint land in the trace

  const state = await page.evaluate(() => ({
    url: location.href,
    persisted: window.__lhPersisted === true,
    stamp: document.getElementById("root")?.getAttribute("data-lh-face-stamp") || null,
    trace: JSON.parse(localStorage.getItem("lh_e2e_trace") || "[]"),
  })).catch((e) => ({ url: "(evaluate failed: " + e + ")", persisted: false, stamp: null, trace: [] }));

  const sawNone = navs.some((u) => u.includes("seed_import=none"));
  const interstitials = state.trace.filter((t) => t === "interstitial").length;
  const docs = state.trace.filter((t) => t.startsWith("doc:")).length;
  const facepaints = state.trace.filter((t) => t === "facepaint").length;
  console.log("nav trace:", JSON.stringify(navs, null, 1));
  // Visible-interruption window ≈ from committing the apex export leg to the
  // tab being back on the tenant (paint-holding hides the gap before commit).
  const iA = navs.findIndex((u) => u.includes("seed_export=1"));
  const iBack = navs.findIndex((u, i) => i > iA && u.startsWith(`https://${TENANT}/`));
  if (iA >= 0 && iBack > iA) console.log(`away-from-face window: ${navT[iBack] - navT[iA]}ms (apex commit → back on tenant)`);
  console.log("tenant trace:", JSON.stringify(state.trace), "| persisted:", state.persisted, "| stamp:", state.stamp, "| final url:", state.url);

  check("final URL is the clean tenant root", state.url === `https://${TENANT}/`, state.url);
  if (EXPECT === "baseline") {
    check("BASELINE: round-trip returned via ?seed_import=none", sawNone);
    check("BASELINE: 'setting up this device…' interstitial painted", interstitials >= 1);
    check("BASELINE: face painted TWICE (visible repaint)", facepaints >= 2, `facepaints=${facepaints} docs=${docs}`);
  } else {
    check("FIXED: no ?seed_import=none navigation anywhere", !sawNone);
    check("FIXED: interstitial never painted", interstitials === 0);
    check("FIXED: at most one face repaint after the round-trip", facepaints <= 2, `facepaints=${facepaints} docs=${docs}`);
    // The gold outcome — bfcache restored the ORIGINAL face untouched.
    const untouched = state.persisted && state.stamp === "original" && docs === 1 && facepaints === 1;
    console.log(untouched
      ? "bfcache RESTORE: original face untouched (zero repaint)"
      : `bfcache miss: clean-URL reload (docs=${docs}, facepaints=${facepaints}, persisted=${state.persisted}, stamp=${state.stamp})`);
    check("FIXED: face untouched (bfcache) OR one clean reload without any seed_import leg",
      untouched || (!sawNone && interstitials === 0 && docs <= 2));
  }

  // ── Scenario B (fixed bundle only): with an apex `.lh_wallet` PRESENT the
  // boot.js fast bounce must DEFER — the wasm boots ("linking this device…"
  // paints) and owns the decision. Guards the owner-adoption safety property:
  // a seed-holding apex is never bounced by JS before the wasm can seal. ──
  if (EXPECT === "fixed") {
    const p2 = await browser.newPage();
    await p2.goto("https://localharness.xyz/", { waitUntil: "domcontentloaded" });
    await p2.evaluate(async () => {
      const root = await navigator.storage.getDirectory();
      const fh = await root.getFileHandle(".lh_wallet", { create: true });
      const w = await fh.createWritable();
      await w.write("not a real mnemonic — e2e fixture so the file EXISTS"); // load() → None; wasm decides
      await w.close();
    });
    // Proof the fast path DEFERRED = the export document went on to import
    // the wasm shim (boot.js only reaches that import after
    // lhSeedExportFastBounce returns false). Racing the on-screen
    // "linking this device…" text is flaky — the wasm-side bounce can beat
    // the poll — so watch the request stream instead.
    const p2reqs = [];
    p2.on("request", (r) => p2reqs.push(r.url()));
    await p2.goto("https://localharness.xyz/?seed_export=1&to=teste2e#epk=00", { waitUntil: "domcontentloaded" });
    const wasmOwned = await waitFor(() =>
      p2reqs.some((u) => u.includes("/pkg/localharness")) ? true : null, 20000, 100);
    check("fast bounce DEFERS when .lh_wallet exists (export doc loads the wasm)", !!wasmOwned);
    // …and the wasm-side no-seed decision still bounces back in-tab.
    const back2 = await waitFor(() => {
      const u = p2.url();
      return u === "https://localharness.xyz/" ? u : null;
    }, 20000, 200);
    check("wasm-side bounce still returns (history.back to the previous entry)", !!back2, p2.url());
    await p2.close();
  }

  console.log("page errors:", pageErrors.length ? pageErrors : "(none)");
} finally {
  await browser.close();
  server.close();
}
finish();
