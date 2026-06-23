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
  CLI `publish` (the app face) POSTs here; HTML face + the browser-studio/agent-tool
  publish are still on-chain (the follow-up flip). The wasm magic + 256 KB cap (the
  host::compose per-child budget) are enforced server-side.
- `stripe-*.ts` — fiat on-ramp (Elements). The webhook once missed bare-PI
  `payment_intent.succeeded` (charged, no `$LH`); recovery = `contracts/script/
  MintForReceipt.s.sol`. READ Stripe docs before touching — guessing charged a card.

## Test before deploy
`bash proxy/test/run.sh` (auth-parity, sponsor-handler incl. the funded-relay case,
webpush-dedupe) + `npx tsc --noEmit`. `node_modules` is gitignored (install locally
to run tests; don't commit it).
