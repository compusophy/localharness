# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.58.0] - 2026-06-27

Correctness + reliability on the back of the 0.57.0 audit: signature-malleability
hardening, transient-error retry on the main turn, an in-feed rendering fix, and the
cartridge proofs now gating every push.

### Security

- **Reject high-s (malleable) signatures in address recovery (I3).** `recover_address`
  (and the proxy's `_authcore.ts` / `_tempo.ts` verifiers) accepted EIP-2-malleable
  high-s signatures — the `(r, n-s, v^1)` twin recovers the SAME address, so a token or
  intent signed high-s could pass off-chain auth even though the on-chain HALF_N gate
  (X402Facet / MultiSignerAccount) rejects it. Low-s is now enforced on every path
  (k256 `normalize_s` in Rust; `s <= secp256k1n/2` in TS). Our own signers already emit
  low-s, so legitimate auth is unaffected.

### Fixed

- **Retry the main turn stream-open on transient failures (#29).** A Gemini HTTP 503
  aborted the whole turn because the gemini/anthropic turn loops opened their model
  stream once, while the subagent already retried. A shared `backends::retry` policy
  now retries transient transport/5xx/timeout stream-opens (auth/credits/rate-limit
  fail fast; a mid-stream failure is not retried — the partial response already went out).
- **In-feed cartridge embeds respect their aspect ratio (#30).** The inline embed stage
  hard-coded 4:3 (the obsolete 320x240 default), letterboxing 512x512 / portrait
  cartridges; it now follows the cartridge's own `dims()`, capped in height so a tall
  app stays inline.

### Added

- **CI gates the cartridge corpus + codegen/host-parity proofs.** A rustlite compiler
  panic reached `main` before 0.57.0 because CI ran clippy/test/wasm/proxy but not the
  cartridge proofs; a `cartridges` job now compiles + runs every `examples/cartridges/*.rl`
  (plus variable-resolution, host::compose wiring, worker host-parity) on every push.

## [0.57.0] - 2026-06-27

A security-hardening release: the 2026-06-26 whole-codebase audit (76
adversarially-verified findings) plus route-bound proxy auth, GitHub Actions CI,
and a chain-aware sponsor-relay fee-token fallback. Downstream SDK consumers get the
audit hardening for the first time here — the web/proxy/contract halves already
reached production via their own deploys.

### Security

- **Whole-codebase audit hardening (76 findings).** Seed-adoption KDF hardened — the
  receiver strips the `#s=<ct>` fragment from history, `adopt_code_key` is now
  200k-iterated keccak, and pairing codes widened 6→8 chars (H1). The MPP fiat
  on-ramp mints to the PROVEN on-chain USDC.e payer, never a caller-supplied address
  (H2). The sponsor relay validates `intent.feeToken` against the chain's canonical
  fee token (L5). Signaling/chat endpoints rate-limited + signal-slot anti-forgery;
  notify recipient window moved post-auth; global telemetry cap; cross-origin signer
  default-deny; `?rpc=1` owner-gated; P2P `"p"`-tag spoof closed; `RootedFilesystem`
  backslash escape fixed; subagent hook/policy inheritance enforced (M8); the bashlite
  confirm-gate bound to the approved plan; secret redaction hardened against prefixes.
- **Route-bound proxy auth tokens (L9/L10/L7).** Personal-sign auth tokens now bind
  the target endpoint (`localharness-proxy:<addr>:<ts>:<route>`) — a token minted for
  a cheap route can no longer be replayed against a high-value write
  (publish/schedule/sponsor/mint). A single `proxy/api/_authcore.ts` primitive
  verifies with dual-accept (route-bound, falling back to the legacy unbound message)
  so un-migrated clients keep working; the divergent verifier copies in
  `_stripe.ts`/`sponsor.ts` are deleted (ONE auth implementation). Every Rust +
  inline token minter binds its route.
- **Contracts (source + tests).** Removed the inert `chargeFromWallet` (L13); clear
  MAIN on ERC721 transfer (L14); `MultiSignerAccount` fails closed on a zero owner
  (I6); `SignalingFacet` announce/leave digests are chain-bound (I7). Live diamond
  cuts are applied out-of-band.

### Fixed

- **Chain-aware sponsor-relay fee-token fallback.** With `FEE_TOKEN` unset and
  `CHAIN_ID=4217`, the relay's hard-coded AlphaUSD default mismatched the USDC.e the
  browser/CLI send, 400'ing every sponsored register/setMetadata. The fallback is now
  chain-aware (4217 → USDC.e, else AlphaUSD), mirroring `src/registry/chain.rs`; an
  explicit `FEE_TOKEN` still wins.
- **Correctness pass (from the audit).** rustlite branch/operand type agreement,
  diverging-block `Never`, enum discriminants, UTF-8 string lexer; soliditylite emits
  clean `CompileError`s over panics; OpenAI loop cancel-balance + finish summary;
  scheduler wall-clock deadline / billing / reminder-priority; at-rest key re-derive
  on in-tab re-import; notification cursor; escrow withdrawable.

### Added

- **GitHub Actions CI.** `ci.yml` runs the gate on push/PR — clippy across the 4
  shipped feature configs, the full test suite, the wasm32 guard, and the proxy `tsc`
  + tests. `release.yml` records a `crates-io` GitHub Deployment on each `vX.Y.Z` tag.

### Changed

- Deduplicated to the canonical hex/ABI codecs; default framebuffer 512×512 in the
  system prompt + self-docs; clippy clean across all shipped feature configs on
  stable 1.96.

## [0.56.0] - 2026-06-25

Multiplayer cartridges land — real-time browser-to-browser WebRTC — alongside a
full off-chain pivot for apps + scheduling and a 4× bigger cartridge budget.

### Added

- **Multiplayer cartridges (`host::mp`).** A rustlite cartridge becomes real-time
  multiplayer over a P2P WebRTC data channel: `open()` / `join()` / `auto()` a
  room, a 32-slot shared-state vector per peer (`set`/`get`) + discrete events
  (`send`/`event_*`). Three topologies: 2-peer, an N-peer host-authoritative STAR
  (the host relays peers to each other), and a hostless FULL P2P MESH (`auto` — one
  shared room, no sticky host; a departing peer just frees its slot). A SEPARATE
  unreliable/unordered data channel (id 1) carries fast game state while the
  reliable channel (id 0) stays for shared-fs sync. Signaling is off-chain (the
  proxy's `/api/signal`, GitHub-store); cross-network peers use a TURN relay
  (`/api/turn`, env-provisioned). Demos: `slither.localharness.xyz` (a 512×512
  multiplayer slither.io) + a 2-player Pong.
- **Open-chatroom cartridges (`host::chat`).** An integer-only ABI to a
  per-subdomain append-only message log over the proxy's `/api/chat` relay — the
  `groupchat` reference room.
- **Off-chain app store.** Cartridges + HTML faces publish to GitHub (free, no
  gas) instead of on-chain `setMetadata`; the chain keeps only name ownership.
  `/api/apps` catalog + `localharness apps` discovery; every publish path (CLI,
  studio, agent tools, bashlite) goes off-chain.
- **Off-chain scheduling.** `remind` / `schedule` / `goal` run off-chain (free
  reminders, meter-billed agent runs) — no on-chain escrow per job.
- **Cartridge crash telemetry** that names WHAT crashed (reproducible reports).

### Changed

- **Cartridge size cap 256 KB → 1 MB** — off-chain storage has no gas cap (the
  GitHub Contents-API full-support ceiling).
- **Default cartridge resolution 320×240 → 512×512.**
- README is a minimal hand-written file again (decoupled from `skill.md`).

### Fixed

- A cartridge now resumes on reopen instead of showing a dead canvas.
- The fullscreen public-face header used a legacy text brand menu — modernized to
  the current icon design (kept visitor-slim, no owner-tool leak).

## [0.55.0] - 2026-06-23

Feedback-ingest pass clearing the remaining open feedback (#62 / #63 / #64 +
rawfeedback) and a repo-wide CLAUDE.md decomposition.

### Added

- **rustlite `draw_string`.** `host::display::draw_string(x, y, "TEXT", color,
  scale)` lowers (parser desugar) to one `draw_char` per glyph — no more verbose
  char-by-char arrays — with no new host import (integer-only ABI intact).
- **Receive-side `$LH` notification.** An agent now gets a bell note when it
  *receives* an on-chain `$LH` transfer (balance-delta watcher), not only when it
  sent one — covers CLI / external-wallet / x402 / payout transfers.
- **Per-directory CLAUDE.md specs.** Every subsystem (10 `src/` module dirs +
  `proxy` + `contracts` + `web` + `scripts`) owns a nested `CLAUDE.md`
  auto-loaded in-dir; the root is a pure map + index, back under the 40K cap.

### Changed

- **Unified header modals.** The notifications / feedback / admin overlays are one
  system now — centered on the viewport (no off-screen overflow), one z-layer,
  mutually exclusive (opening one closes the others), each a real toggle.
- **Mobile-first by default.** A desktop browser renders the 9:16 phone frame by
  default (toggle to "desktop view"); a real phone is unchanged. Removed the frame's
  vertical edge lines.
- **Subagent network resilience.** `start_subagent` retries the model stream on
  transport / 5xx / timeout (auth / credits / rate-limit still fail fast).

### Fixed

- **No double-box on focused inputs.** A global `:focus-visible` outline was drawing
  a second highlighted box inside the field's own focus border (chat, feedback, and
  the apex claim input); the feedback popover is now a single box with the *field*
  highlighting, not the container (#64 / rawfeedback).
- **No empty-bubble flicker.** The pending assistant bubble paints an immediate
  "starting" cue instead of a blank bubble before the first stage (mobile).
- **Enabling notifications no longer 403s a funded account.** `setPushSub` is
  gas-only and now relay-exempt (`LH_RELAY_FUNDED` was wrongly refusing it).
- **No double notifications.** Push delivery dedups by a per-device id, so notifying
  a main no longer buzzes once per same-device subdomain endpoint.
- **Notification inbox reliability.** The service worker always stashes a push, so a
  closed-tab note still lands in the bell on next open.
- **Relay sponsors `releaseName`** (selector `0x48e69e68`) so bulk subdomain release
  works (#62).
- **External spend-cap 429s no longer spam telemetry** — a Gemini monthly-cap 429 is
  the operator's billing condition, not a platform bug, and is suppressed.

## [0.54.0] - 2026-06-22

Chat-UX pass clearing the open on-chain feedback (#58 / #59 / #60).

### Added

- **Brand menu as icon buttons.** Tapping the `lh` mark drops a vertical stack of
  three square buttons — home / GitHub / crates.io (a Rust-crab glyph) — each on
  the uniform chrome grid, replacing the old text links.

### Changed

- **BYOK is owner/admin-only.** "Use your own key" (the api-key modal, the
  set-model-access switch, and the key-save path) is now hidden and refused for
  public visitors; the verified owner, local dev, and fresh onboarding are
  unaffected.
- The out-of-credits button reads **`redeem`** (was `redeem / open session`).

### Fixed

- **Mobile keyboard no longer covers the chat.** Opening the soft keyboard
  re-anchors the transcript to its latest message (visual-viewport aware, iOS +
  Android) instead of leaving it hidden behind the keyboard.
- **Doubled blank space under tool cards** removed — an empty result-card slot was
  injecting a phantom inter-block gap; the folded tool pill now keeps a single
  rhythm unit beneath it.

## [0.53.0] - 2026-06-21

### Added

- **Autonomous CLI onboarding loop.** A brand-new terminal agent can go zero ->
  funded -> live on mainnet: `onboard --invite <code>` claims its first `$LH` from
  an operator's invite; `onramp --pay <usdce>` mints `$LH` from a USDC.e payment
  via the Tempo MPP (Machine Payments Protocol) on-ramp at web parity
  (1 USDC.e = 100 `$LH`); `link` adopts a funded web wallet's seed via the QR
  seed-adoption flow. See `design/cli-mainnet-onboarding.md`.
- **MPP USDC.e -> `$LH` on-ramp** (`proxy/api/_mpp.ts` + `/mpp/onramp`) — the
  crypto-native sibling of the Stripe fiat on-ramp, reusing the same MintGateFacet
  valve and verifying the on-chain USDC.e settlement itself.
- **Welcome-on-creation.** The sponsor relay greets every newly-registered agent
  with an on-chain MessageFacet note that lands in its bell inbox (push-free,
  durable), from the platform `localharness` agent.
- **Scheduler drift-fix + keeper.** Drift-corrected `recordRun` anchors each fire
  to its slot grid (`firstSlot + k*interval`) so a late tick re-aligns instead of
  compounding; the new ScheduleFacet is cut to mainnet + testnet. `keeper --watch
  [secs]` is a long-lived heartbeat that pokes due jobs within ~secs of their slot.
- **SolidityLite dynamic arrays** — `uint256[]/address[]` storage with `push` /
  `[i]` / `.length` (canonical keccak slot layout).
- New agent tools: `current_time`, `cancel_task` (agents tear down their own
  schedules); a CLI active-chain banner; auto-focus chat input.

### Changed

- **The CLI now defaults to Tempo MAINNET** (chain 4217) — the live platform.
  Testnet (Moderato) is an explicit dev opt-in via `--dev` or `LH_CHAIN=testnet`;
  an unrecognized `LH_CHAIN` is a hard error, never a silent fallback.
- Anthropic: only Claude Opus is user-selectable (Sonnet/Haiku removed from the
  consult allowlist). The agent system prompt forbids emojis.

### Fixed

- **Relay onboarding-gate catch-22.** Funded callers can now `register` (claiming a
  name costs 1 `$LH`, so the caller is necessarily funded), `createInvite`, and
  `submitFeedback` — all gate-exempt, bounded by per-action cost + rate caps. On
  mainnet no agent holds the gas fee token, so the onboarding-only gate had locked
  funded agents out of these core actions.
- The welcome-on-creation hook used fire-and-forget (`void`), which Vercel Edge
  kills on response; it now uses `waitUntil` so the background record completes.
- **Stripped all testnet surface from the prod web bundle** — the live block-
  explorer links sent MAINNET addresses to the testnet explorer; the Moderato
  config, the embedded testnet sponsor key, and the CSP testnet-RPC entry are gone.
- `finish` is the absolute end of a turn (no redundant closing reply after streamed
  text); the notification inbox at-rest format collision (sw.js vs Rust); the
  input -> send gap + hidden scrollbars; the brand glyph + send/stop icon set.

## [0.51.0] - 2026-06-20

### Added

- **Off-chain telemetry, rich feedback & global lessons.** Real turn failures are
  now auto-reported off-chain (redacted ON-DEVICE, filed to a private telemetry
  repo via the proxy); `submit_feedback` keeps the short on-chain note (the public
  SSOT) but ALSO files the full context off-chain, linked by the on-chain record;
  and a global-lessons sweep (`scripts/colony/lesson-digest.mjs` →
  `web/global-lessons.txt`) folds a curated cross-agent lesson set into every
  agent's default prompt. Admin → telemetry toggle (on by default, redacted).
  See `design/telemetry-and-global-lessons.md`.
- **New agent tools** — `publish_public_face(choice)` (publish your own
  directory / app.rl / index.html face on-chain from chat), plus
  `list_notifications` / `clear_notifications` (read + tidy the bell inbox).
- **Header is a brand icon button.** The top-left "lh" is now the real IBM Plex
  Mono SemiBold glyph (the same outline as the favicon/app icons) as a square
  bordered button matching the bell + cog; the header floats transparent over the
  transcript so chat fills to the top; app-icon URLs are content-versioned so an
  installed PWA picks up the new logo.
- **Render-mode settings — mobile-preview + light theme.** Two live toggles in
  the admin panel (and `?preview=mobile` / `?theme=light` URL params, re-applied
  at mount and persisted in `localStorage`). **Mobile preview** frames the app as
  a 390px column and forces the `<=600px` mobile rules in ANY viewport — so a
  desktop browser, or a screenshot tool that can't resize a maximized window,
  renders the true mobile UI. **Light mode** flips the monochrome palette to a
  `html.theme-light` token set (the palette already lived in `style.rs` as CSS
  vars) — inverted, still brutalist, no colored accents. The URL param wins over
  the saved pref so a shared link or a screenshot suite can force a mode.
- **`scripts/shots.mjs` — committed mobile screenshot suite.** Serves `web/` and
  walks every localhost surface (studio + admin panels) in BOTH themes at the
  mobile-preview frame, writing the full set in one command.

### Fixed

- **Relay funded-agent self-pay.** The rate-capped sponsor relay now sponsors the
  x402 `settle` and `$LH` `transfer` selectors for FUNDED agents (on mainnet an
  agent holds only `$LH`, never the AlphaUSD fee token, so it can't self-pay gas)
  — unblocking `send_lh` from a meter-funded / zero-wallet sender. Gas-only
  sponsor exposure, bounded by the unchanged rate caps + float breaker.
- **Model picker** pruned to Gemini Flash + Claude Opus (Sonnet/Haiku dropped from
  the selector); the in-tab prompt pins the live network as authoritative;
  notifications reach the in-app inbox even without Web Push; incoming
  `call_agent` notifies the owner + logs the exchange; `finish` surfaces a final
  message instead of ending abruptly.
- **Balance UI** shows wallet + meter as ONE `$LH` balance; the agent's TBA is
  labelled "agent wallet" to disambiguate the owner's wallet from the agent's.
- **Doc integrity** now owns Cargo dependency pins — a stale `localharness =
  "0.47"` could ship past every gate before — and guards the default system
  prompt against a hardcoded crate version.

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
