// datasets/rustlite/export.mjs — emit the corpus as training-ready JSONL.
//
//   node datasets/rustlite/export.mjs [--out train.jsonl] [--stats]
//
// One line per pair: {"messages":[{system},{user},{assistant}]} — the chat
// shape most fine-tune stacks (incl. LoRA tooling) ingest directly. The system
// text pins the REAL contract (the compiler is the referee; the corpus was
// generated against it), so a tuned model learns the language boundaries, not
// just the shapes. --stats prints tag/size distributions and writes nothing.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const statsOnly = args.includes("--stats");
const outIdx = args.indexOf("--out");
const outPath = outIdx >= 0 ? args[outIdx + 1] : join(here, "train.jsonl");

const SYSTEM =
  "You write rustlite: a small Rust-subset compiled to wasm cartridges. " +
  "i32-only; no structs literals, unit-only enums, no globals (state slots " +
  "0..63 via host::display::state_get/state_set); export fn frame(t: i32) " +
  "(animated) or render() (one-shot) and call host::display::present() last. " +
  "Drawing: clear/set_pixel/fill_rect/draw_char(x,y,code,rgb,scale)/" +
  "draw_number(x,y,value,rgb,scale)/draw_line/fill_triangle; input " +
  "pointer_x/pointer_y/pointer_down; trig host::math::sin/cos (angle 1/256 " +
  "turn, result x256); audio host::audio; net/http/mp/agent/compose bridges " +
  "per the platform docs. Reply with ONLY the rustlite source.";

const rows = readdirSync(here)
  .filter((f) => /^\d{3}\.json$/.test(f))
  .sort()
  .map((f) => JSON.parse(readFileSync(join(here, f), "utf8")));

if (statsOnly) {
  const tags = {};
  let bytes = 0;
  for (const r of rows) {
    bytes += r.solution_rl.length;
    for (const t of r.tags ?? []) tags[t] = (tags[t] ?? 0) + 1;
  }
  console.log(`${rows.length} pairs · ${(bytes / 1024).toFixed(1)}KB of solutions`);
  const top = Object.entries(tags).sort((a, b) => b[1] - a[1]);
  for (const [t, n] of top.slice(0, 30)) console.log(`  ${String(n).padStart(3)}  ${t}`);
  process.exit(0);
}

const lines = rows.map((r) =>
  JSON.stringify({
    messages: [
      { role: "system", content: SYSTEM },
      { role: "user", content: r.prompt },
      { role: "assistant", content: r.solution_rl },
    ],
  }),
);
writeFileSync(outPath, lines.join("\n") + "\n");
console.log(`wrote ${rows.length} pairs to ${outPath}`);
