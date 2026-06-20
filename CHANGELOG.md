# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.50.0] - 2026-06-19

### Added

- **bashlite + localharnesslite — a tiny sandboxed shell for scripting the
  platform.** `execute_script(source)` (browser) and `localharness sh
  <script.bl>` / `sh -c '<inline>'` (CLI) run a deterministic, fuel-bounded shell
  (lexer → parser → eval over a `BashHost`; `src/bashlite/`, `design/bashlite.md`):
  variables + `$VAR`/`$?` interpolation, pipes, `&&`/`||` short-circuit, `if`/
  `while`/`for` (with `$( )` field-split), `[ … ]` tests incl. `-e`/`-f`/`-d` file
  tests, `head`/`tail`, and `run`/`source FILE.bl` for fractal (fuel-bounded)
  script composition. The fs builtins (`echo ls cat wc grep find mkdir
  write`) are create-only over the sandbox `Filesystem`. The CLI additionally
  wires **localharnesslite** `lh-*` reads — `lh-whoami`, `lh-balance`,
  `lh-meter`, `lh-resolve`, `lh-tba` (an agent's token-bound account / payment
  target), `lh-price`, `lh-list`, `lh-discover` (find agents), `lh-bounties`
  (find paid work), `lh-help` — plus the value-moving `lh-send` behind a
  dry-run-manifest confirm gate (the script runs DRY first, prints every `$LH`
  move, and only `--confirm` executes).

### Fixed

- `grep -c` now preserves grep's match-based exit status (a zero count exits 1,
  not 0), so `grep -c x && …` no longer fires on no matches.
- Compaction's `should_compact` no longer mis-fires on a negative token delta (an
  `i32`→`u32` wrap); a negative delta is treated as below-threshold.
- Gemini `RECITATION` and OpenAI `content_filter` empty turns are classified as
  `Blocked` (a clear "blocked" message) rather than a generic empty-turn error.
- `execute_script`'s tool docs no longer claim `&&`/`||`/`run`/field-split are
  unsupported — they ship.
- The CLI relay verifier requires the proxy to return `feePayerHash`/`feePayer`,
  closing a silent-bypass path in `registry::sponsor_relay`.

### Internal

- Public-contract + critical-plumbing test coverage: `ToolRunner`
  (overwrite / not-found / batch id-mapping), `Content`/`Media` (MIME validation
  + the `from_path` extension↔MIME table-consistency invariant), the shared
  `dispatch_tool_call` error-lift, the SSE frame decoder, and the stream
  idle-timeout wrapper. Drift guards assert every `lh-*` command appears in both
  `lh-help` and the CLI `USAGE`. Clippy clean across the `wallet` / `anthropic` /
  `openai` / `browser-app` feature matrices (default `cargo clippy` exercises
  none of them).

## [0.49.0] - 2026-06-19

### Added

- **Rate-capped sponsor relay — the published crate ships NO mainnet money key.**
  On mainnet, sponsored Tempo `0x76` writes now have their `fee_payer` half signed
  SERVER-SIDE by the credit proxy (`registry::sponsor_relay`) instead of an
  embedded key. The submit chokepoints in `registry::tx` — and the browser's
  self-assembled `run_sponsored_tempo_call` — route through it when
  `chain::active()` is mainnet, authed by the caller's existing personal-sign
  proxy token and re-verified locally (the relay's returned fee_payer hash must
  match the one the client recomputes; the signature must recover to the
  advertised sponsor). The relay endpoint (`proxy/api/sponsor.ts`) enforces a
  default-deny selector allowlist, a per-address rate window, an onboarding-only
  spend gate, and a low-float circuit-breaker before signing; its TS wire-port
  (`proxy/api/_tempo.ts`) is pinned to the Rust 0x76/0x78 golden vectors.
  `registry::is_mainnet()` is now public so both submit paths branch the same way.

### Changed

- **No build embeds a mainnet sponsor key.** `main.rs::sponsor_key()` dropped its
  `LH_MAINNET_SPONSOR_KEY` env arm and `src/app/sponsor.rs` dropped the
  `env!("LH_MAINNET_SPONSOR_KEY")` compile-time embed — both now return the
  committed testnet key as an UNUSED placeholder on mainnet (the relay signs the
  `fee_payer` half), so no crate build or web bundle carries a money-moving
  mainnet key. `build-web.sh` no longer requires the key.
- The x402 + mint-gate EIP-712 domain tests are chain-agnostic (no longer gated on
  the `mainnet` feature; both chain presets have a non-empty diamond).

## [0.48.0] - 2026-06-18

### Added

- **In-browser Gemma backend reachable from the app** via a new
  `browser-app-local` composite feature. The in-tab local Gemma 3 270M path
  (model selector entry, OPFS download, `start_local` session wiring, the
  `Connection`/`ConnectionStrategy` impl with a `tool_code`-fence tool loop) was
  already built behind the `local` feature but never enabled in any bundle;
  `browser-app-local = ["browser-app","local"]` is the opt-in heavy bundle that
  turns it on. The default web bundle stays lean (Gemma OFF) and no longer
  advertises a dead "Local (Gemma)" selector entry that errored on select. Build
  with `--features browser-app-local`. Forward-pass validation + a live WebGPU run
  remain before it generates coherent text in a tab.
- **CLI runtime chain selection via `LH_CHAIN`** (CLI-mainnet relay phase 1) — one
  published `localharness` binary targets testnet OR mainnet at runtime
  (`LH_CHAIN=mainnet` → chain 4217, else Moderato testnet), resolved once via a
  `OnceLock`, replacing the compile-time `--features mainnet` requirement. The
  binary embeds NO mainnet money key: `main.rs::sponsor_key()` reads
  `LH_MAINNET_SPONSOR_KEY` from the env at runtime only when the active chain is
  mainnet (fail-loud if unset). wasm stays compile-time `cfg`. Verified live
  (default build: `claude` resolves tokenId 8 on testnet, tokenId 2 on mainnet).
- **Multi-chain EVM READ tools** so an agent checks balances / reads contracts /
  resolves ENS on OTHER EVM chains natively instead of `web_fetch`-ing
  third-party explorer APIs. New `registry::multichain` module: a curated,
  CORS-enabled public-RPC table (ethereum, base, optimism, arbitrum, polygon,
  tempo) + generic per-chain `eth_call` / `eth_getBalance` (mirrors `rpc.rs` but
  keyed on a per-chain URL), EIP-137 `namehash`, ENS forward resolution, and a
  human-signature ABI encoder — all READ-ONLY, no writes/signing. Four new browser
  agent tools (`src/app/chat/tools/evm.rs`): `evm_balance(chain, address,
  token?)`, `resolve_ens(name)`, `evm_call(chain, to, function_signature, args?)`,
  `evm_chains()`. Returned chain data is treated as untrusted. Native-tested
  (namehash canonical vector + ABI encoding); live-verified against Base/Ethereum.
- **CLI sandbox (`run_wasm_cli`) — run compiled wasm CLI programs in-browser
  (on-chain feedback #6).** A WASI-SUBSET runtime (`web/wasi-worker.js`) runs a
  `wasm32-wasi` COMMAND (a module exporting `_start`) off-main-thread, capturing
  its stdout/stderr as monochrome terminal text (a fullscreen overlay + an
  inline transcript card with the argv line, stdout, and exit code). The new
  `run_wasm_cli(path, args?)` chat tool reads a `.wasm` from OPFS, runs it under
  the host with a ~4s watchdog, and returns `{ ran, exit_code, stdout, stderr,
  truncated, argv }`. Implemented WASI subset: `fd_write` (stdout/stderr →
  captured text), `proc_exit`, `args_*`, `environ_*` (empty), `fd_read` (stdin =
  EOF), `clock_time_get`, `random_get`, plus defined-errno stubs for the wider
  surface. **Honest boundary:** a WASI-subset *stdout* sandbox — NOT a real
  filesystem (no preopened dirs), no network, no threads, and NOT an x86 PC /
  Linux container (which would need iframes + multi-MB blobs, against this
  project's design). Committed example `examples/cli/hello.wasm` (+ `.wat`
  source); `scripts/verify-wasi-cli.mjs` runs it through the host in node.

### Changed

- **BREAKING (`registry` surface): canonical chain handles are now functions, not
  consts.** `REGISTRY_ADDRESS`, `CHAIN_ID`, `RPC_URL`, `LOCALHARNESS_TOKEN_ADDRESS`,
  `ALPHA_USD_ADDRESS` and `multichain::CHAINS` became `REGISTRY_ADDRESS()` …
  `chains()` so they can resolve the active chain at runtime (see `LH_CHAIN`
  above). Downstream `default-features = false, features = ["wallet"]` consumers
  must add `()` at the call site.
- **Token-usage metering flipped LIVE on the credit proxy.** The live debit is now
  `max(flat_floor, usageCostWei × 1.3×)` (per-provider input/output/cached token
  rates) instead of the flat per-request floor, gated on `LH_TOKEN_METERING=1`
  (now set in prod). Floor-clamped, so it only bills above the floor on outlier
  mega-requests; rollback is removing the env + redeploy. `LH_MARGIN_BPS` (1.3×)
  and `LH_MAX_OUTPUT_TOKENS` (8192) keep safe defaults. Verified live against both
  the Gemini and Anthropic paths.

### Fixed

- **Metering under-counted Gemini output by up to ~66% on thinking-heavy calls.**
  Gemini 3.x bills reasoning tokens at the output rate but reports them in a
  separate `thoughtsTokenCount`; `proxy/api/_usage.ts::extractUsage` counted only
  `candidatesTokenCount`. Folded `thoughtsTokenCount` into `outputTokens` (mirrored
  in `scripts/test-metering-usage.mjs` with the live-verified 35/2224/1481 case).
  Masked by the 1 `$LH` floor at today's prices; matters when floors drop.

## [0.47.0] - 2026-06-18

### Changed

- **`$LH` is decoupled from the dollar — it is NOT a stablecoin.** Pricing is now
  flat and legible: **1 `$LH` = 1 message** on the default model, premium models
  tiered (more `$LH`/message). Fiat mints on the **GROSS** at **$1 = 100 `$LH`**
  (Stripe fees absorbed), so a $2 buy is a round 200 `$LH`. A positive meter
  balance is now **spendable down to zero** — a sub-one-message leftover (e.g.
  0.62 `$LH`) is no longer stranded; the credit path debits `min(cost, balance)`.
- **Onboarding is pay-first.** The fresh-visitor front door is one **"create
  agent"** button with the offer up front (1 agent + 200 `$LH` for $2) — no
  surprise paywall after a visitor has invested in picking a name. The seed is
  held **in memory only until payment confirms**, then persisted and offered as a
  downloadable/printable backup at the safest moment. Reset is now a true wipe.
- **Fiat on-ramp rebuilt on inline Stripe Elements — card only.** The buy-`$LH`
  form mounts **in place** (the apex onboarding card and the admin → account
  buy-`$LH` area — no popup modal), restricted to **card** (no Link/bank/Klarna),
  and confirms via the canonical Payment Element `clientSecret` flow. Minting is
  **webhook-backed**: the proxy mints `$LH` server-side on
  `payment_intent.succeeded`, so the credit lands even if the browser tab closes.
- **Admin/account panel simplified to identity + credits.** Treasury (TBA send),
  scheduling, bounties, guilds, and guild governance/DAO are **out of the panel** —
  that coordination is driven from chat via agent tools now. The panel keeps
  buy-`$LH`, redeem, invites, notifications, identity, devices, security, persona,
  and public-face.

### Added

- **`schedule_task` agent tool** — an in-tab agent can escrow `$LH` for recurring
  or delayed runs (durable via `ScheduleFacet` + the cron worker), no panel needed.
- **Per-agent PWA install identity** — installing a subdomain as a PWA installs it
  as **its own** app (e.g. "krafto"), not "localharness"; the apex stays
  "localharness".
- **Post-payment seed backup** — the just-paid identity's recovery phrase is shown
  with copy + download the moment it's safe, so a device loss can't strand it.

### Fixed

- **On-ramp charged a card but credited no `$LH`.** The Stripe webhook endpoint
  was subscribed to `checkout.session.completed` (the old hosted-Checkout flow),
  not `payment_intent.succeeded` (the Elements bare-PaymentIntent flow), so the
  durable mint backstop never fired. Fixed the subscription; added
  `contracts/script/MintForReceipt.s.sol` to idempotently re-mint a
  confirmed-but-unfulfilled payment.
- **Checkout hung on "processing…".** `lhBuyLh` called `lhUnmountCheckout()` on
  mount, tearing down the success watcher the Rust side armed immediately after
  (microtask ordering) — so the success handler always short-circuited and the
  button never resolved (the payment still succeeded + credited). Confirmation now
  drives off `confirmPayment`'s own result via the canonical flow, with the JS
  status poll as a redirect/late-settle backstop.
- **iOS create-wallet crash ("RefCell already borrowed").** A concurrent
  storage-volatility probe raced the first OPFS seed write, re-entering the
  wasm-bindgen single-thread executor. The OPFS root-handle borrow is now
  await-safe and the probe sequential.
- **iOS ~10s checkout reset.** The wasm payment-status poll (JsFuture + timer loop)
  re-entered the executor on iOS WebKit; the poll now runs in JS.

### Security

- **Owner-gated `lh-open-key`.** A visited (non-owned) subdomain could ask the apex
  signer to decrypt the visitor's stored LLM key; the open-key challenge is now
  owner-gated so only the seed holder's own surfaces can decrypt it.

### Removed

- The economy-coordination panel UI (`events/{tba,bounty,guild,governance}.rs`
  plus their sections/actions) and the popup buy modal — superseded by the inline
  checkout and chat-driven agent tools.

## Older versions

Versions **0.46.0 and earlier** are archived in
[`docs/CHANGELOG-archive.md`](docs/CHANGELOG-archive.md).
