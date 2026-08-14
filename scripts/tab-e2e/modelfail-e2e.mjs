// Model-FAILURE surfaces tab-E2E: two shipped paths the rest of the lattice
// never drove, both asserted on what the USER ends up reading.
//
//   (1) PROMPT BLOCK. Google refuses the INPUT and answers with a frame that
//       has `promptFeedback.blockReason` and NO candidate (mirror of the Rust
//       fixture `wire::PROMPT_BLOCKED_FRAME_JSON`). Unmodelled it folded to
//       nothing and the turn ended with an empty note → `turn_flow::
//       classify_empty` read `Blank` → "check your session/balance" for what
//       was a content block. Assert the NAMED blocked copy, and assert the
//       blank/credits copy is absent — then the same frame MINUS
//       `blockReason` (ratings only, not a block) as the negative control,
//       which must still read as the ordinary blank turn.
//
//   (2) LH3002 BY WHOSE KEY. The upstream rejects the key with a 403. A BYOK
//       user owns that key (name it, show the provider's raw text, pop the
//       key modal); a PLATFORM user owns no key at all, so the same copy sent
//       them after something they cannot fix (telemetry #90). Both branches
//       run here — the BYOK leg is the CONTROL that proves the platform-leg
//       assertions aren't vacuous (same 403, same bundle, one localStorage
//       flag apart). ⛔ The platform leg must NOT pop the api-key modal.
//
// Host::Other (localhost) full chat app, model calls rerouted to a local fake,
// every other request aborted: zero spend, zero real network.
// Run: node scripts/tab-e2e/modelfail-e2e.mjs
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor } from "./lib.mjs";
import { startScriptedGemini, promptBlockedTurn, httpErrorTurn } from "./fake-gemini-scripted.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8803);
const FAKE_PORT = Number(process.env.LH_E2E_FAKE_PORT || PORT + 1);
const ORIGIN = `http://localhost:${PORT}`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("model-failure tab-E2E");

// The user-facing copy under test (verbatim slices, so a reword trips this).
const BLOCKED_COPY = "under its safety filter";          // turn_flow::empty_message(Blocked)
const BLANK_COPY = "check your session/balance";         // turn_flow::empty_message(Blank) — the bug
const BYOK_COPY = "check your Gemini key";               // error_codes::auth_failure_copy(true)
const PLATFORM_COPY = "not your $LH";                    // error_codes::auth_failure_copy(false)
const LH3002 = "LH3002";
// A marker planted in the provider's raw 403 body: BYOK shows the raw text,
// the platform branch must keep it in the console only (`show_raw: false`).
const RAW_MARKER = "e2e-raw-body-marker";

// A Google-shaped upstream key rejection. Deliberately free of "quota" /
// "429" / "clock" wording — `error_codes::classify` is order-sensitive and
// those would classify as rate-limit / stale-auth instead of LH3002.
const GOOGLE_403 = JSON.stringify({
  error: {
    code: 403,
    message: `Requests to this API generativelanguage.googleapis.com are blocked. ${RAW_MARKER}`,
    status: "PERMISSION_DENIED",
    details: [
      {
        "@type": "type.googleapis.com/google.rpc.ErrorInfo",
        reason: "API_KEY_SERVICE_BLOCKED",
        domain: "googleapis.com",
        metadata: { service: "generativelanguage.googleapis.com" },
      },
    ],
  },
});

// `promptFeedback` carrying ONLY ratings is NOT a block (loop.rs's fold gate:
// a block needs a `blockReason`). The negative control below proves the
// blocked assertion above discriminates rather than firing on any frame with
// a `promptFeedback` key at all.
const ratingsOnlyTurn = () => {
  const [frame] = promptBlockedTurn();
  delete frame.promptFeedback.blockReason;
  return [frame];
};

const script = [
  promptBlockedTurn("SAFETY"), // req 1 — the prompt-block frame
  ratingsOnlyTurn(), // req 2 — ratings only: NOT a block
  httpErrorTurn(403, GOOGLE_403), // req 3 — BYOK control
  httpErrorTurn(403, GOOGLE_403), // req 4 — platform branch
];

const server = await serve(ROOT, PORT);
const { server: fake, requests } = await startScriptedGemini(FAKE_PORT, script);
const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: "new",
  args: ["--no-first-run", "--disable-extensions"],
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
    req.abort().catch(() => {}); // chain RPC, the proxy, telemetry — nothing real
  });

  const transcript = () => page.evaluate(() => document.getElementById("transcript")?.textContent || "");
  const hasKeyModal = () => page.evaluate(() => !!document.getElementById("api-key-modal"));
  // Each leg reads the LAST turn only, so wipe the painted transcript between
  // them (DOM-only — the app appends into #transcript by id and never reads it
  // back). Otherwise leg 2's BYOK copy would still be on screen during leg 3.
  const clearTranscript = () => page.evaluate(() => {
    const t = document.getElementById("transcript");
    if (t) t.innerHTML = "";
  });
  const send = async (text) => {
    const ready = await waitFor(() => page.evaluate(() => !!document.getElementById("terminal-send")), 20000);
    if (!ready) throw new Error("send button never came back before: " + text);
    await page.evaluate((t) => {
      const ta = document.getElementById("prompt");
      ta.value = t;
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      document.getElementById("terminal-send").click();
    }, text);
  };
  const waitForCopy = (needle, ms = 25000) =>
    waitFor(async () => ((await transcript()).includes(needle) ? true : null), ms);

  await page.goto(ORIGIN + "/", { waitUntil: "domcontentloaded" });
  const ready = await waitFor(() => page.evaluate(() =>
    document.documentElement.hasAttribute("data-lh-ready") && !!document.getElementById("prompt")), 30000);
  check("boot: chat app painted (Host::Other)", !!ready);
  await sleep(500);

  // ── (1) PROMPT BLOCK ──────────────────────────────────────────────────────
  // BYOK here so the turn is a plain direct call; the block path is the same
  // whichever way the request reached the model.
  await page.evaluate(() => {
    localStorage.setItem("lh_model_access", "byok");
    sessionStorage.setItem("gemini_api_key", "AIza-throwaway-e2e-modelfail");
    document.getElementById("api-key-modal")?.remove();
  });
  await send("!write something the safety filter will refuse");
  const blocked = await waitForCopy(BLOCKED_COPY);
  check("prompt-block: the turn surfaces a NAMED blocked stop", !!blocked, (await transcript()).slice(-300));
  const afterBlock = await transcript();
  check("prompt-block: NOT the blank-turn 'check your session/balance' copy",
    !afterBlock.includes(BLANK_COPY), afterBlock.slice(-300));
  check("prompt-block: exactly one model request fired (a block is terminal, not retried)",
    requests.length === 1, `requests=${requests.length}`);

  // ── (1b) NEGATIVE CONTROL: ratings-only promptFeedback is NOT a block ─────
  // Same frame minus `blockReason`. It must fall through to the ordinary
  // blank-turn copy — if this leg also read as blocked, the assertion above
  // would be firing on the mere presence of `promptFeedback`.
  await clearTranscript();
  await send("!an ordinary turn the model answers with nothing");
  const blankPainted = await waitForCopy(BLANK_COPY);
  check("prompt-block control: ratings-only promptFeedback stays a plain blank turn",
    !!blankPainted, (await transcript()).slice(-300));
  check("prompt-block control: not mis-reported as a safety block",
    !(await transcript()).includes(BLOCKED_COPY), (await transcript()).slice(-300));

  // ── (2a) LH3002, BYOK — the CONTROL leg ───────────────────────────────────
  await clearTranscript();
  await send("!a byok turn the upstream will reject");
  const byokPainted = await waitForCopy(LH3002);
  check("lh3002/byok: the 403 surfaces as LH3002", !!byokPainted, (await transcript()).slice(-300));
  const byokText = await transcript();
  check("lh3002/byok: names the user's OWN key", byokText.includes(BYOK_COPY), byokText.slice(-300));
  check("lh3002/byok: shows the provider's raw body (show_raw)", byokText.includes(RAW_MARKER));
  const byokModal = await waitFor(() => hasKeyModal(), 8000);
  check("lh3002/byok: the api-key modal DOES pop (control — the assertion below can fail)", !!byokModal);

  // ── (2b) LH3002, PLATFORM — the telemetry #90 bug ─────────────────────────
  await page.evaluate(() => {
    document.getElementById("api-key-modal")?.remove();
    localStorage.setItem("lh_model_access", "credits"); // anything but "byok"
  });
  await clearTranscript();
  check("lh3002/platform: the control modal was cleared before the platform leg", !(await hasKeyModal()));
  await send("!a platform-credits turn the upstream will reject");
  const platformPainted = await waitForCopy(LH3002);
  check("lh3002/platform: the 403 surfaces as LH3002", !!platformPainted, (await transcript()).slice(-300));
  const platformText = await transcript();
  check("lh3002/platform: says server-side, not your $LH", platformText.includes(PLATFORM_COPY), platformText.slice(-300));
  check("lh3002/platform: never sends a keyless user after a Gemini key",
    !platformText.includes(BYOK_COPY), platformText.slice(-300));
  check("lh3002/platform: the provider's raw body stays out of the transcript",
    !platformText.includes(RAW_MARKER), platformText.slice(-300));
  // THE bug: popping a key modal at a user who has no key. Give it the same
  // window the control leg needed before declaring it absent.
  await sleep(3000);
  check("lh3002/platform: the api-key modal does NOT appear", !(await hasKeyModal()));

  check("4 model requests fired in total (one per leg, no retries)",
    requests.length === 4, `requests=${requests.length}`);

  console.log("\npage errors:", pageErrors.length ? pageErrors : "(none)");
  check("no uncaught page errors", pageErrors.length === 0, pageErrors.join(" | "));
} finally {
  await browser.close();
  server.close();
  fake.close();
}
finish();
