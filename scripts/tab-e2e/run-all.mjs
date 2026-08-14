// Run EVERY tab-E2E script sequentially (each owns its ports/browser) and
// report one summary. All zero-spend by construction; needs a built web/pkg
// + a local Chrome/Edge. Opt-in, never a verify.sh stage (see README.md).
//
//   node scripts/tab-e2e/run-all.mjs [filter-substring]
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const filter = process.argv[2] || "";
// ⛔ A bare `endsWith("-e2e.mjs")` silently DROPPED `tab-e2e-main.mjs` — the
// suite covering boot, the intent router's free tiers, the bell, the display
// overlay and the #30 sticky-header CSS. "run the whole lattice" quietly ran
// 7 of 8 for as long as this file has existed. Match the suite names, not a
// naming accident.
const isSuite = (f) => f.endsWith("-e2e.mjs") || f === "tab-e2e-main.mjs";
const scripts = readdirSync(here)
  .filter((f) => isSuite(f) && f.includes(filter))
  // Deliberately EXCLUDED from the default sweep: environment-specific probes
  // (`webkit-opfs-probe.mjs` never matched `isSuite` — kept as documentation).
  .filter((f) => !["gemma-e2e.mjs", "gemma-stream-e2e.mjs", "webkit-opfs-probe.mjs"].includes(f))
  .sort();

if (scripts.length === 0) {
  console.error(`no tab-e2e scripts match "${filter}"`);
  process.exit(2);
}

const results = [];
for (const s of scripts) {
  console.log(`\n━━ ${s} ━━`);
  const r = spawnSync(process.execPath, [join(here, s)], { stdio: "inherit" });
  results.push([s, r.status ?? 1]);
}

console.log("\n=== tab-e2e sweep ===");
let failed = 0;
for (const [s, code] of results) {
  console.log(`${code === 0 ? "PASS" : "FAIL"}  ${s}`);
  if (code !== 0) failed++;
}
process.exit(failed ? 1 : 0);
