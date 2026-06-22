# Agenda — forward roadmap (captured 2026-06-22)

Live brain-dump from the maintainer, triaged into actionable / ideation / parked,
with design notes so the next session executes fast. This is the FORWARD agenda;
`cleanup-backlog.md` is the cleanup queue; `feedback-resolved-mainnet.txt` is the
shipped log; `rawfeedback.txt` (gitignored) is the raw inbox awaiting triage.

## Next up (do WITH the user — browser-verify needed, same code area)

### Cartridge auto-error-reporting  (greenlit)
The app already auto-reports TURN errors (`src/app/telemetry.rs` `report()` /
`signature_for()`, panic hook in `debuglog.rs`, admin toggle). Cartridge errors do
NOT report — they only paint the "CARTRIDGE STOPPED" overlay (watchdog/brick fix).
- **Hook point:** since #52a moved cartridges INLINE, a failure surfaces as
  `worker::RunOutcome::Failed { code, detail }` observed by the inline card's
  worker + the watchdog (`display.rs` ~1285 `paint_stopped_overlay_coded`), NOT in
  `run_wasm_reporting` (which now just stashes bytes). Wire the telemetry call at
  the inline-card Failed observation (one spot; dedup by `LH1xxx` code so a
  crashing cartridge doesn't spam), gated by `telemetry::enabled()`, fired via
  `spawn_local(report("cartridge-error", title, sig, body))`.
- **Verify (browser):** crash a cartridge → overlay still paints AND one telemetry
  report fires (check the telemetry repo / network), not N per frame.

### Cartridge resumability bug  (user-reported)
"Open a cartridge → close the app → reopen → it's dead / CARTRIDGE STOPPED."
- **Cause:** cartridges run in a Web Worker (terminated on page unload). On reopen,
  history replay repaints the inline card but the worker is NOT re-spawned and the
  wasm bytes aren't persisted → black/dead canvas. (The #52a agent flagged exactly
  this: "history replay paints the card (black canvas, no stashed bytes)".)
- **Fix direction:** persist the cartridge's wasm bytes (or an OPFS-backed ref +
  re-fetch) keyed to the card, and on replay re-launch the worker (or show a tap-to-
  resume affordance) instead of a dead canvas. Ties into the broader resumability
  ideation below — do the cartridge-scoped fix first.
- **Verify (browser):** run a cartridge, reload, confirm it resumes (or offers
  resume) rather than showing CARTRIDGE STOPPED.

## Ideation (parked, but captured — "something to think about")

### Resumability / instant-FCP / state-serialization (platform-wide)
Maintainer likes Qwik's resumability (serialize state into the HTML; a tiny
bootloader lazy-loads the rest → fastest-possible FCP) but hates JS. Question:
can we get Qwik-style takeaways in our Rust/HTMX/no-full-DOM-rewrite world, for the
whole platform + subdomain pages + chat app + `.rl` cartridges — via Rust
serialization (serde)? Today: conversation history already gives good local
resumability; the boot does icon → loading → loading-agents (re-auth + refresh real
platform data on every load — probably correct, but optimization lying around).
- Not urgent; "don't break anything." Threads: (1) serialize+restore app state
  (serde) to skip re-fetch on reopen; (2) the cartridge resumability fix above is
  the first concrete instance; (3) subdomain public-face pages could ship
  pre-serialized state for instant paint.

### Composable end-state — "recreate localharness from inside a cartridge"
Grow the host-function surface a cartridge can call (it already has net/http,
host::compose cartridge-in-cartridge, display, bashlite `lh-*`) until a cartridge
can do what the host does → an app that grows/evolves itself, end-to-end, enabling
rapid self-iteration (not just games — the platform building the platform). This is
the north star behind the meta-tool layer (skills / set_persona / lessons /
create_and_publish_app / rustlite).

## Parked (deliberate — revisit on trigger)

- **Gemma** — shipped behind `browser-app-local` but dormant. Revisit when doing
  fine-tuning (the `datasets/rustlite-problems` RLVR corpus is the moat). Likely
  first use: cheap on-device SEMANTIC work (intent pre-classification, feedback
  dedup/embeddings), not primary inference (270M too small for tool-routing/codegen).
- **Auto-feedback-to-me** — HELD on purpose: piping new feedback straight to the dev
  short-circuits the self-evolving loop. The system should eventually close its own
  loop (observe → file → fix → verify), free of the developer — the "born" stage.
- **Gemini-drafts-PR → Opus-reviews loop** — fine to skip for now. For REPO code,
  Opus-from-scratch (feedback as spec) beats reviewing a weak Flash Rust draft.
  Draft-then-review is a win for apps/cartridges + the feedback→issue bridge.

## Audit note (2026-06-22)
Swept for stale repo waste: the obvious cruft was already cleared this session
(legacy contracts → `contracts/archive/`, CLI glob/allow cleanup). The remaining
"old" files are load-bearing or deliberate, NOT waste — `RELEASING.md` (the
release-failure recovery runbook, referenced by `release.{sh,ps1}` + CLAUDE.md) and
`datasets/` (the fine-tune RLVR moat). No safe deletions outstanding.
