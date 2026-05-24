# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.10.3] - 2026-05-24

Phase 1 of the payment-hooks frontier: **visitor-pays-agent
gating on Tempo Moderato testnet**, native-ETH only (no Stripe
yet — that's Phase 2). Owner sets a per-turn price; visitors who
aren't the owner must sign a payment tx to the agent's ERC-6551
TBA before each turn runs. The whole loop is client-side — no
backend, no off-chain ledger — and reuses the existing
master-wallet + iframe-signer plumbing.

### Added (Rust SDK)

- **`registry::next_nonce(address_hex)`** — pending-nonce lookup.
- **`registry::current_gas_price()`** — `eth_gasPrice` wrapper.
- **`registry::submit_and_wait_receipt(raw_hex)`** — send a signed
  raw tx and block until the receipt is mined.
- **`registry::rlp_native_transfer_unsigned(...)`** + **`registry::rlp_native_transfer_signed(...)`**
  — EIP-155 envelope builders for a native-ETH transfer. Lift v
  to `chain_id * 2 + 35 + recovery_id` internally so callers
  don't have to remember the rule.
- **`registry::NATIVE_TRANSFER_GAS_LIMIT`** const (21_000).

All `pub`-level additions; no breaks.

### Added (browser app)

- **Payment-gated turns.** New `src/app/pricing.rs` reads/writes
  `.lh_pricing.json` (per-turn price in wei, stringified to
  survive JSON's 53-bit integer limit). `chat::run_send` calls
  `collect_payment_if_required` before each turn — short-circuits
  on free agents, owner-of-this-agent, or unverified state.
- **Iframe signer extended with `lh-sign-tx`.** New postMessage
  message type sits alongside the existing `lh-sign-challenge`.
  Tx-signing always asks the user's explicit consent via
  `window.confirm()` (challenges still auto-approve — they're
  read-only). Consent dialog spells out the recipient, value in
  test ETH, gas, chain id, nonce, and the human-readable purpose.
- **Pricing card** in the right sidebar on tenant chrome. Owner
  sees a decimal-ETH input + save button; visitors see the
  current per-turn cost as read-only. Save validates input
  (positive decimal, max 18 fractional digits) and re-checks
  `verify_state` before writing — belt-and-suspenders against a
  stale DOM submission from a non-owner.
- **Visitor flow unblocked.** `paint_tenant` no longer forces
  fresh visitors to the "claim this name?" prompt when the name
  has an on-chain owner — they get the chat chrome directly so
  the payment loop is reachable.
- **`VerifyState::Visitor` carries `visitor_address`** (the
  recovered signer) so the payment flow can build a tx from the
  correct `from`. Owner banner / pill markup unchanged.

### Refactored (browser app)

- `verify.rs`: the iframe lifecycle (create hidden iframe,
  attach correlation-id-filtered listener, post payload, race
  vs timeout, tear down) is now in a shared `signer_iframe_request`
  helper. Both `sign_via_iframe` (challenges) and the new
  `sign_tx_via_iframe` use it — no more ~80 lines duplicated.

### Known limitations (Phase 2/3 scope)

- **Test-ETH only.** "TIP-20 we mint and control" was discussed
  for a later phase; test ETH is what the Tempo faucet gives us
  today.
- **No Stripe MPP yet.** Pure on-chain settlement. Stripe Sessions
  + Stripe Connect + Stripe Issuing come in Phase 2/3 — they need
  a thin Vercel serverless function for session creation +
  webhook receipt, which doesn't exist yet.
- **No receipt log.** Each turn pays again; no "I already paid for
  this turn" memory. Reasonable for MVP; a paid-credits balance
  belongs in a follow-up.
- **No price feed.** Owner sets price in wei via a decimal-ETH
  input. No USD pegging yet.

## [0.10.2] - 2026-05-24

### Added (browser app)

- **Admin reset affordance** in the footer of apex and tenant
  chrome. Click the small "admin" link (intentionally muted +
  dashed-border-separated from the main footer) to reveal a panel
  with a `Reset local state` button. Confirm dialog is
  origin-aware:
  - **Apex:** "Reset apex local state? This deletes your master
    wallet…" — back up the seed first or lose the identity.
  - **Tenant subdomain:** "Reset this subdomain's local state?
    This deletes the owner marker, conversation history, API
    key, and every file in this subdomain's OPFS. Your master
    wallet at the apex origin is untouched."
  - **Other** (localhost / Vercel preview): wipes every file in
    this origin's OPFS sandbox.

  The wipe walks `read_dir("")` and deletes every top-level
  entry — including dotfiles like `.lh_wallet` / `.lh_owner` /
  `.lh_chat_history` / `gemini_api_key` — then reloads the page
  so the next paint is the first-visit state. Lets a developer
  see the new-visitor UX without opening an incognito tab.

## [0.10.1] - 2026-05-24

UX polish on the apex onboarding flow plus a repo cleanup pass.
No public SDK API changes.

### Changed (browser app)

- **Apex page is identity-gated.** A first-time visitor to
  `localharness.xyz` no longer has a master wallet silently
  generated in their OPFS just for landing. The apex renders an
  identity sidecar with `[Create identity]` + `[Import existing
  seed]` buttons and a *disabled* claim form; the form unlocks
  only after explicit consent. Returning visitors with an existing
  wallet see the address + agents list above a live claim form.
- **Signer iframe no longer auto-creates.** `?signer=1` loads
  render a "no identity" chrome and reject every postMessage
  challenge when the apex origin has no wallet, instead of
  conjuring one to sign with (which would never match the on-chain
  owner anyway).
- `wallet_store::load_or_create` is split into a pure `load() ->
  Option<MasterWallet>` and an explicit `create_and_persist()`.
  `pub(crate)` API only — no external impact.

### Changed (repo hygiene)

- Dropped three historical docs at the repo root: `DESIGN.md`
  (0.2.x SDK runtime plan, fully shipped), `DESIGN_M5_PLUS.md`
  (M5+ platform plan, shipped through 0.10.0), `UPSTREAM.md`
  (Python upstream tracking, project hasn't been a port since
  0.2.x). Anything you need from them is preserved under git
  tags `v0.1.0`–`v0.10.0`.
- `Cargo.toml` exclude list updated — `contracts/**` is now
  excluded from the published crate (it was leaking into
  `target/package/` previously).
- `RELEASING.md` refreshed: dropped stale Python-upstream-sync
  section + dead `PYTHON_README.md` / `sync-upstream.sh`
  references; added a row noting that
  `src/app/templates.rs` carries a hardcoded `"web demo · X.Y.Z"`
  tag the user has to bump before running the release script.

### Fixed

- The PowerShell 5.1 stderr trap noted in CLAUDE.md is still
  triggered by `build-web.ps1` (cargo's progress lines turn into
  ErrorRecords). The wasm bundle build succeeds anyway because the
  script captures `$LASTEXITCODE` — this is documented as a known
  cosmetic; not a regression.

## [0.10.0] - 2026-05-23

The on-chain story landed in 0.9.0; this release exposes the
registry as a real SDK module so off-bundle consumers (CLI tools,
indexers, native back-ends) can query it without instantiating the
browser app. Also: the registry contract is now a Diamond with an
ERC-721 facet + ERC-6551 token-bound accounts wired up.

### Added (Rust SDK)

- **`pub mod registry`** — JSON-RPC client for the on-chain
  `LocalharnessRegistry` diamond. Hand-rolled (no alloy
  dependency). Gated on `feature = "wallet"`. Constants exposed:
  `RPC_URL`, `REGISTRY_ADDRESS`, `CHAIN_ID`. Public API:
  - `check_name(name) → Status` (Unknown / Available / Taken)
  - `owner_of_name(name) → Option<address-hex>`
  - `tba_of_name(name) → Option<address-hex>` (ERC-6551)
  - `list_owned_tokens(owner_hex) → Vec<OwnedToken>` (iterates
    `1..nextId`; fine for small token counts, swap for log
    indexing or multicall if registry grows past a few hundred)
  - `claim_name(signer, name) → tx hash` (faucet → sign → send
    → poll receipt; requires `feature = "wallet"`)
  - `request_faucet_funds(address_hex)` (Tempo's
    `tempo_fundAddress` JSON-RPC method)
  - `Status`, `OwnedToken` public types
- `sleep_ms` is cfg-gated: `tokio::time::sleep` on native,
  Promise-around-`setTimeout` on wasm. Means the entire registry
  module — including write methods — works equally on a CLI host
  and in the browser bundle.

### On-chain — Tempo Moderato testnet (chain 42431)

The diamond's address (`0xed7a2d170ab2d41721c9bd7368adbff6df0c656d`)
is the only constant the bundle reads. Facets are added/removed via
`diamondCut` without ever changing it.

- **Diamond** at `0xed7a2d…c656d` — EIP-2535 proxy. Storage
  isolated per facet via `keccak256("localharness.<name>.storage.v1")`
  slots.
- **ERC-721 facet** at `0x016882…0e5e` — every registered name is
  now an NFT. `register()` mints `tokenId == agentId` and emits
  Transfer(0, owner, id). Standard surface: balanceOf, ownerOf,
  transferFrom, safeTransferFrom, approve, setApprovalForAll +
  Metadata extension (name="Localharness Names", symbol="LH",
  tokenURI returns `https://<name>.localharness.xyz/`).
- **TBA facet** at `0xe43d11…73a4` — wraps EIP-6551. Public views:
  `tokenBoundAccount(tokenId)`, `tokenBoundAccountByName(name)`,
  `createTokenBoundAccount(tokenId)`. Every name gets a
  deterministic counterfactual wallet at a predictable address.
- **ERC-6551 reference** deployed at:
  - Registry: `0xc7cadc…41d6`
  - Account impl: `0x8ad49e…d7f4` (CALL-only variant — DELEGATECALL
    explicitly disabled to avoid the self-destruct footgun)

### Added (browser app)

- **Cross-origin iframe signer** at `localharness.xyz/?signer=1`
  (M8). Subdomains verify the visitor's address against the
  on-chain owner via postMessage + signature recovery.
- **Visitor read-only mode** — when verification confirms the
  visitor isn't the owner, the input region swaps for a banner.
  Transcript + OPFS panel stay browsable.
- **Apex "your agents" panel** — read the diamond after wallet
  load, list all NFTs owned by the master wallet, link each to
  its subdomain + ERC-6551 wallet on the block explorer.
- **TBA pill in tenant chrome** — header shows the agent's ERC-6551
  wallet address with a link to the explorer.
- **`?prefill=<name>`** apex query param — tenant subdomains' "claim
  on-chain" CTA pre-fills the apex form and kicks off the live
  availability check on arrival.

### Changed

- The registry is now a Diamond at the same address forever;
  future facets (ERC-8004 reputation/validation, MPP/x402
  payments, anything else) cut in without touching the bundle.
- The flat `LocalharnessRegistry.sol` at `0x42c8D4…F9db` is
  abandoned (state not migrated; testnet population was tiny).
- One-name-per-address constraint **dropped** — multi-agent
  ownership is the intended path now that each name is an NFT.
- 67 lib tests pass (up from 62 — registry module brought
  selector + encoding tests with it).

### Notes

- `contracts/` has the full Solidity stack: Diamond core +
  Cut/Loupe/Ownership/Registry/ERC721/TBA facets +
  ERC-6551 reference (registry + account impl) + foundry deploy
  scripts. Architecture write-up in `contracts/README.md`.
- The wasm bundle's behaviour didn't change between 0.9.0 and
  0.10.0 except for the new "your agents" panel and the TBA pill —
  this release is primarily about exposing the registry as a
  reusable SDK module.

## [0.9.0] - 2026-05-23

M8 + M9 — the identity story gets a real auth boundary (cross-origin
signature verification) and the on-chain registry is now an EIP-2535
Diamond, so future facets (ERC-721 / ERC-8004 / ERC-6551 / MPP)
won't churn the bundle's registry address constant ever again.

### Added

- **Cross-origin owner verification** via an apex-hosted iframe
  signer. Subdomains create a hidden iframe to
  `localharness.xyz/?signer=1`, send a domain-separated sign
  challenge (`keccak256("localharness-auth-v0:" || nonce)`), recover
  the address from the returned signature, and compare it to the
  on-chain owner. Status pill in the tenant chrome reflects the
  result: `verifying… → ✓ owner / visitor · owner 0xABC… / not
  on-chain / verify failed`.
- **Visitor read-only mode.** When verification confirms the visitor
  isn't the on-chain owner, the entire input region (key + prompt +
  send button) swaps for a "visitor mode" banner showing who owns
  the name and a link to claim your own. The transcript + OPFS panel
  stay browsable — read access is unaffected.
- **Wildcard subdomain awareness** in the bundle.
  `window.location.hostname` classifies the request into apex /
  tenant / other, and three chrome variants render accordingly. The
  apex marketing page has a single-CTA "claim your subdomain" input
  that does a live on-chain `idOfName(string)` check on every
  keystroke.
- **Master wallet at the apex origin** — auto-generated on first
  visit via `k256 + sha3` directly (avoided alloy due to a
  `serde::__private` compat snag). Persisted to OPFS at `.lh_wallet`
  as a 12-word BIP-39 mnemonic. Show/hide seed phrase + import flow
  for cross-device migration.
- **On-chain registration flow.** Apex form submission: faucet the
  wallet via `tempo_fundAddress`, build + sign + RLP-encode a
  `register(name)` legacy EIP-155 tx, send via
  `eth_sendRawTransaction`, poll for receipt, redirect to the new
  subdomain. Brand-new users go from nothing to "owns
  name.localharness.xyz with a verifiable EVM address" in one click,
  no email, no wallet extension.
- **`feature = "wallet"`** standalone public feature for the keypair
  + signing primitives (also pulled in transitively by
  `browser-app`). New public module `localharness::wallet` with
  `generate`, `generate_with_mnemonic`, `from_private_key_hex`,
  `address`, `sign_hash`, `recover_address`, `verify_hash`, plus
  hand-rolled `rlp_bytes` / `rlp_list` / `rlp_uint` for tx envelope
  encoding (12 unit tests cover the spec's canonical RLP vectors).

### Changed

- **Registry is now an EIP-2535 Diamond** at
  `0xed7a2d170ab2d41721c9bd7368adbff6df0c656d` on Tempo Moderato
  testnet. Replaces the flat contract at `0x42c8D4…F9db`. ABI
  surface is identical (`register / ownerOfName / idOfName /
  setMetadata / transfer / ownerOfId / nextId / metadata`), so the
  wasm bundle code didn't change — only the `REGISTRY_ADDRESS`
  constant. Future ERC-721 / ERC-8004 / ERC-6551 / MPP facets cut in
  without changing the bundle's address.
- The legacy flat `LocalharnessRegistry.sol` stays in-tree as
  historical reference. The deployed-but-unused address is
  documented in the registry module's doc comment.
- `browser-app` feature now transitively pulls in `wallet` (the
  apex chrome needs it).
- Bundle: ~2.2 MB → ~2.2 MB (no measurable delta from the M8 work).

### Notes

- `contracts/src/Diamond.sol` + the 4-facet stack (Cut, Loupe,
  Ownership, LocalharnessRegistryFacet) + `DiamondInit` reference
  nick-mudge's MIT EIP-2535 impl, with the registry's storage
  isolated at `keccak256("localharness.registry.storage.v1")` via
  `LibRegistryStorage`. New facets get their own
  `LibXyzStorage` modules at fresh slots — never touch existing
  ones. Full architecture write-up in `contracts/README.md`.
- The legacy UUID-format `.lh_owner` files on existing tenant
  subdomains keep working as a fallback when verification fails or
  the name has no on-chain entry. No forced migration.

## [0.8.0] - 2026-05-23

M5 + M6 + M7 — the SDK gains a self-sovereign identity story. The
browser bundle now reads its hostname to know which tenant it's
serving, generates an Ethereum-compatible keypair on the apex origin,
hits an on-chain registry on Tempo Moderato testnet to check + claim
names, and persists the master identity via a 12-word BIP-39 seed
phrase. Crate consumers gain `wallet` as a standalone feature for the
keypair / RLP / hashing primitives.

### Added

- **`feature = "wallet"`** (off by default; pulled in by `browser-app`).
  Adds `k256 + sha3 + rand_core + bip39` deps. New public module
  `localharness::wallet` with:
  - `generate()`, `generate_with_mnemonic()`, `signer_from_mnemonic()`,
    `mnemonic_from_phrase()` for keypair management
  - `address(signer)`, `sign_hash`, `recover_address`, `verify_hash`
    for Ethereum-style identity primitives
  - `rlp_bytes`, `rlp_list`, `rlp_uint` for minimal RLP encoding of
    tx envelopes (12 unit tests covering the spec's canonical vectors)
- **Wildcard subdomain awareness** in the browser app. The bundle
  classifies its hostname into `Apex` (`localharness.xyz`) /
  `Tenant(name)` / `Other(raw)` and routes to three chrome variants:
  apex marketing page, per-tenant claim prompt, full app. Per-origin
  OPFS gives per-subdomain data isolation for free.
- **Apex marketing page** with a single-CTA "claim your subdomain"
  input that live-checks availability on every keystroke via an
  on-chain `idOfName(string)` call.
- **Master wallet at the apex origin** — auto-generated on first
  visit, persisted to OPFS at `.lh_wallet` as the 12-word phrase.
  Affordances: collapsible "show seed phrase" with a reveal confirm,
  collapsible "import a seed phrase" to migrate from another device.
- **On-chain registry** — `LocalharnessRegistry.sol` in `contracts/`
  (foundry project). Mirrors ERC-8122's `register / ownerOf /
  setMetadata` surface plus an `idOfName` reverse index for fast
  "is this taken?" checks. Validates names on-chain (a-z 0-9 -,
  3–32 chars, no leading/trailing dash) so the wasm sanitiser
  doesn't have to stay in sync. Deployed on Tempo Moderato testnet
  at `0x42c8D4EaF99bA80F6B6FCA8E163E077D9FC2F9db` (chain id 42431).
- **On-chain claim flow.** Click "claim →" on apex → bundle hits
  the Tempo faucet (`tempo_fundAddress`) to fund the wallet → builds
  + signs + RLP-encodes a `register(name)` legacy tx → submits via
  `eth_sendRawTransaction` → polls `eth_getTransactionReceipt` →
  redirects to the new subdomain with `?claim=1` for the local OPFS
  marker. Brand-new users go from "nothing" to "owns name.localharness.xyz
  with a verifiable on-chain address" in one click, no email, no
  wallet extension.
- **Inline tool-result rendering on subdomains** (carried over from
  0.7.2): tool blocks now flip from `⋯ running` to `✓ done` / `✗ error`
  and the result panel fills with the returned JSON.

### Changed

- `browser-app` feature now transitively pulls in `wallet`. Library
  consumers can still take `wallet` alone for non-browser uses.
- Bundle: ~2.0 MB (0.7.2) → ~2.2 MB (0.8.0). Delta is k256 + sha3 +
  bip39 + the larger app surface.

### Fixed (in addition to 0.7.x rollups)

- The agent loop now emits `StreamChunk::ToolResult` after every tool
  execution (was dead code; never emitted in 0.7.0/0.7.1).
- `ToolResult.error` now reflects tool-encoded `{"error": ...}` JSON
  so UIs can branch cleanly on success vs failure.

### Notes

- `DESIGN_M5_PLUS.md` is the design doc for everything in this
  release plus the M8+ roadmap (iframe-signer for cross-origin auth,
  ERC-6551 per-agent wallets, x402/MPP payments, ERC-8004 reputation).
- Contract source + Foundry deploy script live in `contracts/`. The
  deployed address is baked into `src/app/registry.rs::REGISTRY_ADDRESS`.
- API key persistence in OPFS (`.lh_api_key`) is unchanged from 0.7.2.

## [0.7.2] - 2026-05-23

Two browser-app fixes surfaced by the first real end-to-end smoke of
0.7.1, plus API-key-in-OPFS for ergonomics.

### Fixed

- **Tool result panel never rendered.** The Gemini agent loop emitted
  `StreamChunk::ToolCall` but never `StreamChunk::ToolResult`, so the
  app's result branch was dead code — tool blocks stayed in "running"
  state and the result panel never appeared. Fixed in
  `backends/gemini/loop.rs`: after every tool execution we now emit a
  `ToolResult` chunk in addition to dispatching the post-tool hook.
- **Tool-level errors looked like successes.** When a built-in tool
  returned its error as `{"error": "..."}` JSON (the wire convention),
  `ToolResult.error` was still `None`, so UIs couldn't tell. The loop
  now lifts the JSON `error` field into the typed `ToolResult.error`
  so consumers can branch cleanly.

### Added

- **API key persistence in OPFS** (`src/app/key_store.rs`). The key is
  stored at `.lh_api_key` next to `.lh_history.json` so it survives a
  tab close (sessionStorage doesn't). Same threat model as
  sessionStorage — per-origin sandboxed, XSS-readable. The existing
  "clear" button wipes both OPFS and sessionStorage.

### Notes

- DESIGN_M5_PLUS.md added at repo root — multi-tenant / subdomain /
  wallet plan for what comes after 0.7.x. Nothing in it is shipped.

## [0.7.1] - 2026-05-23

Bugfix for the 0.7.0 browser app — `start_session` failed immediately
with "write tools are enabled but no safety policies are configured"
because the app called `with_capabilities(CapabilitiesConfig::unrestricted())`
without installing a corresponding policy.

### Fixed

- **`src/app/chat.rs::start_session`** now installs
  `policy::allow_all()` alongside the unrestricted capabilities so the
  Agent constructor accepts the configuration. OPFS is sandboxed
  per-origin and the demo is single-tenant, so `allow_all` is the
  right policy here; library consumers in less trusted contexts
  should pick a tighter one.

### Changed

- Web demo footer + version tag now reflect 0.7.0+ behavior:
  conversation history persists across reloads, inline file editing
  is available, fs tools work against OPFS. Previous copy still
  claimed history was tab-only.

## [0.7.0] - 2026-05-23

M4 — the browser-resident IDE moves into the crate as `src/app/`,
gated on `feature = "browser-app"`. The previous `localharness-web`
JS-binding crate and the ~700 lines of inline JS in `web/index.html`
are gone; the UI is now pure Rust + maud HTML templates + HTMX-style
fragment swaps.

### Added

- **`feature = "browser-app"`** (default off). Compiles `src/app/`
  into the crate as a wasm cdylib. Pulls in `maud` for HTML templating
  and `console_error_panic_hook`. Has no effect on a native build.
- **`src/app/`** — the in-tab IDE. Modules: `mod` (mount + state),
  `templates` (maud), `dom` (web-sys helpers), `events` (delegated
  click + keydown), `chat` (turn flow), `opfs` (file browser).
  Architectural rule: no imperative DOM manipulation — all updates are
  `swap_inner` / `swap_outer` / `insert_adjacent_html` targeted at
  fixed element ids.
- **Inline tool-call rendering.** Each `ToolCall` from the
  `StreamChunk` stream renders a collapsible `<details>` block in
  the assistant turn; the matching `ToolResult` swaps the block's
  status pill (`⋯ running` → `✓ done` / `✗ error`) and fills the
  args + result panes.
- **Rust-driven OPFS panel.** The file browser now reads through the
  `Filesystem` trait (was: hand-rolled JS over `navigator.storage`).
  Navigate via `data-action="opfs-nav"` + `data-arg=path`; open files
  via `data-action="opfs-open"`. Refreshes after every chat turn.

### Changed

- **`web/index.html`** shrunk from ~700 lines of JS application code +
  ~250 lines of HTML/CSS to a ~15-line bootstrap (style + `<div id="root">`
  + a one-line `import init` script). All chrome is rendered by Rust
  templates.
- **`scripts/build-web.{sh,ps1}`** now invokes `wasm-pack build .
  --features browser-app --no-default-features` against the root crate
  (was: `wasm-pack build ./localharness-web`). Output bundle name
  changed from `localharness_web*` to `localharness*`.
- **`[lib] crate-type = ["lib", "cdylib"]`** added so native consumers
  still get an rlib and wasm-pack gets a cdylib from the same package.
- `[package.metadata.wasm-pack.profile.release].wasm-opt = false` —
  modern rustc emits post-MVP wasm ops (bulk-memory,
  nontrapping-fptoint) that the wasm-pack-bundled wasm-opt rejects.
  Costs ~10-20% binary size; gains a build that doesn't depend on a
  moving toolchain target.

- **Markdown rendering for assistant text** via `pulldown-cmark`
  (optional dep, pulled in by `browser-app`). Renders at end-of-turn
  per text segment; tool-call blocks remain interleaved between
  rendered segments.
- **`Filesystem::delete(path)`** trait method. Implemented on
  `NativeFilesystem` (recursive `remove_dir_all` / `remove_file`) and
  `OpfsFilesystem` (`removeEntry` with `recursive: true`). Required
  `FileSystemRemoveOptions` web-sys feature. Source-compat break for
  external `Filesystem` impls — they must implement the new method.
- **OPFS wipe button** now actually wipes. Confirms via `window.confirm`,
  walks the OPFS root, deletes every top-level entry, refreshes the panel.
- **Per-turn timing pills** in the status line —
  `done · ttft N ms · total M ms · K turns`.
- **Conversation history persistence.** `GeminiConnection::history_bytes()`
  / `set_history_bytes()` serialize/restore the Gemini wire history as
  opaque bytes. `GeminiAgentConfig::with_history_bytes()` seeds a new
  agent on startup. `Agent::history_bytes()` exposes the typed accessor
  for non-trait Gemini APIs (typed handle stashed during
  `start_gemini` via a new `GeminiConnectionStrategy::with_typed_capture`).
  The browser app writes `.lh_history.json` to OPFS after every turn
  and restores on mount; the "new conversation" button also deletes
  the marker file so a reload starts fresh.
- **Inline OPFS file editing.** The file viewer gains an `edit` button
  that swaps it into an editor (textarea + save/cancel). Save calls
  `Filesystem::write_atomic` and re-renders the viewer with the new
  contents.
- **Public transcript view** for repainting a UI on session resume.
  New types `TranscriptEntry { role, text }` + `TranscriptRole`; new
  methods `GeminiConnection::transcript()` and `Agent::transcript()`;
  new free function `decode_transcript_bytes(&[u8])` for the
  no-instance case (the browser app uses this on mount before any
  agent exists). Tool-call activity is intentionally dropped from the
  projection — this is the human-readable view.

### Removed

- **`localharness-web/`** crate. The published SDK never re-exported it
  (it was `publish = false`), and no external consumer existed. All
  its functionality (`start_session`, `chat`) moved into `src/app/`
  as internal-only code.

## [0.6.0] - 2026-05-22

M3 — fs builtins on a portable `Filesystem` trait with native + OPFS
implementations. The same 6 fs-shaped tools the CLI uses now run in a
browser tab against the Origin Private File System.

### Added

- **`Filesystem` trait** (`src/filesystem/`). Five-method async surface
  (`read`, `write_atomic`, `metadata`, `read_dir`, `walk`) plus
  `DirEntry` / `WalkEntry` / `Metadata` / `EntryKind` value types. The
  `write_atomic` docstring spells out the atomicity contract every impl
  must satisfy.
- **`NativeFilesystem`** (gated on `feature = "native"`). Wraps
  `tokio::fs` + `walkdir` + `tempfile`. Atomicity via tempfile + rename.
- **`OpfsFilesystem`** (wasm32 only). Backs the trait against the
  browser's Origin Private File System via `web-sys`. Atomicity via
  `FileSystemWritableFileStream.close()` swap. Recursive walk + async
  iteration over `FileSystemDirectoryHandle.entries()`.
- **`GeminiBackendConfig::with_filesystem(fs)`** and the delegating
  **`GeminiAgentConfig::with_filesystem(fs)`**. Plug in any
  `Filesystem` impl; `Arc<ConcreteFs>` unsize-coerces to
  `Arc<dyn Filesystem>` automatically.
- **Browser demo gains the 6 fs builtins.** `localharness-web` now
  ships an `OpfsFilesystem` to the agent and enables the full
  capabilities set, so the model in the live demo can `list_directory`,
  `view_file`, `find_file`, `search_directory`, `create_file`, and
  `edit_file` against per-origin OPFS storage.

### Changed

- The 6 fs built-ins (`list_directory`, `view_file`, `find_file`,
  `search_directory`, `create_file`, `edit_file`) no longer call
  `tokio::fs` / `walkdir` / `tempfile` directly — they hold an
  `Arc<dyn Filesystem>` and dispatch through the trait. Their
  constructors changed from unit structs to `Tool::new(fs)`. Source
  compat for downstream code that built tools directly is broken; the
  `register_builtins` path is unchanged.
- The 6 fs built-ins lost their per-file `#[cfg(feature = "native")]`
  gates. They now compile on all targets; registration is gated by
  whether `BuiltinDeps.fs` is `Some(_)`. On native, `connect`
  auto-installs `NativeFilesystem`; on wasm, callers supply an OPFS
  (or other) impl via `with_filesystem`.
- `GeminiConnectionStrategy::connect` honors a caller-supplied
  filesystem before falling back to the platform default.

## [0.5.0] - 2026-05-22

Phase 8 — the SDK now compiles to `wasm32-unknown-unknown`. The same
`Agent` loop the CLI uses runs inside a browser tab; a live demo is
hosted at [antig-compusophys-projects.vercel.app](https://antig-compusophys-projects.vercel.app/).

### Added

- **`wasm32-unknown-unknown` target.** `cargo check
  --no-default-features --target wasm32-unknown-unknown` succeeds.
  The full `Agent → Conversation → Connection → ToolRunner` chain
  is available in the browser; 4 portable built-in tools
  (`ask_question`, `finish`, `generate_image`, `start_subagent`)
  register automatically.
- **`native` cargo feature** (default-on). Gates the parts of the
  SDK that need OS primitives: subprocess spawning, multi-threaded
  tokio, the 6 filesystem builtins (`list_directory`, `view_file`,
  `find_file`, `search_directory`, `create_file`, `edit_file`),
  `run_command`, and the MCP stdio bridge. wasm callers depend with
  `default-features = false`.
- **`src/runtime.rs`** — new module. `runtime::spawn` cfg-gates
  between `tokio::spawn` (native) and
  `wasm_bindgen_futures::spawn_local` (wasm).
  `runtime::MaybeSendSync` is a marker trait that's `Send + Sync` on
  native and empty on wasm — every trait supertraits it instead of
  `Send + Sync` directly.
- **`Connection::subscribe_steps`** now returns a `StepStream` type
  alias that maps to `BoxStream` on native (Send-bound, for
  `tokio::spawn` compatibility) and `LocalBoxStream` on wasm (where
  browser fetch streams aren't `Send`).
- **`localharness-web/` cdylib** (not published). wasm-bindgen
  reference wrapper exposing `start_session(api_key)`,
  `chat(prompt, on_chunk)`, `reset_session()` to JavaScript. Stores
  one `Agent` per tab in a `thread_local<RefCell<Option<Rc<Agent>>>>`.
- **`web/` static site** with `index.html` (streaming chat UI,
  markdown rendering, multi-turn conversation, key cached in
  sessionStorage) and `web/pkg/` (committed wasm-pack output).
- **`vercel.json` + `.vercelignore`** for static-deploy config.
- **`scripts/build-web.{ps1,sh}`** to rebuild the wasm bundle.
- **`scripts/probe-gemini.ps1`** — isolates request-shape vs
  response-parse bugs by hitting the live Gemini API with curl-style
  diagnostics.
- **`CLAUDE.md`** at the repo root — project orientation for future
  Claude Code sessions.
- **`DESIGN.md` Phase 8 addendum** documenting the wasm scope and
  what's deferred.

### Changed

- Every `#[async_trait]` site is now `cfg_attr`'d to use
  `async_trait(?Send)` on wasm so reqwest's browser-fetch futures
  (which aren't `Send`) can satisfy the trait method signatures.
- Trait supertraits — `Tool`, `Connection`, `ConnectionStrategy`,
  the 6 hook traits, `Trigger` — changed from `: Send + Sync` to
  `: MaybeSendSync`.
- `JoinHandle` storage in `Agent` / `Conversation` /
  `TriggerRunner` is cfg-gated; on wasm we fire-and-forget through
  `spawn_local` (no abort handle).
- README adds a "Run in the browser" section and the status line
  now mentions wasm32.

### Fixed

- **`GeminiSseStream::take_frame`** now accepts `\r\n\r\n` frame
  separators in addition to `\n\n`. Browser fetch surfaces Gemini's
  SSE with CRLF — the old parser silently dropped every frame on
  wasm (0 chunks emitted). Regression test covers the CRLF case.

### Compatibility

- 0.x → 0.x: the trait supertrait change (`Send + Sync` →
  `MaybeSendSync`) is source-compatible for downstream impls
  because `MaybeSendSync` is blanket-implemented for any
  `T: Send + Sync` on native. On wasm the bound is relaxed.
- `wasm-bindgen-futures` is a new wasm-target-only dependency.
  Native consumers don't pull it in.

## [0.4.0] - 2026-05-21

GA of Phase 7 — context-window compaction + MCP stdio bridge. The
crate now covers every roadmap item from the original `DESIGN.md`.

### Added

- README expanded with feature-tour entries for MCP-bridged tools
  and automatic compaction.

### Changed

- Built-in tool table marks `start_subagent` as shipping (was
  "not yet implemented" in 0.2.0).

This release contains no code changes vs `0.4.0-alpha.2` other than
the bump and the doc edits. The two alphas covered the implementation.

## [0.4.0-alpha.2] - 2026-05-21

### Added

- **MCP stdio client** under `backends::mcp`. The agent can now expose
  tools served by an external [MCP][mcp] server. Configure via
  `with_mcp_server(McpServerConfig::Stdio { command, args })`; the
  bridge spawns the server, performs the JSON-RPC `initialize`
  handshake, fetches `tools/list`, and registers each remote tool into
  the `ToolRunner` as an `McpTool` adapter. Tool calls are forwarded
  to the server with a 60 s per-call timeout; the response is
  flattened into `{ text, images, is_error }`.

  Scope (alpha.2):
  - Stdio transport only. `Sse` / `Http` variants on
    `McpServerConfig` are accepted at the type level but
    `connect()` returns `Error::Config`. SSE / HTTP land in a later
    alpha.
  - Tools surface only — prompts, resources, sampling, and
    subscriptions are out of scope.
  - Eager registration. Tools are fetched once at connect; server-side
    tool changes are not re-discovered.
  - Custom or built-in tools already registered under the same name
    **win** (MCP doesn't overwrite).

- `AgentConfig::with_mcp_server` and `GeminiAgentConfig::with_mcp_server`
  builder methods.
- Re-exports: `McpBridge`, `McpClient`, `McpToolDecl` from the crate
  root.
- The agent shutdown sequence tears down every MCP subprocess after
  the connection closes.

[mcp]: https://modelcontextprotocol.io

## [0.4.0-alpha.1] - 2026-05-21

### Added

- **Context-window compaction** under
  `backends::gemini::compaction`. When the last turn's
  `prompt_token_count` exceeds
  `CapabilitiesConfig::compaction_threshold`, the loop summarizes the
  oldest history entries via a separate Gemini call and replaces them
  with one synthetic user-role turn tagged `[compacted prior context]`.

  Algorithm:
  - Always preserve the system instruction and the **last 6 user/model
    pairs** verbatim.
  - Honor function-call / function-response pairing — never split a
    `Model { functionCall }` from its `User { functionResponse }`.
  - If summarization fails (network, missing client), fall back to a
    drop-oldest strategy with a tag so the model knows context was
    dropped.
  - A turn never errors out because of a compaction failure; the loop
    logs at WARN and continues.

- 4 new unit tests covering `pick_split` boundary behavior and the
  `should_compact` threshold check. Total: 24 passing.

### Notes

- Threshold is opt-in via `CapabilitiesConfig::compaction_threshold`
  (existing field — previously unused). Set to `None` (default) to
  disable. Typical values: 60-80% of your model's max context window.
- Compaction is intentionally conservative: a small history isn't
  compacted at all (`MIN_HISTORY_TO_COMPACT = 8`).

## [0.3.0] - 2026-05-20

### Removed (BREAKING)

- `Agent::start_local`, `LocalAgentConfig`, `LocalConfig`,
  `connections::local::LocalConnection`,
  `connections::local::LocalConnectionStrategy`, and the entire `proto`
  module are gone. The Go-binary backend they implemented was
  `#[deprecated]` since `0.2.0-alpha.1`; migrate to `start_gemini` /
  `GeminiAgentConfig`.
- Dependencies dropped: `tokio-tungstenite`, `prost`, `prost-types`,
  `path-clean`. The `signal` tokio feature is no longer enabled.
- `Error::ProtoEncode`, `Error::ProtoDecode`, `Error::WebSocket`,
  `Error::BinaryNotFound` removed (no callers). `Error::Http` added in
  case a future backend wants it.

### Added

- **`start_subagent` built-in tool** — completes the 11/11 `BuiltinTool`
  matrix. Spawns a one-shot subagent against the parent's Gemini client:
  takes `{ system_instructions, prompt }`, runs a single text-only turn,
  returns `{ final_response, finish_reason }`. No tool delegation in v1
  (subagent tool dispatch is 0.4.x work).

### Changed

- Crate description updated for the post-Go-binary world.

## [0.2.0] - 2026-05-20

GA of the Rust-native runtime. The crate is now fully self-contained —
no Go binary, no Python install, no localhost daemon.

### Added

- README rewritten for the Gemini backend as the documented default.
  Built-in tool catalog table, structured-output and workspace
  examples, updated architecture diagram showing the inline tool
  dispatch loop.

### Changed

- The `start_gemini` API surface is now considered stable for 0.2.x.
  Breaking changes will require a minor (or major) bump.

### Deprecated

- `Agent::start_local`, `LocalAgentConfig`, `LocalConfig`,
  `LocalConnection`, `LocalConnectionStrategy` remain marked
  `#[deprecated(since = "0.2.0-alpha.1")]`. Removal scheduled for 0.3.0.

## [0.2.0-beta.1] - 2026-05-20

### Added

- **`generate_image` built-in tool** — calls the Gemini image-generation
  model (default `gemini-3.1-flash-image-preview`) via a new
  `GeminiClient::generate` non-streaming method. Returns
  `{ mime_type, data_base64, bytes_len }`; the agent's `image_model`
  config and shared `GeminiClient` are injected at strategy time.
- **`ask_question` built-in tool** (default no-op). Returns
  `{ skipped: true, responses: [] }`. Designed to be overridden — a
  user-registered `ask_question` tool wins (the strategy never
  overwrites). Lets the model attempt interactive flows on hosts that
  don't yet wire interactive UI.
- `BuiltinDeps` struct passed to `register_builtins` so future built-ins
  can pick up additional construction context (image client today).

### Status

All 11 `BuiltinTool` variants except `start_subagent` are now
implemented. Subagents land in 0.3.x.

## [0.2.0-alpha.3] - 2026-05-20

### Added

- **Three write tools** under `backends::gemini::tools`:
  - `create_file(path, content)` — atomic write via `NamedTempFile` +
    rename. Refuses to overwrite. Auto-creates parent directories.
  - `edit_file(path, old_string, new_string, replace_all?)` — exact-once
    substring replacement (or `replace_all: true` to replace every
    occurrence). Atomic write.
  - `run_command(command, working_dir?, timeout_sec?)` — shell exec
    (`cmd /C` on Windows, `sh -c` elsewhere). Per-stream 256 KiB output
    cap, default 30s / max 600s timeout, `kill_on_drop`, surfaces
    `{stdout, stderr, exit_code, timed_out}`.
- All three are auto-registered when `CapabilitiesConfig` enables them
  (the unrestricted default). Workspace-only safety: pair with
  `with_workspace(...)` to gate file writes inside specified directories.

### Changed

- `extract_canonical_path` now resolves the parent directory when the
  target file does not yet exist (necessary for `create_file` to be
  guarded by `workspace_only`).
- 8 new unit tests covering create/edit/run_command happy + error
  paths. Total: 20 tests passing.

### Dependencies

- `tempfile = "3"` (atomic file writes).

## [0.2.0-alpha.2] - 2026-05-20

### Added

- **Tool calling end-to-end** through the Gemini backend. The agent
  loop now drives a model ↔ tool dispatch loop: streams the response,
  collects `functionCall` parts, routes each through hooks → policies →
  `ToolRunner`, appends `functionResponse` parts to history, and
  continues until the model produces no more function calls (or hits
  the 16-round safety cap).
- **Five read-only built-in tools** under `backends::gemini::tools`:
  - `list_directory(path)` — sorted children with name/kind/size.
  - `view_file(path, start_line?, end_line?)` — 1-indexed inclusive
    range, 256 KiB truncation cap, UTF-8 lossy.
  - `find_file(path, pattern, max_depth?)` — glob-matched recursive
    file search, 1000-match cap.
  - `search_directory(path, pattern, file_glob?, case_sensitive?)` —
    regex content search, 500-match / 4 MiB-per-file cap.
  - `finish(output?)` — terminates the turn; captures structured
    output when the agent is configured with a response schema.
- `tools::ToolRunner::iter_tools()` — snapshot every registered tool
  for `FunctionDeclaration` construction.
- `GeminiBackendConfig::with_capabilities` and `GeminiAgentConfig`
  routes built-in selection through `CapabilitiesConfig::effective_tools`.
- Built-in tools are auto-registered into the `ToolRunner` at connect
  time per the capability list. User-registered tools of the same name
  win (no overwrite).
- Unit tests for `list_directory`, `view_file` against the real
  filesystem.

### Changed

- `Agent::start_local` / `start_gemini` now go through
  `start_with_factory<S, F>` so backends can opt into runner injection.
  The Gemini strategy uses this to dispatch function calls inline.
- The agent loop emits `Step { kind: ToolCall, target: Environment }`
  events when dispatching, so `ChatResponse::tool_calls()` lights up.
- `walkdir`, `globset`, `regex` added as deps (built-ins only).

## [0.2.0-alpha.1] - 2026-05-20

### Added

- **`Agent::start_gemini(GeminiAgentConfig)`** — Rust-native Gemini
  backend. Talks to the Gemini REST API directly via `reqwest`; no Go
  binary, no Python install, no external process. This is Phase 1 of
  the 0.2.x runtime per `DESIGN.md`.
- `backends::gemini::{GeminiBackendConfig, GeminiConnectionStrategy,
  GeminiConnection}` — public API for the new backend.
- `backends::gemini::api::GeminiClient` — async client over `reqwest`
  with API-key redaction in `Debug`. Includes a small SSE decoder
  (`GeminiSseStream`) that handles partial chunks and `[DONE]` terminators.
- `backends::gemini::wire::*` — `serde` types matching the Gemini REST
  contract (camelCase verbatim). Round-trip tests cover text, thought,
  and `functionCall` part shapes.
- `backends::gemini::loop::run_turn` — the agent loop. Streams text and
  thought deltas, accumulates the assistant turn into history, emits a
  terminal `Step`. Phase 1 is text-only; tool calls land in Phase 2.
- `examples/text_chat.rs` — end-to-end example against `GEMINI_API_KEY`:
  streams tokens, prints usage summary.

### Changed

- `ChatResponse::text_stream()`, `thoughts()`, `tool_calls()` now return
  `BoxStream<...>` so callers can iterate with `.next().await` without
  needing to `Box::pin` themselves.
- `Agent::start_local`/`start_gemini` share a single
  `start_with_strategy` bootstrap — every future backend gets the same
  hook/tool/policy wiring for free.

### Deprecated

- `Agent::start_local` and the entire 0.1.x `LocalConnection`
  (Go-binary-backed) backend. Will be removed in 0.3.0.

## [0.1.1] - 2026-05-20

### Changed

- Rewrote `README.md` as a full crate landing page: hero example,
  collapsible feature tour (streaming, dual-cursor, custom tools,
  policies, workspace, triggers, multimodal, resume), ASCII
  architecture diagrams, design-notes section, comparison table
  vs the Python SDK, and FAQ.

### Added

- `RELEASING.md`, `CHANGELOG.md`, and `scripts/release.{sh,ps1}`
  define a one-command atomic release process.

## [0.1.0] - 2026-05-20

### Added

- Initial Rust port of the [`google-antigravity`][upstream] Python SDK,
  pinned to upstream commit
  [`d6be9ca`](https://github.com/google-antigravity/antigravity-sdk-python/commit/d6be9ca).
- **`Agent`** (Layer 1) — builder-style config, write-tool safety check,
  background dispatcher routing custom tool calls through
  hooks → policies → `ToolRunner` → `send_tool_results`.
- **`Conversation` + `ChatResponse`** (Layer 2) — stateful session with
  multi-cursor lazy stream (replay-from-zero per cursor). Filtered
  cursors: `text_stream`, `thoughts`, `tool_calls`. Per-turn usage,
  cumulative usage, structured output extraction.
- **`Connection` + `LocalConnection`** (Layer 3) — transport over the
  `localharness` binary. `AtomicBool` for idle, `tokio::sync::broadcast`
  for step fan-out, bounded `mpsc` inbox (cap 16), single
  `tokio::select!` supervisor, separate process supervisor with
  `kill_on_drop`. 10 s handshake timeout.
- **Hook system** — six trait kinds (session start/end, pre/post turn,
  pre/post tool call) with hierarchical `HookContext`.
- **Policy engine** — Python-matching precedence (specific deny ≻
  specific ask ≻ specific allow ≻ wildcard deny ≻ wildcard ask ≻
  wildcard allow), `enforce()` adapter, `workspace_only()` with
  component-wise path containment (defeats `/foo/bar-evil` vs
  `/foo/bar` prefix tricks).
- **`ToolRunner`** — lock-free context swap via `arc_swap`, `ClosureTool`
  builder for ad-hoc tools.
- **`TriggerRunner`** — `every()` helper, abort-on-drop,
  `TriggerDelivery` semantics.
- **Multimodal input** — `Content` / `Part` / `Media` with `from_path()`
  MIME inference; `Bytes`-backed payloads (refcounted, zero-copy clones).
- **Typed errors** — flat `thiserror` enum; `io::Error`,
  `serde_json::Error`, `prost` errors fold via `#[from]`.
- **Smoke example** (`cargo run --example smoke`) — end-to-end against a
  stubbed `Connection`.
- **Upstream sync infrastructure** — `UPSTREAM.md` pins commit;
  `scripts/sync-upstream.{sh,ps1}` diff against pin without modifying
  the working tree.

[upstream]: https://github.com/google-antigravity/antigravity-sdk-python
[Unreleased]: https://github.com/compusophy/localharness/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/compusophy/localharness/releases/tag/v0.1.0
