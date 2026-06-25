# proxy — $LH credit proxy subsystem spec

> Module-owned context (auto-loaded when an agent works in `proxy/`). This is a
> SEPARATE Vercel project ("proxy") — the ONE deliberate off-chain component
> (everything else is Tempo + browser). TypeScript on Vercel Edge. Full prose:
> `proxy/README.md`.

## ⛔ SEPARATE DEPLOY — the gotcha that bites every time
`proxy/` is NOT shipped by the web/release deploys. After ANY change under
`proxy/api/`, you MUST `cd proxy && vercel --prod` and then TEST THE LIVE proxy.
A registry-side change that depends on the proxy (e.g. a new relay selector) is
INERT until the proxy is redeployed. Secrets are env-only (`LH_MAINNET_SPONSOR_KEY`,
Stripe keys, GitHub PAT) — NEVER in the wasm bundle.

## Endpoints (`api/`; `_`-prefixed = shared helpers, not routes)
- `gemini.ts` — multi-provider passthrough (Gemini/Claude/OpenAI). Auth = Ethereum
  personal-sign `address:timestamp:signature` in the `x-goog-api-key` header (5-min
  freshness window — a skewed device clock = stale-auth, not a bad key). Gates on a
  session OR `creditOf`, debits the meter BEFORE streaming, charges
  `min(cost,balance)` (a positive balance spends to zero). 1 `$LH`/message; fiat
  gross-mints at $1 = 100 `$LH`.
- `sponsor.ts` — the KEYLESS mainnet fee-payer RELAY: selector allowlist +
  onboarding-only gate (funded callers refused value-sponsorship → `LH_RELAY_FUNDED`)
  + `ALWAYS_FREE_SELECTORS` (gas-only, no-value: submitFeedback/register/releaseName/
  settle/transfer/setPushSub) + rate window + float breaker. The TS tx wire-port is
  PINNED to Rust golden vectors — keep them in sync.
- `scheduler.ts` — Vercel-Cron no-tab job worker (`vercel.json` `* * * * *`, 1-min);
  calls `recordRun` (SCHEDULER-ROLE, CAS-guarded). Sub-minute can't ride this.
- `notify.ts` + `_webpush.ts` — web-push (self or cross-agent `to`), dedup by the
  per-device `dev` field (NOT endpoint — one device's cross-origin endpoints collapse).
- `telemetry.ts` — files GitHub ISSUES in the private telemetry repo = the off-chain
  feedback/error task list (PRIMARY path now). `MAX_BODY_BYTES` mirrors the Rust clamp
  in `src/app/telemetry.rs` — keep them equal.
- `publish.ts` + `app.ts` — the OFF-CHAIN app store (cartridges live in GitHub, NOT
  on-chain; the chain keeps only the name's ownership). `publish.ts` (POST, personal-
  sign authed like telemetry) verifies the caller owns `name` on-chain (`ownerOf(
  idOfName(name))`) then commits `<name>/app.wasm` (+ `app.rl`) to `GH_APPSTORE_REPO`
  via the Contents API (`GH_APPSTORE_TOKEN` ?? `GH_TELEMETRY_TOKEN`). `app.ts` (GET
  `?name=`) re-serves the cartridge as `application/wasm` from raw.githubusercontent
  (public repo, no token) with CORS + 5-min CDN cache — the browser fetches it via
  `registry::app_wasm_from_store`. Mirrors the feedback/telemetry off-chain model:
  on-chain `setMetadata` publishing cost ~$0.32–$2.80/cart and drained the sponsor.
  EVERY app-cartridge publish path POSTs here now: CLI `publish`, the browser studio
  (`events/public_face.rs`), the agent tools (`chat/tools/platform.rs`:
  create_and_publish_app / publish_app_to / publish_public_face) + bashlite
  `lh-publish` — off-chain when the device's MASTER wallet owns the name (proxy
  re-checks `ownerOf`), with an on-chain fallback for TBA-owned names / linked
  devices. Only the HTML face + persona/lessons/x402-price metadata stay on-chain
  (not cartridge bytes). The wasm magic + 256 KB cap (host::compose per-child
  budget) are enforced server-side.
- `stripe-*.ts` — fiat on-ramp (Elements). The webhook once missed bare-PI
  `payment_intent.succeeded` (charged, no `$LH`); recovery = `contracts/script/
  MintForReceipt.s.sol`. READ Stripe docs before touching — guessing charged a card.
- `signal.ts` — WebRTC matchmaking rendezvous (GitHub-store, like jobs/chat). POST
  personal-sign authed; GET open. Slots: `offer`/`answer` (legacy 2-peer) +
  `offer-{joinerId}`/`answer-{joinerId}` + `join` roster (N-PEER STAR: host polls
  `join` to discover joiners, since the store can't list slots) + reserved
  `cands-*` (trickle ICE, deferred). `SLOT_RE` is the one gate every slot passes —
  widen it in lockstep with `webrtc.rs`. Blobs self-expire past `SIGNAL_TTL_SECS`.
- `turn.ts` — serves `{iceServers}`: STUN always + static TURN when `TURN_URLS` +
  `TURN_USERNAME` + `TURN_CREDENTIAL` env are set (Metered/Twilio/coturn), else
  STUN-only. **Vercel CANNOT host the TURN relay** (always-on UDP) — this only
  serves creds; the relay is external infra the operator provisions. `webrtc.rs`
  consumes it with a STUN fallback, so it's a regression-free env toggle.
- `chat.ts` — open-chatroom relay (GitHub-store). POST personal-sign authed (sender
  short-addr = name, no meter gate = open room); GET open. Backs `host::chat`.

## Test before deploy
`bash proxy/test/run.sh` (auth-parity, sponsor-handler incl. the funded-relay case,
webpush-dedupe) + `npx tsc --noEmit`. `node_modules` is gitignored (install locally
to run tests; don't commit it).
