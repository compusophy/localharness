# web — static site + worker runtime subsystem spec

> Module-owned context (auto-loaded when an agent works in `web/`). The Vercel
> static site (project "antig"; deploy = `vercel deploy --prod --yes` from the repo
> root). `index.html` is a near-EMPTY shell — ALL chrome/transcript/panels render
> from the wasm/maud templates and swap into `#root`; no app JS lives in the page.
> `pkg/` is wasm-pack output (gitignored).

## ⛔ The /pkg cache-buster is REQUIRED — `build-web.sh` stamps it, don't hand-edit
`max-age=0, must-revalidate` is NOT enough for wasm: Chrome's WASM code cache serves
a stale module for an unchanged URL (redeploys invisible until a hard reload).
`build-web.sh` stamps the wasm content hash as `?v=<hash>` on `boot.js` +
`stripe-embed.js` in `index.html`, AND inside `boot.js` on the shim import + the
EXPLICIT `init()` wasm url (the shim drops the query otherwise). So: change wasm →
`build-web.sh` (re-stamps) → commit `boot.js`/`index.html` → deploy. `styles.css`
is `max-age=0`+ETag (revalidated; no stamp needed).

## ⛔ cartridge-worker.js HAND-PORTS Rust — keep it in PARITY
`cartridge-worker.js` is the off-main-thread cartridge runtime (the brick fix: wasm
cartridges run in a Web Worker; a main-thread WATCHDOG kills hung workers). For
`host::compose` it's a TREE (every node owns a children/focus table;
`compositeChildren` recurses). `blitChild` / `mapPointerIntoChild` HAND-PORT
`src/compose.rs` and are PARITY-TESTED (`test-compose-wiring.mjs`, verify.sh stage
10) — edit BOTH sides together. `composeReset` MUTATES `rootNode` (never reassign —
`host_compose` closes over it). The cartridge host bindings mirror
`src/rustlite/loader.rs` (integer-only ABI) — add a host fn in BOTH or instantiation
fails ("module is not an object or function"). A new compose fn ALSO needs a key in
`INERT_COMPOSE`, or a node at the depth cap dies with "not a function".

**Call receipts ride the frame post.** Every `compose::call` that resolved a
READY child records `{uid, fn, args (exactly what was forwarded), result,
status}` into a per-frame batch (`RECEIPT_MAX_PER_FRAME`=256, drops counted)
flushed as `calls` on the frame message — NEVER per-call postMessage. The worker
stays HASH-FREE (keccak in JS would be a parity liability): the main thread
computed `keccak(bytes)` at `compose_bytes` time and joins on `uid`
(`src/app/display/bridge/receipts.rs` → `.lh_receipts.jsonl`, 256-line ring).
Which statuses receipt is the Rust SSOT (`receipt::CallStatus::
from_compose_status`) — the worker only skips the no-child cases (bad handle /
not ready / reentrant / budget). Shape parity: wiring test 8y1–8y5.
`oneShotLibCall` (message `lib_call` → `lib_call_result`) is the verify_receipt
re-execution primitive: same CALL_* semantics, run in a DEDICATED short-lived
worker the main thread spawns + terminates (never the shared slot — a hung
export dies with its own worker). Wiring 8z1–8z5 + tab-E2E re-confirmation.

**Callable libraries (`spawn_lib`/`call`/`call_ok`, telemetry #70).**
`instantiateChild` RETAINS `child.exports` — that retention is the whole feature;
it used to drop the exports object on the floor. A `lib` child mounts headless (no
`dims()`, no `fb`, skipped by `compositeChildren`, refused by `focus_module`) and
gets `INERT_COMPOSE` (a never-ticked node's children could never draw). ⛔ The
`call` host fn MUST keep its try/catch: trap containment INVERTS here — a trap
inside a host import unwinds through the CALLER's wasm frame and kills the whole
run (LH1002), where the composite walk would merely tombstone the child. Status
codes + `COMPOSE_MAX_CALLS_PER_FRAME`/`_CALL_ARGS` mirror `src/compose.rs` and are
parity-asserted in `test-compose-wiring.mjs` stage 8.

## CSP + headers (vercel.json)
CSP ships as `Content-Security-Policy-Report-Only` (logs, doesn't block) — validate
against the running app, THEN flip to enforce. **Do NOT add a Referrer-Policy** — a
stricter referrer was the suspected breaker of BYOK Gemini keys that carry
HTTP-referrer restrictions (commit c0393e0). Don't re-add without testing that path.

## Other
- `boot.js` seed-pull fast bounce: on the apex `?seed_export=1` leg with NO
  `.lh_wallet` in OPFS (definitive NotFoundError only), `history.back()` BEFORE the
  wasm loads — the visitor's subdomain face restores from bfcache with zero repaint.
  `.lh_wallet` mirrors `wallet_store.rs`; parity guard `tests/seed_pull_boot_parity.rs`.
  Any doubt falls through to the wasm path (owner adoption never rides this branch).
- `sw.js` — service worker: push → `push_arrived` → bell; ALWAYS `stashPending` so a
  closed-tab push still lands in the inbox.
- Design tokens come from `src/app/style.rs` (Rust SSOT), injected as `:root` — use
  `var(--…)` in `styles.css`, never hardcode a color (monochrome brutalist).
- `index.html` viewport: `viewport-fit=cover` + `interactive-widget=resizes-content`
  (the keyboard fix is finished by the visualViewport handler in `src/app/events`).
