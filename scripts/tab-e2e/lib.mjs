// Shared helpers for the tab-E2E harness (scripts/tab-e2e). See README.md.
import { existsSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

export const REPO_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

/// Resolve the web/ root to serve: argv[2] > LH_WEB_ROOT > <repo>/web.
export function webRoot() {
  return process.argv[2] || process.env.LH_WEB_ROOT || join(REPO_ROOT, "web");
}

/// Find a local Chromium binary: CHROME_PATH first, then the standard
/// Windows / macOS / Linux install locations for Chrome and Edge.
export function findBrowser() {
  const pf = process.env["ProgramFiles"] || "C:\\Program Files";
  const pf86 = process.env["ProgramFiles(x86)"] || "C:\\Program Files (x86)";
  const local = process.env.LOCALAPPDATA || "";
  const candidates = [
    process.env.CHROME_PATH,
    // Windows
    join(pf, "Google", "Chrome", "Application", "chrome.exe"),
    join(pf86, "Google", "Chrome", "Application", "chrome.exe"),
    local && join(local, "Google", "Chrome", "Application", "chrome.exe"),
    join(pf, "Microsoft", "Edge", "Application", "msedge.exe"),
    join(pf86, "Microsoft", "Edge", "Application", "msedge.exe"),
    // macOS
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    // Linux
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/microsoft-edge",
  ].filter(Boolean);
  const hit = candidates.find((p) => existsSync(p));
  if (!hit) {
    console.error("tab-e2e: no Chrome/Edge found.");
    console.error("Set CHROME_PATH to a Chromium binary (checked CHROME_PATH plus the");
    console.error("standard Windows/macOS/Linux install paths for Chrome and Edge).");
    process.exit(2);
  }
  return hit;
}

/// Fail fast with a clear message when web/ has no built wasm bundle.
export function requireBundle(root) {
  const wasm = join(root, "pkg", "localharness_bg.wasm");
  if (!existsSync(join(root, "index.html")) || !existsSync(wasm)) {
    console.error(`tab-e2e: no built bundle under ${root}`);
    console.error("(need index.html + pkg/localharness_bg.wasm; web/pkg is gitignored).");
    console.error("Build one first: ./scripts/build-web.sh");
    process.exit(2);
  }
  return root;
}

/// pass/fail tally: const { check, finish } = makeChecker("name");
export function makeChecker(suite) {
  let pass = 0, fail = 0;
  const failures = [];
  return {
    check(name, cond, extra = "") {
      const line = name + (extra ? " :: " + extra : "");
      if (cond) { pass++; console.log("PASS  " + line); }
      else { fail++; failures.push(line); console.log("FAIL  " + line); }
    },
    finish() {
      console.log(`\n=== ${suite}: ${pass} passed, ${fail} failed ===`);
      if (failures.length) { console.log("FAILURES:\n" + failures.join("\n")); process.exit(1); }
    },
  };
}

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function waitFor(fn, timeout = 8000, step = 100) {
  const t0 = Date.now();
  for (;;) {
    const v = await fn();
    if (v) return v;
    if (Date.now() - t0 > timeout) return null;
    await sleep(step);
  }
}
