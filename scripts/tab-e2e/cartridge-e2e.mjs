// Cartridge-loop tab-E2E of the shipped bundle (Host::Other / localhost),
// covering the 3 headlessly-drivable slices of the loop WITHOUT any chain
// write or real publish:
//
//   (0) fixture gate — examples/cartridges/bouncing_ball.rl compile-checks
//       with the repo CLI (`localharness compile`, local only); skipped when
//       no target/release binary exists.
//   (B) FEED EMBED via the history-replay seam: a synthetic SUCCESSFUL
//       `create_and_publish_app` tool result pre-written into the OPFS
//       history (`.lh_history.json`, plaintext on a seedless origin) replays
//       as the playable `embed-app-card` — and `history::resume_last_cartridge`
//       recompiles the recorded SOURCE in-browser and boots the cartridge
//       worker into the card's canvas (the designed reopen-resume path). We
//       assert the card + live link + canvas, the worker boot
//       (`window.__lhEmbedTrace` + page.workers()), and real animation.
//   (A) LOCAL app.rl paints via `try_paint_app`: with `app.rl` in OPFS a
//       reload boots the fullscreen cartridge face (in-browser rustlite
//       compile → `run_in_root_canvas`), with the `[studio]` escape
//       (owner_overlay) and NO chat prompt. Animation is asserted across a
//       window longer than the worker WATCHDOG (1.5s + tick) — a watchdog
//       kill freezes frames, so continued motion proves it never tripped.
//   (C) DISPLAY OVERLAY runs the fixture and ESC stops the worker:
//       `?edit=1` forces the workshop, an injected `opfs-open app.rl` probe
//       drives the app's own action seam (files-modal open path →
//       `run_cartridge_file` → overlay `run_wasm`), then ESC closes the
//       overlay and the Web Worker count drops to 0 (genuine termination,
//       not just DOM removal).
//
// NOT covered here (honestly): the LIVE auto-embed at the tool-success path
// (`chat::stream_turn` → `launch_pending_embed`) needs a real model turn
// calling `create_and_publish_app`, i.e. a fake-Gemini functionCall stream +
// tenant-sim; and the owner-studio `#studio-app-slot` pin is tenant-only
// (`paint_tenant` → `mount_studio_app_card`), unreachable on Host::Other.
// Zero network spend: every non-local request is aborted; no metered model
// call ever fires (asserted).
//
//   node scripts/tab-e2e/cartridge-e2e.mjs [webRoot]
//
// Needs a built bundle (./scripts/build-web.sh) + puppeteer-core (README.md).
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor, REPO_ROOT } from "./lib.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8792);
const URL = `http://localhost:${PORT}/`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("cartridge-loop tab-E2E");

const FIXTURE_PATH = join(REPO_ROOT, "examples", "cartridges", "bouncing_ball.rl");
const FIXTURE = readFileSync(FIXTURE_PATH, "utf8");

// ── (0) fixture gate: the .rl compiles with the repo's own rustlite CLI ──
const CLI = process.env.LH_CLI
  || join(REPO_ROOT, "target", "release", process.platform === "win32" ? "localharness.exe" : "localharness");
if (existsSync(CLI)) {
  const r = spawnSync(CLI, ["compile", FIXTURE_PATH], { encoding: "utf8" });
  const line = (r.stdout || r.stderr || "").split("\n").find((l) => l.includes("compiled")) || (r.stderr || "").trim();
  check("fixture: bouncing_ball.rl compile-checks with the repo CLI (local, no chain write)",
    r.status === 0, line.trim());
} else {
  console.log("(fixture CLI gate skipped — no target/release/localharness binary; browser compile still covers it)");
}

// The synthetic history: one SUCCESSFUL create_and_publish_app turn in the
// Gemini wire shape `history_bytes` persists (a JSON array of Contents; the
// functionResponse carries `url` + no `error`, so the shared auto-embed
// predicate `turn_flow::tool_result_embeds_cartridge` gates it IN).
const HISTORY = JSON.stringify([
  { role: "user", parts: [{ text: "build and publish the bouncing ball" }] },
  { role: "model", parts: [{ functionCall: { name: "create_and_publish_app", args: { name: "e2eball", source: FIXTURE } } }] },
  { role: "user", parts: [{ functionResponse: { name: "create_and_publish_app", response: { name: "e2eball", url: "https://e2eball.localharness.xyz/", tx_hash: "off-chain", off_chain: true, updated: false } } }] },
  { role: "model", parts: [{ text: "Published e2eball — it plays inline above." }] },
]);

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

  // Hermetic: same-origin + data: only. Metered model calls and telemetry
  // posts are counted (they must be ZERO — nothing metered, no crash report).
  const modelCalls = [], telemetryPosts = [];
  await page.setRequestInterception(true);
  page.on("request", (req) => {
    const u = req.url();
    if (u.includes("streamGenerateContent")) { modelCalls.push(u.slice(0, 90)); return; } // stall
    if (u.includes("/api/telemetry")) telemetryPosts.push(u.slice(0, 90));
    if (u.startsWith(`http://localhost:${PORT}`) || u.startsWith(`http://127.0.0.1:${PORT}`) || u.startsWith("data:")) {
      req.continue().catch(() => {});
      return;
    }
    // Stage (D) call receipts: the published library's bytes come from the
    // FREE off-chain app store — the ONE external URL allowed through (a
    // plain unauthenticated GET; no $LH, no chain write).
    if (u.includes("/api/app?name=")) {
      req.continue().catch(() => {});
      return;
    }
    req.abort().catch(() => {}); // RPC / proxy / fonts: nothing real leaves the box
  });

  const opfsWrite = (name, text) => page.evaluate(async (n, t) => {
    const root = await navigator.storage.getDirectory();
    const fh = await root.getFileHandle(n, { create: true });
    const w = await fh.createWritable();
    await w.write(t);
    await w.close();
  }, name, text);
  const snap = (sel) => page.evaluate((s) => {
    const c = document.querySelector(s);
    return c ? c.toDataURL() : null;
  }, sel);
  /// Two frames `gapMs` apart must both exist and differ (the ball moves).
  const animates = async (sel, gapMs) => {
    const a = await snap(sel);
    await sleep(gapMs);
    const b = await snap(sel);
    return !!a && !!b && a !== b;
  };
  const workers = () => page.workers().length;

  // ── (B) feed embed card via the OPFS history-replay seam ──
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  let ready = await waitFor(() => page.evaluate(() => document.documentElement.hasAttribute("data-lh-ready")), 30000);
  check("boot: workshop ready (fresh origin)", !!ready);
  await opfsWrite(".lh_history.json", HISTORY);
  await page.reload({ waitUntil: "domcontentloaded" });
  ready = await waitFor(() => page.evaluate(() => document.documentElement.hasAttribute("data-lh-ready")), 30000);
  check("embed: workshop ready after history plant", !!ready);

  const card = await waitFor(() => page.evaluate(() =>
    !!document.querySelector("#transcript .embed-app-card canvas.embed-app-canvas") || null), 15000);
  check("embed: replayed create_and_publish_app renders the playable embed-app-card (canvas inside the tool card)", !!card);
  check("embed: card header links the live subdomain", await page.evaluate(() =>
    !!document.querySelector('#transcript .embed-app-card a[href="https://e2eball.localharness.xyz/"]')));
  check("embed: card carries the [fullscreen] relaunch", await page.evaluate(() =>
    !!document.querySelector('#transcript .embed-app-card [data-action="run-in-display"]')));

  // resume_last_cartridge recompiles the recorded SOURCE and boots the worker
  // into THIS card's canvas — the embed-canvas → worker boot, observable.
  const trace = await waitFor(() => page.evaluate(() => {
    const t = globalThis.__lhEmbedTrace;
    return t && t.startsWith("launched into") ? t : null;
  }), 15000);
  check("embed: cartridge worker launched into the card's canvas (__lhEmbedTrace)", !!trace, JSON.stringify(trace));
  const w1 = await waitFor(() => (workers() >= 1 ? workers() : null), 8000);
  check("embed: a live Web Worker is running the cartridge", !!w1, `workers=${workers()}`);
  check("embed: the embedded cartridge ANIMATES (two frames differ)", await animates("#transcript canvas.embed-app-canvas", 300));
  check("embed: no metered model call fired (replay is free)", modelCalls.length === 0, String(modelCalls.length));

  // ── (A) local app.rl on Host::Other paints via try_paint_app ──
  await opfsWrite("app.rl", FIXTURE);
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  const face = await waitFor(() => page.evaluate(() => !!document.getElementById("display-canvas") || null), 30000);
  check("app.rl: reload boots the fullscreen cartridge face (in-browser rustlite compile)", !!face);
  check("app.rl: the face is NOT the workshop (no chat prompt)", await page.evaluate(() => !document.getElementById("prompt")));
  check("app.rl: [studio] escape painted (owner_overlay on Host::Other)", await page.evaluate(() =>
    !!document.querySelector('a.app-edit[href="?edit=1"]')));
  const w2 = await waitFor(() => (workers() >= 1 ? workers() : null), 8000);
  check("app.rl: cartridge worker live", !!w2, `workers=${workers()}`);
  // Span the watchdog window (WATCHDOG_MS 1500 + tick 500): a watchdog kill
  // freezes the frame, so continued motion proves it never tripped.
  check("app.rl: still animating past the watchdog window (no LH1001 kill)", await animates("#display-canvas", 2300));

  // ── (C) display overlay runs the fixture; ESC stops the worker ──
  await opfsWrite(".lh_history.json", ""); // clear the replay so no embed resume races the overlay
  await page.goto(URL + "?edit=1", { waitUntil: "domcontentloaded" });
  ready = await waitFor(() => page.evaluate(() =>
    document.documentElement.hasAttribute("data-lh-ready") && !!document.getElementById("prompt")), 30000);
  check("overlay: ?edit=1 forces the workshop even with app.rl present", !!ready);
  check("overlay: no display canvas before the run", await page.evaluate(() => !document.getElementById("display-canvas")));

  // Drive the app's OWN action seam: a probe button dispatches `opfs-open
  // app.rl` through the delegated listener → run_cartridge_file → overlay.
  await page.evaluate(() => {
    const b = document.createElement("button");
    b.dataset.action = "opfs-open";
    b.dataset.arg = "app.rl";
    document.body.appendChild(b);
    b.click();
    b.remove();
  });
  const overlay = await waitFor(() => page.evaluate(() => {
    const o = document.getElementById("display-overlay");
    return o && !o.hasAttribute("hidden") && !!document.getElementById("display-canvas") ? true : null;
  }), 15000);
  check("overlay: opening app.rl mounts the display overlay + canvas", !!overlay);
  const w3 = await waitFor(() => (workers() >= 1 ? workers() : null), 8000);
  check("overlay: cartridge worker live in the overlay", !!w3, `workers=${workers()}`);
  check("overlay: overlay cartridge animates", await animates("#display-canvas", 300));

  await page.keyboard.press("Escape");
  const closed = await waitFor(() => page.evaluate(() => !document.getElementById("display-canvas") || null), 8000);
  check("overlay: ESC closes the overlay", !!closed);
  const gone = await waitFor(() => (workers() === 0 ? true : null), 8000);
  check("overlay: ESC TERMINATES the cartridge worker (worker count → 0)", !!gone, `workers=${workers()}`);
  check("watchdog/crash telemetry never fired (0 /api/telemetry posts)", telemetryPosts.length === 0, String(telemetryPosts.length));
  check("no metered model call across the whole run", modelCalls.length === 0, String(modelCalls.length));

  // ── (D) CALL RECEIPTS end-to-end: worker records → frame post →
  //        main-thread hash binding → .lh_receipts.jsonl OPFS ring ──
  // The one link the wiring test can't reach is the postMessage boundary
  // (worker.rs "frame" arm → bridge/receipts.rs → OPFS). Prove it with the
  // REAL published library: app.rl = the uses_lib consumer, which spawn_libs
  // `lib-physics` (fetched from the FREE off-chain app store — the single
  // external URL this harness allows) and calls its exports every frame.
  await opfsWrite("app.rl", readFileSync(join(REPO_ROOT, "examples", "cartridges", "uses_lib.rl"), "utf8"));
  await page.goto(URL, { waitUntil: "domcontentloaded" });
  const faceD = await waitFor(() => page.evaluate(() => !!document.getElementById("display-canvas") || null), 30000);
  check("receipts: uses_lib face boots", !!faceD);
  // "LIB PHYSICS OK" only paints when spawn_lib mounted AND call_ok()==0 —
  // i.e. real cross-cartridge calls are happening. Don't race the physics
  // (the ball SETTLES, and a settled scene repeats frames identically —
  // this flaked): assert the canvas is painted non-uniformly instead; the
  // receipts file below is the real liveness signal.
  const painted = await waitFor(() => page.evaluate(() => {
    const c = document.getElementById("display-canvas");
    if (!c) return null;
    const px = c.getContext("2d").getImageData(0, 0, c.width, c.height).data;
    let colors = new Set();
    for (let i = 0; i < px.length; i += 4096) colors.add((px[i] << 16) | (px[i + 1] << 8) | px[i + 2]);
    return colors.size > 1 ? true : null; // more than one sampled color = drawn scene
  }), 15000);
  check("receipts: composition painted a non-uniform scene (library mounted + drawing)", !!painted);
  const ring = await waitFor(() => page.evaluate(async () => {
    try {
      const root = await navigator.storage.getDirectory();
      const fh = await root.getFileHandle(".lh_receipts.jsonl");
      const text = await (await fh.getFile()).text();
      return text.trim() ? text : null;
    } catch (_e) { return null; }
  }), 15000);
  check("receipts: .lh_receipts.jsonl exists with content", !!ring, ring ? `${ring.trim().split("\n").length} line(s)` : "missing");
  if (ring) {
    const lines = ring.trim().split("\n").map((l) => JSON.parse(l));
    const ok = lines.find((r) => r.call && r.call.status === "ok" && r.module === "lib-physics");
    check("receipts: an OK call receipt binds module lib-physics", !!ok, ok ? ok.receipt : "none");
    check("receipts: receipt hash + module keccak are hex-committed",
      !!ok && /^0x[0-9a-f]{64}$/.test(ok.receipt) && /^0x[0-9a-f]{64}$/.test(ok.module_keccak),
      ok ? ok.module_keccak : "");
    check("receipts: source is explicitly UNBOUND on a call receipt (zeros)",
      !!ok && /^0x0{64}$/.test(ok.source_keccak));
    const exports = new Set(lines.filter((r) => r.call).map((r) => r.call.export));
    check("receipts: the physics exports the consumer calls all left records",
      ["gravity", "integrate", "bounce", "clamp"].every((e) => exports.has(e)),
      [...exports].join(","));
    check("receipts: ring respects its cap", lines.length <= 256, String(lines.length));

    // verify_receipt's containment primitive, in the SHIPPED bundle: post a
    // one-shot `lib_call` to a fresh dedicated worker with the real published
    // lib-physics bytes and one recorded call — the re-execution must
    // reproduce the receipt's outcome. (The Rust driver wraps exactly this
    // message; its chat-tool path needs a real model turn, covered by
    // dogfood, not this zero-spend harness.)
    const okLine = ring.trim().split("\n").map((l) => JSON.parse(l))
      .find((r) => r.call && r.call.status === "ok" && r.call.result !== null);
    if (okLine) {
      const reexec = await page.evaluate(async (rec) => {
        const resp = await fetch(`https://proxy-tau-ten-15.vercel.app/api/app?name=${rec.module}`);
        const bytes = await resp.arrayBuffer();
        const w = new Worker("/cartridge-worker.js");
        const out = await new Promise((resolve) => {
          const timer = setTimeout(() => resolve(null), 8000);
          w.onmessage = (e) => {
            if (e.data && e.data.type === "lib_call_result") {
              clearTimeout(timer);
              resolve({ status: e.data.status, result: e.data.result });
            }
          };
          w.postMessage({ type: "lib_call", wasm: bytes, fn: rec.call.export, args: rec.call.args }, [bytes]);
        });
        w.terminate();
        return out;
      }, okLine);
      check("receipts: one-shot lib_call re-execution CONFIRMS a recorded receipt",
        !!reexec && reexec.status === 0 && reexec.result === okLine.call.result,
        `recorded=${okLine.call.result} observed=${reexec && reexec.result}`);
    } else {
      check("receipts: found an OK receipt to re-execute", false, "none in ring");
    }
  }

  console.log("\npage errors:", pageErrors.length ? pageErrors : "(none)");
  check("no uncaught page errors", pageErrors.length === 0, pageErrors.join(" | "));
} finally {
  await browser.close();
  server.close();
}
finish();
