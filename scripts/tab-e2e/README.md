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
- **`cartridge-e2e.mjs`** — the CARTRIDGE LOOP, no chain write / no publish:
  - fixture gate: `examples/cartridges/bouncing_ball.rl` compile-checks with
    the repo CLI (`localharness compile`; set `LH_CLI` to the binary, else it
    probes `target/release/` and skips honestly)
  - feed embed via the history-replay seam: a synthetic successful
    `create_and_publish_app` result planted in OPFS `.lh_history.json`
    replays as the playable `embed-app-card`, and the resume path recompiles
    the recorded source and BOOTS the cartridge worker into the card's canvas
    (`__lhEmbedTrace` + live `page.workers()` + two-frame animation diff)
  - local `app.rl` on Host::Other boots the fullscreen face via
    `try_paint_app` (in-browser rustlite compile), `[studio]` escape painted,
    still animating past the watchdog window (no LH1001 kill)
  - display overlay runs the fixture (`opfs-open app.rl` through the app's
    own delegated-action seam) and ESC genuinely TERMINATES the worker
    (count → 0), with zero telemetry/crash posts and zero metered calls
  - NOT covered (honest): the live tool-success auto-embed
    (`chat::stream_turn` → `launch_pending_embed` needs a real model turn
    calling the tool) and the tenant-only `#studio-app-slot` owner pin.

- **`seedpull-e2e.mjs`** — the seed-pull apex round-trip must not repaint a
  pure visitor's public face. Serves the bundle over LOCAL https on 443 with
  the REAL production URL shapes (`https://localharness.xyz` + a tenant, no
  ports — `seed_pull.rs` hardcodes them; throwaway openssl cert +
  `--ignore-certificate-errors`, hosts resolver-mapped to 127.0.0.1, chain RPC
  faked with the synthetic all-1s owner word, NO request interception — that
  would disable the bfcache under test, and prod-parity `must-revalidate`
  headers — `no-store` would too). Asserts: auto-kick → apex `?seed_export=1`
  → `history.back()` restore with the ORIGINAL face DOM untouched
  (`pageshow.persisted` + a data-stamp; zero repaint, no `?seed_import=none`
  leg, no "setting up this device…" interstitial), and that the boot.js fast
  bounce DEFERS to the wasm whenever an apex `.lh_wallet` exists (the
  owner-adoption safety property). `LH_E2E_EXPECT=baseline` flips the
  assertions to document the pre-fix behavior against an old bundle.

Helpers: `serve.mjs` (static web/ server with wasm MIME, READ-ONLY),
`fake-gemini.mjs` (one SSE chunk then silence), `lib.mjs` (browser discovery,
bundle check, pass/fail tally).

## How to run

```sh
./scripts/build-web.sh                       # 1. build a bundle (web/pkg is gitignored)
npm install --prefix scripts/tab-e2e        # 2. pull puppeteer-core (the only dep)
node scripts/tab-e2e/tab-e2e-main.mjs        # 3. run (exit 0 = all checks passed)
node scripts/tab-e2e/stop-e2e.mjs
node scripts/tab-e2e/cartridge-e2e.mjs
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
the chat turn loop, the intent router, Stop/TURN_ACTIVE, the bell, the
display overlay, or the cartridge loop (embed cards / history resume /
public-face boot / worker lifecycle) — and before deploying a rebuilt bundle.
