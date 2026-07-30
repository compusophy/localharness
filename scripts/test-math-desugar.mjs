// scripts/test-math-desugar.mjs — behavioral proof of the `host::math` codegen
// desugar (telemetry #83): compile a trig cartridge with the CLI, instantiate
// the wasm with a stub host, and assert exact table values BY EXECUTION —
// sin/cos land as a baked data-segment table + inline load, so this guards the
// table bytes, the wrap (& 255), cos's +64 phase, and the no-import invariant.
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const dir = mkdtempSync(join(tmpdir(), "lh-math-"));
const src = join(dir, "math.rl");
const wasm = join(dir, "math.wasm");
writeFileSync(
  src,
  `fn s(a: i32) -> i32 { host::math::sin(a) }
fn c(a: i32) -> i32 { host::math::cos(a) }
fn frame(t: i32) {
    host::display::clear(0);
    host::display::fill_rect(100 + host::math::cos(t), 100 + host::math::sin(t), 4, 4, 255);
    host::display::present();
}
`,
);

execSync(`cargo run --quiet --features wallet --bin localharness -- compile "${src}" "${wasm}"`, {
  stdio: ["ignore", "ignore", "inherit"],
});
const buf = readFileSync(wasm);

// No `math` import module may exist — the desugar's whole point.
const bytes = new Uint8Array(buf);
const needle = new TextEncoder().encode("math");
for (let i = 0; i + needle.length <= bytes.length; i++) {
  if (needle.every((b, j) => bytes[i + j] === b)) {
    console.error("FAIL: 'math' appears in the module — an import was registered");
    process.exit(1);
  }
}

const stub = new Proxy({}, { get: () => () => 0 });
const imports = new Proxy({}, { get: () => stub });
const { instance } = await WebAssembly.instantiate(buf, imports);
const { s, c } = instance.exports;

let failed = 0;
const check = (name, got, want) => {
  const ok = got === want;
  if (!ok) failed++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${name} = ${got} (want ${want})`);
};
// Quadrant landmarks, the 45-degree value linear approximations get wrong,
// the & 255 wrap (300 -> 44), and negative-angle wrap (-64 -> 192).
check("sin(0)", s(0), 0);
check("sin(32)", s(32), 181);
check("sin(64)", s(64), 256);
check("sin(128)", s(128), 0);
check("sin(192)", s(192), -256);
check("sin(300)", s(300), 226);
check("sin(-64)", s(-64), -256);
check("cos(0)", c(0), 256);
check("cos(32)", c(32), 181);
check("cos(64)", c(64), 0);
check("cos(128)", c(128), -256);
// Frame runs trap-free.
instance.exports.frame(0);
instance.exports.frame(1234);
console.log("frame() ran trap-free");

rmSync(dir, { recursive: true, force: true });
if (failed) {
  console.error(`\n${failed} check(s) failed`);
  process.exit(1);
}
console.log("\nPASS: host::math desugar — table values, wrap, cos phase, no import");
