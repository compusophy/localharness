// Tab-E2E of the shipped browser bundle, served locally (Host::Other → full
// chat app, throwaway device identity). Verifies: (1) intent-router status +
// free 'balance' routing (no metered call, no api-key modal), (2) Stop
// turn-guard smoke — prompt-ack + TURN_ACTIVE release (model calls are
// intercepted; the genuine mid-stream stop proof is stop-e2e.mjs), (3) bell
// push-state line paint. Smoke: display overlay ESC close; inline-card
// sticky-header CSS (#30). Nothing real is called, nothing is spent.
//
//   node scripts/tab-e2e/tab-e2e-main.mjs [webRoot]
//
// Needs a built bundle (./scripts/build-web.sh) + puppeteer-core (README.md).
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor } from "./lib.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8792);
const URL = `http://localhost:${PORT}/`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("main-bundle tab-E2E");

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
const status = (page) => page.evaluate(() => (document.getElementById("system-status")?.textContent || ""));
const transcriptHTML = (page) => page.evaluate(() => (document.getElementById("transcript")?.innerHTML || ""));
const has = (page, id) => page.evaluate((i) => !!document.getElementById(i), id);

const server = await serve(ROOT, PORT);
const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: "new",
  args: ["--no-first-run", "--disable-extensions"],
});
try {
  const page = await browser.newPage();
  const pageErrors = [], consoleErrors = [];
  page.on("pageerror", (e) => pageErrors.push(String(e)));
  page.on("console", (m) => { if (m.type() === "error") consoleErrors.push(m.text()); });

  // Stall ONLY the metered model call so the Stop test has a live turn to kill.
  await page.setRequestInterception(true);
  const stalled = [];
  page.on("request", (req) => {
    if (req.url().includes("streamGenerateContent")) { stalled.push(req.url()); return; } // never respond
    req.continue().catch(() => {});
  });

  await page.goto(URL, { waitUntil: "domcontentloaded" });
  const ready = await waitFor(() => page.evaluate(() => document.documentElement.hasAttribute("data-lh-ready")), 30000);
  check("boot: data-lh-ready", !!ready);
  await sleep(500);

  // ── (a) router: fresh tab '/router status' says ON ──
  await send(page, "/router status");
  await waitFor(async () => (await status(page)).includes("intent router"));
  const st = await status(page);
  check("router: fresh-tab '/router status' reports ON (the default)", st.includes("ON (the default)"), JSON.stringify(st));

  // ── (a) 'balance' answers free with a card ──
  await send(page, "balance");
  let ok = await waitFor(async () => (await transcriptHTML(page)).includes("routed free — no $LH spent"), 15000);
  const tb = await transcriptHTML(page);
  check("router: 'balance' answers FREE with an inline card + footer", !!ok, tb.replace(/<[^>]+>/g, " ").slice(-300).trim());
  check("router: no api-key modal on the free route", !(await has(page, "api-key-modal")));
  check("router: no stalled model call fired for 'balance' (0 metered requests)", stalled.length === 0, String(stalled.length));

  // ── (c) bell: tap paints the push-state line. The INSTANT line (painted
  // synchronously by notif_bell_pressed) is read in the SAME task as the
  // click, before the async enroll outcome can overwrite it. ──
  const panel1 = await page.evaluate(() => {
    document.getElementById("notif-bell").click();
    const p = document.getElementById("notif-bell-panel");
    return p && !p.hasAttribute("hidden") ? p.textContent.trim() : null;
  });
  check("bell: panel opens on tap", panel1 !== null, JSON.stringify(panel1));
  check("bell: push-state line painted immediately (honest state, not silence)",
    !!panel1 && /push: (blocked|awaiting permission|enrolled|enrolling)/.test(panel1), JSON.stringify(panel1));
  await sleep(3000); // async verified-enroll outcome may overwrite the line
  const panel2 = await page.evaluate(() => {
    const p = document.getElementById("notif-bell-panel");
    return p && !p.hasAttribute("hidden") ? p.textContent.trim() : null;
  });
  check("bell: state line still honest after async enroll resolves (push:/⚠ reason, no silent blank)",
    panel2 === null || /push: |⚠/.test(panel2), JSON.stringify(panel2));
  await page.keyboard.press("Escape"); // close panel
  await sleep(200);

  // ── smoke: display overlay open + ESC close ──
  await send(page, "display");
  ok = await waitFor(() => has(page, "display-canvas"));
  check("display: 'display' opens the fullscreen overlay", !!ok);
  await page.keyboard.press("Escape");
  ok = await waitFor(async () => !(await has(page, "display-canvas")));
  check("display: ESC closes the overlay", !!ok);

  // ── smoke: inline-card sticky headers (#30) — computed style on live CSS ──
  const sticky = await page.evaluate(() => {
    const host = document.getElementById("transcript") || document.body;
    const probe = document.createElement("div");
    probe.innerHTML =
      '<div class="inline-card" id="probe-plain"><div class="ic-head">h</div><pre>x</pre></div>' +
      '<div class="inline-card embed-app-card" id="probe-embed"><div class="ic-head">h</div><pre>x</pre></div>';
    host.appendChild(probe);
    const plain = getComputedStyle(document.querySelector("#probe-plain .ic-head")).position;
    const embed = getComputedStyle(document.querySelector("#probe-embed .ic-head")).position;
    probe.remove();
    return { plain, embed };
  });
  check("cards: self-scrolling inline-card header stays sticky", sticky.plain === "sticky", JSON.stringify(sticky));
  check("cards: embed-app-card header NOT sticky (won't latch to chat scrollport, #30)", sticky.embed === "static", JSON.stringify(sticky));

  // ── (b) Stop turn-guard SMOKE: engage a metered turn and click Stop.
  // Host::Other has no tenant pricing, so the fail-closed pricing gate stops
  // the send before any model call fires — the GENUINE mid-stream stop proof
  // is stop-e2e.mjs (tenant-sim + local fake-Gemini stream). Here we assert
  // only the turn-guard mechanics: engage → instant ack → TURN_ACTIVE release.
  // Throwaway fake BYOK key; interception above guarantees nothing real is
  // ever called or spent even if a bundle lets the request through. ──
  await page.evaluate(() => {
    localStorage.setItem("lh_model_access", "byok");
    sessionStorage.setItem("gemini_api_key", "AIza-throwaway-e2e-stall");
  });
  await send(page, "!hello there, long metered request");
  const stopBtn = await waitFor(() => has(page, "terminal-stop"), 20000);
  check("stop: metered turn engaged (stop button appeared)", !!stopBtn);
  if (stopBtn) {
    const ack = await page.evaluate(async () => {
      const t0 = performance.now();
      document.getElementById("terminal-stop").click();
      // request_stop_turn paints synchronously — read back immediately
      const instantStatus = document.getElementById("system-status")?.textContent || "";
      let ackMs = null, how = null;
      for (;;) {
        const st = document.getElementById("system-status")?.textContent || "";
        const btnGone = !document.getElementById("terminal-stop");
        if (st.includes("stopping")) { ackMs = performance.now() - t0; how = "status:" + st; break; }
        if (btnGone) { ackMs = performance.now() - t0; how = "send-button-restored"; break; }
        if (performance.now() - t0 > 2000) { how = "timeout status=" + st; break; }
        await new Promise((r) => setTimeout(r, 20));
      }
      return { instantStatus, ackMs, how };
    });
    check("stop: first click acks within 300ms (status 'stopping…' or button swap)",
      ack.ackMs !== null && ack.ackMs <= 300,
      `ackMs=${ack.ackMs === null ? "none" : ack.ackMs.toFixed(0)} via=${ack.how} instant=${JSON.stringify(ack.instantStatus)}`);
    // TurnGuard must restore the send button (TURN_ACTIVE released)
    ok = await waitFor(() => page.evaluate(() => !!document.getElementById("terminal-send")), 10000);
    check("stop: TURN_ACTIVE released (send button restored)", !!ok);
    // (transcript Stopped-note + mid-stream reconcile asserted in stop-e2e.mjs)
    // a second message can be sent (free route — proves dispatch is live again)
    await send(page, "/router status");
    ok = await waitFor(async () => (await status(page)).includes("intent router"), 8000);
    check("stop: a second message sends fine after the stop", !!ok, await status(page));
  }

  console.log("\nconsole errors:", consoleErrors.length ? consoleErrors : "(none)");
  console.log("page errors:", pageErrors.length ? pageErrors : "(none)");
  check("no uncaught page errors", pageErrors.length === 0, pageErrors.join(" | "));
} finally {
  await browser.close();
  server.close();
}
finish();
