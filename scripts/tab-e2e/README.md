# scripts/tab-e2e — browser tab-E2E harness

Drives the SHIPPED web bundle in a real headless Chrome/Edge tab (puppeteer-core)
and asserts what a user actually sees. Zero network spend: the bundle is served
locally, every metered model call is intercepted (stalled or rerouted to a local
fake-Gemini SSE endpoint), and the tenant-sim aborts all RPC/proxy/apex requests.

## What it verifies

- **`tab-e2e-main.mjs`** — the Host::Other (localhost) full chat app:
  - boot to `data-lh-ready`
  - intent router: `/router status` reports ON (the default); `balance` answers
    FREE with an inline card ("routed free — no $LH spent"), no api-key modal,
    zero metered requests fired
  - bell: tap paints an honest push-state line immediately and after the async
    enroll resolves
  - display overlay opens on `display` and closes on ESC
  - inline-card sticky-header CSS (#30): plain card header `sticky`,
    `embed-app-card` header `static`
  - Stop turn-guard smoke: a metered send (throwaway fake BYOK key; model
    calls intercepted) engages a turn, the first Stop click acks within 300ms,
    `TURN_ACTIVE` is released (send button restored), and a follow-up message
    sends fine. Host::Other can't fire a real model call (the fail-closed
    pricing gate), so the mid-stream assertions live in `stop-e2e.mjs`.
  - no uncaught page errors
- **`stop-e2e.mjs`** — the Stop button genuinely MID-STREAM, as a TENANT:
  maps `teste2e.localharness.xyz` → 127.0.0.1, plants a synthetic `.lh_owner`
  hint in that origin's OPFS, blocks the chain RPC so verification fails
  transiently (optimistic studio kept), then streams one chunk from the local
  fake-Gemini endpoint and stops the turn mid-stream. Same ack/release/
  reconcile assertions.

Helpers: `serve.mjs` (static web/ server with wasm MIME, READ-ONLY),
`fake-gemini.mjs` (one SSE chunk then silence), `lib.mjs` (browser discovery,
bundle check, pass/fail tally).

## How to run

```sh
./scripts/build-web.sh                       # 1. build a bundle (web/pkg is gitignored)
npm install --prefix scripts/tab-e2e        # 2. pull puppeteer-core (the only dep)
node scripts/tab-e2e/tab-e2e-main.mjs        # 3. run (exit 0 = all checks passed)
node scripts/tab-e2e/stop-e2e.mjs
```

- **Browser**: set `CHROME_PATH` to a Chromium binary, else the harness probes
  the standard Windows/macOS/Linux install paths for Chrome and Edge and exits
  with a clear message if none exists.
- **Web root**: defaults to `<repo>/web`; override with an argv
  (`node ... path/to/web`) or `LH_WEB_ROOT`. The server never writes to it.
- **Ports**: 8792 (bundle) / 8793 (fake Gemini); override via `LH_E2E_PORT` /
  `LH_E2E_FAKE_PORT`.
- `puppeteer-core` (pinned in `package.json` here) drives an EXISTING local
  browser — it downloads nothing. `npx`/`npm exec` can't inject a library into
  `node`'s import resolution, so install it in this dir as above.

## Relationship to verify.sh

NOT a verify.sh stage — browser availability varies by machine, and the
proof-of-spec gate must stay hermetic. It is documented there as an OPT-IN
extension (like `verify-onchain.sh` / `verify-e2e.sh`); run it after touching
the chat turn loop, the intent router, Stop/TURN_ACTIVE, the bell, or the
display overlay — and before deploying a rebuilt bundle.
