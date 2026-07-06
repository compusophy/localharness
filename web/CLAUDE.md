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
fails ("module is not an object or function").

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
