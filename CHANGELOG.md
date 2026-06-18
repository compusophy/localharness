# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
