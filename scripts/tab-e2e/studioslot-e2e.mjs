// Studio-slot pin tab-E2E: the LAST uncovered slice of the cartridge loop —
// the owner landing pins ONE playable card of this subdomain's own app above
// the chat history (`paint_tenant` → `mount_studio_app_card`,
// `#studio-app-slot`). Tenant-only, so it was unreachable on Host::Other —
// but the tenant-sim dance (host-resolver MAP + .lh_owner hint) reaches
// `paint_tenant`, and the card's resolution PREFERS the LOCAL `app.rl` draft,
// which needs zero network.
//
// Run: node scripts/tab-e2e/studioslot-e2e.mjs   (zero spend)
import { readFileSync } from "node:fs";
import { join } from "node:path";
import puppeteer from "puppeteer-core";
import { serve } from "./serve.mjs";
import { findBrowser, requireBundle, webRoot, makeChecker, sleep, waitFor, REPO_ROOT } from "./lib.mjs";

const PORT = Number(process.env.LH_E2E_PORT || 8801);
const ORIGIN = `http://teste2e.localharness.xyz:${PORT}`;
const ROOT = requireBundle(webRoot());
const CHROME = findBrowser();
const { check, finish } = makeChecker("studio-slot pin tab-E2E");

const FIXTURE = readFileSync(join(REPO_ROOT, "examples", "cartridges", "bouncing_ball.rl"), "utf8");

const server = await serve(ROOT, PORT);
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
    if (u.startsWith(ORIGIN) || u.startsWith("data:")) {
      req.continue().catch(() => {});
      return;
    }
    req.abort().catch(() => {}); // RPC / proxy / store: the card resolves LOCALLY
  });

  // Tenant-sim: owner hint + a LOCAL app.rl draft, then reload into the studio.
  // `?edit=1` forces the workshop (the app.rl would otherwise paint the
  // fullscreen public face on this ownerless-looking origin).
  await page.goto(ORIGIN + "/?edit=1", { waitUntil: "domcontentloaded" });
  await sleep(2500);
  await page.evaluate(async (fixture) => {
    const root = await navigator.storage.getDirectory();
    const w1 = await (await root.getFileHandle(".lh_owner", { create: true })).createWritable();
    await w1.write("0x1111111111111111111111111111111111111111");
    await w1.close();
    const w2 = await (await root.getFileHandle("app.rl", { create: true })).createWritable();
    await w2.write(fixture);
    await w2.close();
    localStorage.setItem("lh_model_access", "byok");
    sessionStorage.setItem("lh_seed_pull_tried", "1");
  }, FIXTURE);
  await page.reload({ waitUntil: "domcontentloaded" });
  const ready = await waitFor(() => page.evaluate(() =>
    document.documentElement.hasAttribute("data-lh-ready") && !!document.getElementById("prompt")), 30000);
  check("boot: tenant studio painted", !!ready);

  // THE PIN: one playable card in #studio-app-slot, compiled from the LOCAL
  // app.rl draft (prefer-local resolution), launched into ITS canvas.
  const slot = await waitFor(() => page.evaluate(() =>
    !!document.querySelector("#studio-app-slot canvas") || null), 20000);
  check("studio-slot: playable card pinned (canvas in #studio-app-slot)", !!slot);

  const trace = await waitFor(() => page.evaluate(() => {
    const t = globalThis.__lhEmbedTrace;
    return t && t.startsWith("launched into #studio-app-slot") ? t : null;
  }), 15000);
  check("studio-slot: cartridge launched into the SLOT's canvas (trace)", !!trace, JSON.stringify(trace));

  const worker = await waitFor(() => (page.workers().length >= 1 ? true : null), 10000);
  check("studio-slot: a live Web Worker runs the pinned cartridge", !!worker);

  const snap = () => page.evaluate(() => {
    const c = document.querySelector("#studio-app-slot canvas");
    return c ? c.toDataURL() : null;
  });
  const a = await snap();
  await sleep(400);
  const b = await snap();
  check("studio-slot: the pinned cartridge ANIMATES", !!a && !!b && a !== b);

  // Never auto-fullscreen: the display overlay must NOT have mounted.
  check("studio-slot: no fullscreen hijack (no #display-canvas)", await page.evaluate(() =>
    !document.getElementById("display-canvas")));

  console.log("\npage errors:", pageErrors.length ? pageErrors : "(none)");
  check("no uncaught page errors", pageErrors.length === 0, pageErrors.join(" | "));
} finally {
  await browser.close();
  server.close();
}
finish();
