// Stall-recovery tab-E2E (telemetry #80): proves the SHIPPED run_send wiring
// end-to-end with a scripted fake model — the eval proved the NUDGE works on
// the live model (9/9); this proves the LOOP actually sends it:
//
//   turn 1 (user "!finish the maze…"): the fake replies with the exact wild
//     stall shape — a SHORT text-only "I will now write…" declaration. Today
//     that classifies FinalAnswer; the #80 fix must intercept it.
//   turn 2 (hidden): the harness asserts the REQUEST BODY carries
//     turn_flow::STALL_NUDGE (the wiring proof) and that NO user bubble was
//     painted. The fake answers with update_plan(steps, completed) — the real
//     closure tool executes and the plan card paints.
//   turn 3 (hidden): an ordinary AUTO_CONTINUE follows the tool activity; the
//     fake closes with a final text and the run ends cleanly.
//
// Tenant-sim + BYOK-fake, model calls rerouted to loopback: zero spend, zero
// real network. Run: node scripts/tab-e2e/stall-e2e.mjs
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor } from "./lib.mjs";
import { startScriptedGemini, textTurn, functionCallTurn } from "./fake-gemini-scripted.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8797);
const FAKE_PORT = PORT + 1;
const ORIGIN = `http://teste2e.localharness.xyz:${PORT}`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("stall-recovery tab-E2E");

// The exact nudge text the wiring must send (turn_flow::STALL_NUDGE).
const STALL_NUDGE_MARK = "your last reply called no tool, which ends the run";
// The stall reply: short, pure declaration — the wild #80 shape.
const STALL_TEXT =
  "I will now write the complete maze-runner cartridge with the win screen \
and the high-score display, and publish it to its own subdomain.";

// Request flow (an engine "turn" spans MULTIPLE requests when tools fire):
//   req 1: the user turn            → stall text (FinalAnswer… intercepted)
//   req 2: the STALL_NUDGE turn     → update_plan functionCall
//   req 3: the tool-RESULT follow-up (same engine turn) → closing text
//   req 4: run_send AUTO_CONTINUE (tool activity, no finish) → final text
const script = [
  textTurn(STALL_TEXT),
  functionCallTurn("update_plan", { steps: ["ship the maze game"], completed: [0], note: "shipping" }),
  textTurn("All done — the plan is complete."),
  textTurn("All done — the plan is complete."),
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

  // Tenant-sim boot (the stop-e2e dance): owner hint + BYOK fake key.
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
    sessionStorage.setItem("gemini_api_key", "AIza-throwaway-e2e-stall");
    document.getElementById("api-key-modal")?.remove();
  });

  // ── the turn: '!' forces the model path; the fake stalls, the fix recovers ──
  await page.evaluate(() => {
    const ta = document.getElementById("prompt");
    ta.value = "!finish the maze game and ship it";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    document.getElementById("terminal-send").click();
  });

  // Turn 1's stall text paints (the model DID reply — the fix must not hide it).
  const stallPainted = await waitFor(() => page.evaluate(() =>
    (document.getElementById("transcript")?.textContent || "").includes("I will now write the complete maze-runner") ? true : null), 20000);
  check("stall: the narrated stall painted in the transcript", !!stallPainted);

  // The run must CONTINUE past it: the plan card from turn 2's update_plan.
  const planPainted = await waitFor(() => page.evaluate(() =>
    (document.getElementById("transcript")?.textContent || "").includes("ship the maze game") ? true : null), 20000);
  check("stall: recovery turn executed update_plan (plan card painted)", !!planPainted);

  // And close cleanly on turn 3's final text.
  const closed = await waitFor(() => page.evaluate(() =>
    (document.getElementById("transcript")?.textContent || "").includes("All done — the plan is complete.") ? true : null), 20000);
  check("stall: run closed on the final answer", !!closed);

  // THE WIRING PROOF: request 2's body carries the STALL_NUDGE as a user turn.
  check("stall: 4 model requests fired (stall → nudge → tool result → auto-continue)",
    requests.length === 4, `requests=${requests.length}`);
  const asText = (r) => JSON.stringify(r);
  check("stall: request 2 carries turn_flow::STALL_NUDGE",
    requests.length >= 2 && asText(requests[1]).includes(STALL_NUDGE_MARK));
  check("stall: request 3 is the update_plan tool-result follow-up",
    requests.length >= 3 && asText(requests[2]).includes("functionResponse"));
  check("stall: request 4 is the ordinary goal continuation",
    requests.length >= 4 && asText(requests[3]).includes("Continue toward the user's goal"));
  // The nudge is INTERNAL: it must never paint as a user bubble.
  check("stall: the nudge never painted as a user bubble", await page.evaluate((mark) =>
    !(document.getElementById("transcript")?.textContent || "").includes(mark), STALL_NUDGE_MARK));

  console.log("\npage errors:", pageErrors.length ? pageErrors : "(none)");
  check("no uncaught page errors", pageErrors.length === 0, pageErrors.join(" | "));
} finally {
  await browser.close();
  server.close();
  fake.close();
}
finish();
