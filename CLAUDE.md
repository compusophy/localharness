# CLAUDE.md

Project context for Claude Code sessions. Read this first.

> Keep under **40K chars** (harness cap). A *map + gotchas*, not a reference —
> facet semantics in `contracts/README.md`, wire detail in
> `examples/tempo_tx_live.rs`, agent detail in `web/llms.txt`. Adding a fact?
> Cut or compress an older one; don't append.

## What this is

`localharness` is a Rust-native, **model-agnostic** agent SDK **and** a
self-sovereign browser-resident agent platform built on it. ONE crate. `cargo add`
gives an agent loop with streaming text, tool calling, hooks, policies, triggers,
MCP, and context compaction (behind a `Connection`/`ConnectionStrategy` seam —
Gemini/Anthropic/OpenAI/Mock backends ship). Build with `browser-app` on wasm32 and
you also get the live IDE at `<name>.localharness.xyz`.

- [crates.io/crates/localharness](https://crates.io/crates/localharness) (**0.51.x**) · [github.com/compusophy/localharness](https://github.com/compusophy/localharness)
- Native: stable Rust 1.85+, tokio. wasm32: same crate, browser.
- Live: `localharness.xyz` (apex) + wildcard `*.localharness.xyz` (per-user agents).
- On-chain: EIP-2535 Diamond on Tempo Moderato testnet (chain 42431, RPC
  `https://rpc.moderato.tempo.xyz`); Tempo MAINNET live (chain 4217) — flip via
  `mainnet`. See **Canonical addresses** below.

## Canonical addresses (post-reset)

| What | Address |
|------|---------|
| Diamond (`registry::REGISTRY_ADDRESS`) | `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` |
| `$LH` token (`LocalharnessCredits`, TIP-20) | `0x90B84c7234Aae89BadA7f69160B9901B9bc37B17` |
| 6551 Registry | `0x2795810e5dfC8bC92Ef7fc9557F6c0699E11c3B3` |
| 6551 Account impl (`MultiSignerAccount`) | `0x86be7c44d1940F4dE53A738153A12FaAEa68B5a7` |
| Sponsor (fee_payer, in `sponsor.rs`) | `0x0AFf88Ad13eF24caC5BeFD0F9Dc3A05DF79a922C` |
| Diamond owner (cut/admin key, NOT in repo) | `0x313b1659F5037080aA0C113D386C5954F348EF1e` |
| AlphaUSD (sponsor fee_token) | `0x20c0000000000000000000000000000000000001` |
| Credit proxy | `https://proxy-tau-ten-15.vercel.app` |

**Per-facet addresses are NOT pinned** — facets churn via `diamondCut`; query live
via DiamondLoupeFacet (`facets()` / `facetAddress(selector)`). The diamond address
is the only durable handle.

## Repo layout

> Subsystems own NESTED `CLAUDE.md` specs (auto-loaded when an agent works in that
> dir; update the matching one when you change a subsystem): `src/app` (UI / overlay
> modals / no-DOM / one-box-input), `src/registry` (on-chain gas/tx/relay),
> `src/backends` (provider wire quirks), `src/bin/localharness` (CLI), `src/rustlite`
> (cartridge compiler), `src/filesystem` (FS impls + at-rest encryption /
> EXEMPT_FILES), `src/builtins` (tool schema-lint rule), `src/soliditylite`
> (EVM-subset compiler), `src/bashlite` (sandboxed shell — fuel + confirm-gate),
> `src/connections` (L3 transport seam — wasm cfg-gating), `proxy/` (the
> separate-deploy credit proxy / relay / metering), `contracts/` (Diamond
> cut/storage/deploy gotchas; facet semantics in contracts/README.md), `web/`
> (cache-buster + cartridge-worker↔Rust parity), and `scripts/` (release atomicity
> + PS5 trap + feedback tooling). Every `src/` module dir + proxy + contracts + web +
> scripts own a spec; the root stays a whole-repo MAP and detail lives in the specs.

```
src/                  library crate
├── lib.rs            re-exports + module roots
├── agent.rs          Agent facade (L1): start_gemini/start_anthropic/start_mock
├── conversation.rs   Conversation + ChatResponse (L2)
├── connections/      Connection / ConnectionStrategy traits (L3)
├── content.rs        Content, Media, Part (user message types)
├── tools.rs          Tool trait + ToolRunner + ClosureTool
├── hooks.rs          6 hook traits + HookRunner
├── policy.rs         Predicate / Policy / Decision + workspace_only
├── triggers.rs       Trigger trait + TriggerRunner + every()
├── runtime.rs        cfg-gated spawn + sleep_ms + MaybeSendSync marker
├── encoding.rs       THE canonical hex/address/amount codecs (don't re-roll one)
├── turn_flow.rs      pure turn-classification + MAX_AUTO_CONTINUATIONS (hoisted
│                     from app::chat so its loop-guard tests run natively)
├── turn_stage.rs     pure stage state machine for the pending-turn "paying →
│                     thinking → streaming" line (painted by app/chat/stage.rs)
├── builtins/         backend-NEUTRAL builtin tools (8 fs, ask_question, finish,
│                     start_subagent, generate_image, call_agent, ...) + the
│                     schema-lint guards (see src/builtins/CLAUDE.md)
├── filesystem/       Filesystem trait + Native/OPFS impls + Encrypted (at-rest) +
│                     Rooted (confine to a sub-tree — the bashlite CLI sandbox)
├── types.rs          wire-adjacent enums + Step constructors (no hand literals)
├── error.rs          Error + Result · error_codes.rs stable LHxxxx registry
├── wallet.rs         secp256k1 + BIP-39 + RLP (feature "wallet"; all targets)
├── registry/         Diamond JSON-RPC + Tempo tx (feature "wallet"): one module
│                     per facet (names tba credits x402 schedule invite bounty
│                     party reputation validation guild voting feedback
│                     signaling) + multichain (READ-ONLY EVM:
│                     per-chain eth_call/getBalance, ENS namehash+resolve, curated
│                     CORS RPC table — the evm_* tools) + sponsor_relay(mainnet
│                     fee_payer relay client; submit chokepoints route here when
│                     is_mainnet()) + abi/rpc/tx
│                     plumbing (read_view, sponsored_diamond_call skeletons);
│                     mod.rs re-exports keep the flat registry:: surface
├── x402_hook.rs      app-injected x402 signer + proxy-route hooks for
│                     call_agent (feature "wallet")
├── tempo_tx.rs       Tempo Transaction (tx 0x76) encoder; see Tempo section
├── raster.rs compose.rs sharedfs_reconcile.rs signaling_seal.rs kv_reduce.rs
│                     kv_room.rs lessons.rs confirm.rs cut_guard.rs(static
│                     facet-cut safety lint, reserved selectors) keeper.rs(pure
│                     decentralized-scheduler keeper decision core) qr.rs(inline
│                     SVG QR, browser-app) skills.rs(SKILLS LOOP blob core)
│                     native-testable cores (framebuffer/compose/reconcile/SDP
│                     seal/#22 KV CRDT + AES op-seal/lessons/confirm/…)
├── rustlite/         Rust-subset → wasm compiler: lexer / parser / ast /
│                     typecheck / codegen(wasm emitter) / loader(wasm32 cartridge)
├── soliditylite/     Solidity/EVM-subset → EVM-bytecode compiler (the EVM analog
│                     of rustlite, ~5KLOC): lexer / ast / parser / codegen / asm
│                     (bytecode assembler) / mod(compile pipeline). PURE, no deps,
│                     native+wasm. E2E proofs in `examples/soliditylite_*`.
├── bashlite/        tiny sandboxed shell (lexer/parser/eval over a BashHost):
│                     fs builtins + `run`/`source` script COMPOSITION (fractal,
│                     fuel-bounded) + `&&`/`||` + for-`$( )` field-split + lh-*
│                     platform reads/writes (platform.rs, feature wallet) behind
│                     the dry-run-manifest confirm gate. CLI `sh`, browser
│                     `execute_script`. design/bashlite.md
├── app/              browser-resident IDE (browser-app + wasm32) — see below
└── backends/
    ├── (shared)      sse.rs(frame decoder, CRLF-safe) dispatch.rs(hook-gated
    │                 tool pipeline) runners.rs compaction.rs(ONE generic fold
    │                 engine; per-backend compaction.rs are thin adapters)
    │                 stream_timeout.rs — fix backend plumbing HERE, not per-backend
    ├── gemini/       api.rs(client) wire.rs loop.rs compaction.rs mod.rs
    ├── anthropic/    Claude Messages API backend (feature "anthropic")
    ├── openai/       OpenAI Chat Completions backend (feature)
    ├── mock/         deterministic offline backend (Agent::start_mock; wasm-clean)
    ├── mcp/          stdio MCP client (native-only)
    └── local/        in-browser Gemma 3 270M via Burn/wgpu (feature "local")

src/app/ (browser IDE):
  mod.rs(mount routing) templates.rs(all maud HTML) dom.rs(web-sys swaps)
  events/(Action enum + parse + the ONE delegated click/keydown/submit/input
    listener set + dispatch in mod.rs; handler bodies per domain: claim admin
    credits schedule devices subdomains key_sync public_face layout
    (bounty/guild/governance/tba panels removed 0.47.0 — chat tools now)) chat/(
    turn loop in mod.rs; session.rs prompt.rs
    access.rs tools/{platform,bounty,guild,governance,misc})
  history.rs(OPFS conversation + tool-call replay) opfs.rs(file browser/editor
    MODAL off the ADMIN panel — #71 killed the header [files] button)
  display.rs(framebuffer: runs wasm cartridges off-main-thread in a Web Worker +
    rasterizes HTML; main-thread WATCHDOG kills hung workers — the brick fix;
    surface = a fullscreen dismissable OVERLAY, not a tab/panel; host_agent
    bridge: notify + feed + broadcast_compose + viewer_is_owner/has_identity)
  gas.rs(set_metadata_gas — THE sponsored-setMetadata formula, one home)
  notifications.rs(notify tool: local + `to:` cross-agent; sub under
    keccak256("localharness.push_sub"); bell inbox persists to OPFS via sw.js
    relay/stash→push_arrived)
  signer_protocol.rs(lh-* postMessage consts + challenge preimage, used by BOTH
    signer.rs and verify.rs — never re-fork it)
  key_store.rs owner.rs(.lh_owner on-chain-derived hint) tenant.rs(host
    classifier + require_tenant/current_tenant_owner)
  wallet_store.rs signer.rs(apex/?signer=1 postMessage service)
  seed_pull.rs(local-seed-per-origin — mobile fix) agent_rpc.rs(?rpc=1)
  encryption.rs(AES-256-GCM + ECIES) shared_fs.rs/webrtc.rs/sharedfs_sync.rs/
    teams_sync.rs(P2P teams layer, SignalingFacet) system_prompt.rs self_docs.rs
  tool_allowlist.rs sponsor.rs(testnet fee_payer key; mainnet→relay, no embed) verify.rs(owner
    verify + iframe signer client, all LOCAL-FIRST off APP.wallet)

src/bin/localharness/  — agent-onboarding CLI (feature wallet+native): main.rs
  dispatcher + one module per command family + util.rs shared helpers. ~40
  commands; harness-agnostic, server-free; what skill.md tells external agents to
  run. Conventions + mainnet-default + `call`(headless)/`--pay`/keyless-relay/key
  gotchas → `src/bin/localharness/CLAUDE.md` (auto-loaded in-dir). Smoke:
  scripts/smoke-cli.sh.

contracts/   Foundry project (EIP-2535 diamond)
├── src/      Diamond.sol + interfaces/ + libraries/(LibDiamond + one
│             LibXyzStorage per facet) + facets/(see On-chain stack) + erc6551/
├── script/   DeployDiamond.s.sol + one Add<Facet>.s.sol per facet
└── README.md architecture write-up (facet detail lives HERE)

web/          Vercel static site: index.html + boot.js + cartridge-worker.js
              (off-main-thread cartridge runtime, the brick fix) + pkg/(wasm-pack
              output, gitignored) + llms.txt(full agent spec) + skill.md(onboarding)
proxy/        $LH credit proxy — SEPARATE Vercel project. The ONE off-chain
              component. api/gemini.ts(multi-LLM: Gemini/Claude/GPT) +
              api/mcp.ts(x402-gated MCP-over-HTTP) + api/scheduler.ts(Vercel-Cron
              no-tab job worker) + api/notify.ts(web-push, self or cross-agent
              `to`, sender-stamped; CLI `notify --to`)
scripts/      release.{ps1,sh} build-web.{ps1,sh} harvest-feedback.{sh,ps1}
              clear-feedback.sh issue-to-pr.sh test-fleet/(12 QA personas)
examples/tempo_tx_live.rs  — live harness vs Moderato; source of truth for tempo_tx
design/       README.md(index) + active docs + shipped/ (e.g.
              shipped/agent-coordination.md — the economy-ladder design)
```

## Build / test / run

```sh
cargo build        # native
cargo test         # full suite
cargo check --no-default-features --target wasm32-unknown-unknown  # wasm guard
./scripts/build-web.sh      # rebuild wasm bundle
vercel deploy --prod --yes  # deploy web/
```

wasm app build: `wasm-pack build . --target web --out-dir web/pkg --release
--no-default-features --features browser-app`. wasm-opt is disabled (bundled
wasm-opt rejects post-MVP features modern rustc emits).

## Cargo features

- **`native`** (default): tokio + walkdir + tempfile. Required for `run_command`,
  MCP stdio bridge, default `NativeFilesystem` (8 fs builtins: list_directory,
  view_file, find_file, search_directory, create_file, edit_file, delete_file,
  rename_file).
- **`wallet`** (off): `pub mod wallet` + `pub mod registry`. Pulls
  k256+sha3+rand_core+bip39. All targets.
- **`browser-app`** (off): `src/app/` as wasm cdylib. Pulls maud, pulldown-cmark,
  +wallet, +anthropic, +openai transitively. No native effect.
- **`anthropic`** / **`openai`** (off): Claude Messages / OpenAI Chat Completions
  backends. ADDITIVE — no new deps. BYOK or platform `$LH` via the proxy. OpenAI
  gotcha: streamed `tool_calls` are index-keyed fragments to concat (`openai/loop.rs`).
- **`local`** (off): in-browser Gemma 3 270M via Burn wgpu/WebGPU (no proxy/key).
  HEAVY (~570MB); off the DEFAULT browser bundle. The full in-tab path
  (model selector entry, OPFS download button, `start_local` session wiring) is
  ALREADY in `browser-app`, feature-gated on `local`; the **`browser-app-local`**
  composite (`= ["browser-app","local"]`) turns it on — build the local bundle
  with `--no-default-features --features browser-app-local`. `build-web.sh` ships
  the lean `browser-app,mainnet` bundle (no `local`). Gotchas: getrandom-0.4 needs
  `.cargo/config.toml getrandom_backend="wasm_js"` + renamed `getrandom_v04`;
  burn-store DIRECT (memmap2 wasm-broken); GPU read-back MUST
  `into_data_async().await`.
- wasm targets auto-drop walkdir/tempfile, add wasm-bindgen-futures, uuid/js,
  getrandom/js via target-cfg.

SDK-only wasm: `default-features = false`, skip `browser-app`. Registry-only
consumers: `default-features = false, features = ["wallet"]`.

## The wasm story

The crate compiles to `wasm32-unknown-unknown`:

- `runtime.rs::spawn` cfg-gates `tokio::spawn` (native) vs `spawn_local` (wasm).
- `runtime.rs::MaybeSendSync` = `Send + Sync` (native) / empty (wasm). Traits that
  needed `: Send + Sync` now require `: MaybeSendSync`.
- Every `#[async_trait]` is `cfg_attr`'d to `?Send` on wasm.
- `Connection::subscribe_steps` → `StepStream` = BoxStream (native) / LocalBoxStream
  (wasm). `JoinHandle` storage/abort cfg-gated; wasm fire-and-forgets.
- Only `run_command` + MCP stdio bridge are `feature="native"`-gated. The 8 fs
  builtins register whenever a `Filesystem` is supplied (`BuiltinDeps.fs`), so they
  run on wasm over OPFS too. Guard: `fs_builtins_gate_on_filesystem_not_native`.
  Client-free tools (`ask_question`, `finish`, `start_subagent`, `generate_image`)
  work on both, no filesystem.

Adding traits or `tokio::spawn`? Mirror these or wasm breaks SILENTLY (gated
modules don't trip a default `cargo check`).

## Common gotchas

> Module-local gotchas moved to NESTED specs (auto-loaded when you work in that
> dir): on-chain gas/selectors + Tempo-tx wire + the two-$LH-pots bridge →
> `src/registry/CLAUDE.md`; Gemini wire quirks (model-IDs flip, union-schema-400,
> 3.x thought-parts/thoughtSignature echo, SSE CRLF) → `src/backends/CLAUDE.md`;
> the no-DOM / one-box-input / centered-modal-overlay rules → `src/app/CLAUDE.md`;
> release atomicity + the PS5 stderr trap → `scripts/CLAUDE.md`; the /pkg
> cache-buster + cartridge-worker parity → `web/CLAUDE.md`. The cross-cutting ones
> remain below.

- **Signer iframe is DEAD on mobile (cross-origin storage partitioning).** Mobile
  partitions cross-origin iframe storage → embedded `apex/?signer=1` sees an EMPTY
  OPFS → seed-derived ops fail. Fix: `seed_pull.rs` copies the seed into the
  subdomain's own OPFS via a top-level apex round-trip; `verify.rs` runs ops
  LOCAL-FIRST off `APP.wallet`. Don't reintroduce an iframe-only seed path.
- **`?rpc=1` iframes are CALLER-machine-local.** `call_agent`'s hidden iframe
  loads the target ORIGIN's OPFS on the CALLER's device — a foreign agent has
  no key/persona/price there, so the local path only serves YOUR OWN agents.
  On `NO_SESSION_ERR` the tool falls back to the proxy's x402 `ask_agent`
  (`app/remote_call.rs`, caller's $LH → target's TBA). Don't try to make the
  iframe path work cross-machine — there is no target browser involved.

## Release process

```sh
# 1. Land feature work as normal commits.
# 2. Edit CHANGELOG.md — add `## [X.Y.Z]` heading (no date; script adds).
# 3. Run the atomic release script:
./scripts/release.sh X.Y.Z                  # bash / git-bash
pwsh scripts/release.ps1 -Version X.Y.Z     # PowerShell on Windows
```

Pre-flight → version bump → cargo verify → commit → tag → push → cargo publish →
GH release in one shot. On mid-way failure consult `RELEASING.md`; don't hand-fix.

## The browser app (`src/app/`, `feature=browser-app` + wasm32)

**Design rule: no imperative DOM.** All HTML from `maud` templates; only DOM ops
are `set_inner_html`/`set_outer_html`/`insert_adjacent_html` at fixed element ids
(HTMX-style fragment swaps). ONE delegated `click`/`keydown`/`submit`/`input`
listener at document level dispatches via `data-action`/`data-arg`. Zero
`Closure::wrap` outside those four listeners.

**UNIFIED STREAM (issue #28): chat IS the app.** One chronological transcript
fills the content area on every viewport (no mobile FILES/CHAT/DISPLAY tab bar,
no side panels). Tool outputs surface inline (`inline_result_card`); FILES is a
modal off the ADMIN panel (`opfs::toggle_files_modal`, editor in `#fs-viewer`;
header button removed, #71), DISPLAY a fullscreen overlay (ToggleDisplay /
`display::mount_canvas`; × stops the cartridge). `#ctx-bar` sits at the TOP of
the chat column (feedback #62).

**host::compose (cartridge-in-cartridge, NO iframes — RECURSIVE).** A parent
`compose::spawn_module(name,x,y,w,h)`s another subdomain's `app.wasm` as a CHILD
in a sub-rect. Pixel math = `src/compose.rs` (`blit_child`, `map_pointer_into_child`,
`ComposeBudget::v1` 8/node · 16K · 256K · depth 5 · 24 nodes · FB-area
1M/child·8M, #78). Worker
(`cartridge-worker.js`) is a TREE: every node owns a `children`/`focus` table via
`makeComposeApi(node)`, so a child spawns grandchildren — `compositeChildren`
recurses. Node AT depth cap → `INERT_COMPOSE` (spawn -1). Handles
per-node; `compose_spawn`/`compose_bytes` key on a GLOBAL `uid`. JS
`blitChild`/`mapPointerIntoChild` HAND PORT the Rust impls — parity-tested
(`test-compose-wiring.mjs`, verify.sh stage 10).
`composeReset` MUTATES `rootNode` (never reassign — `host_compose` closes over
it). `examples/cartridges/fractal.rl` = the Droste demo.

**Mount-time routing (`mod.rs::mount`):**
1. `?signer=1` → minimal signer chrome + postMessage listener, return. No apex
   wallet → `signer_no_identity`, challenges error; NEVER silently generate a wallet.
2. Else classify via `tenant::current()`:
   - **`Host::Apex`** → identity-gated. `paint_apex` calls `wallet_store::load()`
     (never creates) — fresh visitors see `identity_sidecar` with [Create
     identity]+[Import existing seed], claim form disabled. Wallet creation only
     via `Action::CreateIdentity` / `Action::ImportSeed`.
   - **`Host::Tenant(name)`** → check `.lh_owner`: missing+`?claim=1` → auto-claim;
     missing+no hint → "claim this name"; present → full chat app. Then
     `kick_verification` (background) queries on-chain owner, runs
     `verify::verify_owner`, updates `#verify-pill`, swaps `#input-region` to a
     read-only banner for visitors. Fetches `tba_of_name` for 💰.
   - **`Host::Other`** (Vercel preview, localhost) → full chat app, no verify.

**Two surfaces per subdomain (public face vs studio)**, keyed on `owner.is_some()`:
- **Owner** → lands in the **studio**, never auto-hijacked to fullscreen. Previews
  via `?view=public` (header link → fullscreen face with a `[studio]` escape →
  `?edit=1`).
- **Visitor** → only ever the **public face**. No studio, no edit door.

`resolve_public_face(name)` reads the on-chain choice under
`keccak256("localharness.public_face")` (`registry::public_face_of`) —
`directory`/`app`/`html` — preferring local working copy (owner previews
unpublished edits) else published. `PublicFace`: **Cartridge** (`app.rl` /
`app_wasm_of` → `display::run_in_root_canvas`), **Html** (`index.html` /
`public_html_of` → `render_html_in_root_canvas`), **Directory**
(`paint_public_landing`: profile + sibling agents via `list_owned_tokens`, personas
batch-fetched via `personas_of`). UNSET infers "cartridge if one exists, else
directory". `Host::Other` uses `try_paint_app` (local `app.rl` only).

**Picker (admin → "public face").** `[directory] [publish app] [publish html]` →
`Action::SetPublicFace`. `directory` sets only the choice; `app`/`html`
compile/read local `app.rl`/`index.html` and publish it **plus** set the choice in
ONE sponsored Tempo tx (two `setMetadata` calls).

**Second-device owner upgrade.** A seed-bearing owner without `.lh_owner` paints
as visitor; background `redirect_to_studio_if_owner` navigates to `?edit=1` once
`verify_owner` proves control.

**Cross-visitor publishing (on-chain).** Local `app.rl`/`index.html` are
owner-device working copies; *visitors* see published bytes in the diamond under
`setMetadata(uint256,bytes32,bytes)` (no new facet). Keys:
`keccak256("localharness.{app.wasm, public.html, public_face, persona, x402_price}")`.
`x402_price` = the agent's advertised per-call `$LH` price (decimal-wei UTF-8;
default 0.01 `$LH` unset; `registry::{x402_price_of, x402_ask_price_of,
encode_set_x402_price}`; price-LOCKED (floor + 10% ceiling, #72) by ask_agent).
Generic `registry::{metadata_bytes_of, encode_set_metadata_bytes}` back the
typed accessors.

**Identity-gate invariant.** `wallet_store::load_or_create` is GONE. Two callers:
`load()` (pure read → `Option<MasterWallet>`) and `create_and_persist()` (only from
`Action::CreateIdentity`). Don't reintroduce load-or-create — silent wallet
generation on a marketing-page visit was the bug the gate fixes.

**Device linking is seed-adoption via QR (Option A).** Desktop encrypts its seed
under a one-time code; QR fragment carries the ciphertext to `localharness.xyz/
?adopt=1#s=...`; the other device types the code to import the SAME seed.

## The on-chain stack

Each facet's storage = `keccak256("localharness.<facet>.storage.v1")` in a
`LibXyzStorage` lib; each cut via `script/Add<Facet>.s.sol`. **Full facet
semantics + ABI + gas notes live in `contracts/README.md`** — this is one line
each, gotchas only.

- **DiamondCut / DiamondLoupe / Ownership** — `diamondCut` + introspection +
  EIP-173 `owner()`/`transferOwnership`. RESERVED selectors (`cut_guard.rs`).
- **LocalharnessRegistryFacet** — names + NFT mint + `setMetadata`/`metadata`;
  `register` FREE. `setMetadata` ≈7.6k gas/BYTE — never guess a cap.
- **ERC721Facet** — every name is an NFT; `tokenURI(id)` → `<name>.localharness.xyz`.
- **TbaFacet** — EIP-6551 `tokenBoundAccount(id)`/`…ByName`; deploy idempotent.
- **MainIdentityFacet** — `mainOf`/`mainNameOf`/`isMain`; auto-set on first-claim.
- **FeedbackFacet** — `submitFeedback(string)` (2048-byte cap, 1.3–17M gas; gas =
  spam filter). Owner `clearFeedback()` = TRANSIENT inbox; event log survives.
  **OPT-IN now** (gas-costly): feedback + auto error/cartridge reports default to
  the OFF-CHAIN telemetry repo (`src/app/telemetry.rs` → `proxy/api/telemetry.ts`
  → GitHub Issues = the task list); on-chain is the `lh_feedback_onchain` toggle.
- **CreditsFacet** — `LocalharnessCredits` TIP-20; diamond holds `ISSUER_ROLE`.
  `dailyAllowance` 0 (DISABLED — sybil hole). Funding = redeem + `send_lh`.
- **RedeemFacet** — owner `addRedeemCodes`, holder `redeem(code)` (mint + burn).
- **InviteFacet** — PERMISSIONLESS refundable bearer codes; SUPPLY-NEUTRAL escrow.
- **SessionFacet** — coarse time-boxed sessions; SHELVED (metering is live path).
- **CreditMeterFacet** — per-MESSAGE meter; `meter(addr,amt)` (meter-key-only)
  debits `min(cost,balance)`; `withdrawCredits` pulls unspent back.
- **MintGateFacet** — fiat→`$LH`: issuer-signed `mintFromFiat`→buyer METER (one-shot
  per PI); proxy Stripe-webhook-fired; recovery `MintForReceipt.s.sol`.
- **X402Facet** — x402 EIP-712 "exact" $LH settle (ecrecover + EIP-1271, one-shot
  nonce); `x402DomainSeparator()` read live; price-LOCKED ceiling (#72).
- **DeviceRegistryFacet** — enumerable `linkDevice/devicesOf/isDeviceLinked`
  (replaces log scraping; Tempo RPC caps at 100k blocks).
- **ReleaseFacet** — holder `releaseName` burn (refuses MAIN) + owner
  `adminBurnNames`/`adminResetAll` (testnet); `_burn` clears `register()` writes.
- **ScheduleFacet** — escrowed recurring jobs; `recordRun` SCHEDULER-ROLE-only
  (CAS-guarded); recursion via `scheduleChildJob`; `/goal`→`finish_goal`/`completeJob`.
  Owns `taskOf(uint256)` (BountyFacet must use `bountyTaskOf`).
- **SignalingFacet** — OWNER-SIGNED on-chain WebRTC signaling/presence (topic =
  `keccak256("localharness.devices"‖owner)` + ecrecover; 10-min TTL).
- **BountyFacet** — rung 1: escrowed `postBounty`/`claimBounty`/`acceptResult`→
  worker TBA (x402). Task view `bountyTaskOf`, NOT `taskOf`. Proven E2E.
- **PartyFacet** — rung 2: consent-gated bps-split escrow squads (`*Party*`).
- **GuildFacet** — rung 3: guild = own identity + TBA treasury; roles + nest.
- **VotingFacet** — rung 4: `propose`/`vote`/`execute`, member-count SNAPSHOT.
- **ReputationFacet** — `attest(subject, 1..5, workRef)`, per-work dedup.
- **ValidationFacet** — ERC-8004 stake/challenge/resolve escrow on a workRef.
- **SessionRoomFacet** (#22, cut live) — member-gated append-only OPAQUE KV-op log;
  CRDT+AES off-chain (`kv_reduce`/`kv_room`); createRoom ≈1.3M gas.
- **PairingFacet** — REMOVED (QR seed-adoption superseded it).

**ERC-6551 account** (`MultiSignerAccount`): CALL-only; additional device signers on
top of the NFT holder + EIP-1271 `isValidSignature` (no seed sharing); signers bound
to enroller → an NFT transfer revokes them; rejects high-s. Detail in
`contracts/README.md`.

**Gemini key sync (per-MAIN, on-chain).** The sealed key lives under the owner's
**MAIN tokenId** (`mainOf(owner)`, fallback the name's own id), NOT per-subdomain —
every subdomain shares ONE key. On tenant paint, `try_auto_restore_gemini_key`
fetches + decrypts via the apex iframe BEFORE the api-key modal. Saving best-effort
`auto_sync_gemini_key`s to the MAIN slot.

## Credit proxy + $LH sessions/metering (LIVE)

The proxy (SEPARATE Vercel project "proxy") is the ONE off-chain component —
platform `$LH` is the PRIMARY path, BYOK the fallback. Auth = Ethereum personal-sign
in `x-goog-api-key`; the meter debits `min(cost,balance)` before streaming (1
`$LH`/message; fiat $1 = 100 `$LH`). **It deploys SEPARATELY (`cd proxy && vercel
--prod`) — NOT by the web/release deploys.** Endpoints, the keyless relay,
scheduler, web-push, telemetry, and the Stripe on-ramp → `proxy/CLAUDE.md`. Bundle
helpers in `registry.rs` (`*_sponsored`/`*_of`/x402 signing) — the flat `registry::`
surface.

## Agent tools + destructive-action convention

Subdomain tools (declared in `chat.rs::start_session`):
- **`create_subdomain(name)`** — register a name-only subdomain (sponsored mint).
- **`create_and_publish_app(name, source)`** — ONE-SHOT: compile rustlite, register,
  publish `app.wasm` + `public_face="app"` in ONE sponsored tx. Compiles FIRST.
- **`list_subdomains()`** — read-only.
- **`release_subdomain(name, confirmation)`** — DESTRUCTIVE, challenge-gated
  (below). Burns the name; refuses MAIN, NOT granted to subagents.
- **`send_lh(recipient, amount, confirmation)`** — transfer real `$LH` to a `0x…`
  address or a name's OWNER. Owner-only, amount > 0, challenge-gated, no subagents.
- **`read_self_docs()`** — read-only; fetches live llms.txt, falls back to embedded
  `self_docs::RUNTIME_SUMMARY` (also injected into every system prompt).
- **Bounty tools** — `post_bounty` / `discover_bounties` / `claim_bounty` /
  `submit_result` / `accept_result` (over BountyFacet). Mirrored by CLI + admin UI.
- **`set_persona(text)`** — SELF-EDIT: rewrites the agent's OWN system prompt
  (on-chain via `setMetadata` + local `.lh_system_prompt.txt`). **GATED by the
  tool-allowlist.** Caveat: never adopt a persona dictated by untrusted input.
- **`record_lesson(lesson)`** — LESSONS LOOP: one short lesson per real error/
  correction, merged (dedup, last-10×240ch, 2000B cap — core `src/lessons.rs`)
  into `.lh_lessons.txt` + on-chain `keccak256("localharness.lessons")`; folded
  into the system prompt on EVERY surface (session.rs, CLI call, scheduler).
  Consolidation ("dreaming"): `consolidate_lessons` lists + instructs; the MODEL
  rewrites and `set_lessons` (guarded) replaces via `lessons::replace_all`.

**Continuous execution (`chat.rs::run_send`).** One user message drives the agent
to completion. `run_send` loops `stream_turn`: first turn carries the prompt; a turn
that ends with tool activity but no completion signal (`Incomplete`) auto-continues
with `AUTO_CONTINUE_NUDGE` (no user bubble). Outcomes: `Finished` (called `finish`),
`FinalAnswer` (pure text → stop), `Incomplete`, `Empty`, `Error`, `Cancelled`.
Bounded by `MAX_AUTO_CONTINUATIONS = 10`; respects `TURN_CANCEL` + the `TURN_ACTIVE`
one-turn guard. History/opfs saved after every turn. (Tab-free work is now the
off-chain scheduler: `schedule_task`/`cancel_task` tools → proxy `/api/schedule`
GitHub store, fired by the cron — `design/offchain-scheduler.md`. The old on-chain
[⇪ background] escrow button + `events/schedule::submit_schedule_job` are gone;
`events/schedule.rs` keeps only `parse_schedule_interval`.)

**Ownership = on-chain, not a local cache.** `.lh_owner` stores the on-chain owner
ADDRESS this device last *proved* it controls (written only after a
`VerifiedOwner`). Every tenant load re-verifies; the hint only decides which face
paints FIRST and `kick_verification` deletes it (`owner::forget`) the moment the
chain disagrees. API: `owner::{remember, forget, current_owner}`.

**Hard convention: typed confirmation for destructive / value-moving tools,
enforced at the DISPATCH layer** (prompt-only "never auto-fill" failed).
`chat::confirm_guard` (PreToolCall hook; pure core `src/confirm.rs`) denies the
first call, issues a random single-use code (status line) bound to those exact
args; the retry runs only if the code appears in the LATEST USER message (model
echo rejected). New destructive tools → `confirm_guard::CONFIRM_GATED`.

## Tempo Transactions + sponsorship

User-facing writes use Tempo's **native** AA tx type (`0x76`) so users hold ZERO of
anything. `src/app/sponsor.rs` signs as `fee_payer` and pays fees in AlphaUSD.
Every user-facing write goes through `events::run_sponsored_tempo_call`: tenant
computes sender_hash, apex wallet signs it via the iframe's `lh-sign-digest`
message, the embedded sponsor signs `fee_payer`. **On MAINNET no build embeds a
fee_payer key** — the `fee_payer` half is signed SERVER-SIDE by the rate-capped
relay (`registry::sponsor_relay` → `proxy/api/sponsor.ts`: selector allowlist +
onboarding-only gate + rate window + float breaker), authed by the caller's
personal-sign token. `registry::is_mainnet()` routes the submit chokepoints + the
browser's `run_sponsored_tempo_call` to it. Mainnet sponsor `0x066E748367df…0168f`
(rotated from bundle-exposed `0xE70f4B…`), proxy-env only. `design/cli-mainnet-relay.md`.

### Wire format (live-verified — `examples/tempo_tx_live.rs`)

```text
0x76 || rlp([
    chain_id, mpfpg, mfpg, gas_limit,
    calls,                // [[to, value, input], ...]
    access_list,          // EIP-2930
    nonce_key, nonce,     // Tempo's 2D nonce
    valid_before, valid_after,
    fee_token,            // 0x80 (empty) in sender hash if sponsored
    fee_payer_signature,  // 0x00 placeholder in sender hash; 0x80 or rlp([v,r,s])
    aa_authorization_list,
    key_authorization?,   // truly optional; omit when None
    sender_signature      // flat 65 bytes (r||s||v, v=0/1)
])
```

- Sender hash: `keccak256(0x76 || rlp([1..14_without_sender_sig]))`.
- Fee-payer hash: `keccak256(0x78 || rlp([1..10, fee_token, sender_address,
  aa_authorization_list, key_authorization?]))`. The spec page OMITS
  `aa_authorization_list` at position 13 — found by diffing `wevm/ox`'s
  `TxEnvelopeTempo`. **sender_sig is flat bytes; fee_payer_sig is `rlp([v,r,s])`.**
- Sponsorship overhead ~275k gas on top of the inner call.

### $LH is TIP-20-shaped credit, NOT fee-token-eligible

Tempo `fee_token` validation requires TIP-20 + `currency()=="USD"`.
`LocalharnessCredits` implements the TIP-20 surface but returns
`currency()=="credits"`, so the chain rejects it as a fee_token (intentional — $LH =
in-system credits, not gas). **AlphaUSD** remains the sponsor's fee_token. Mint paths:
`CreditsFacet.claimDaily()` (disabled) + `RedeemFacet.redeem(code)`.

### Sponsor key

`sponsor.rs` const = the dedicated low-budget sponsor (rotated 2026-05-25). It is
NOT the deployer/owner. The embedded sponsor only pays user fees in AlphaUSD; if the
bundle is extracted, loss is capped at its balance. **Rotate again before mainnet.**
Tempo access keys CANNOT sign as `fee_payer` (confirmed from their SDK) — fee_payer
must come from the root key, which is why a sponsor key must be embedded in wasm.

## What's pending

Shipped: SDK runtime, browser IDE, platform layer, Tempo native AA, Anthropic +
OpenAI backends, scheduling + recursion, Mock backend, economy rungs 1–4 +
Reputation + colony, x402, host::compose, SessionRoom KV (#22), at-rest OPFS enc,
Stripe fiat on-ramp (MintGateFacet), in-browser Gemma (`browser-app-local`), CLI
runtime chain selection (`LH_CHAIN`), the **rate-capped sponsor RELAY** (mainnet
keyless fee_payer signed server-side — NO build embeds a mainnet money key;
`registry::sponsor_relay` + `proxy/api/sponsor.ts`, LIVE), and **bashlite /
localharnesslite** (CLI `sh` + browser `execute_script`; design/bashlite.md). Open:

- **Browser relay E2E + web redeploy** — the keyless bundle routes onboarding
  through the relay (committed); needs an in-browser onboarding test, then a
  deliberate `build-web.sh` + deploy.
- **Relay funded-agent writes (PARTIAL)** — a funded agent CAN now spend its own $LH
  on mainnet via the relay (`SELF_PAY_SELECTORS`: transfer/settle/approve/createInvite
  are gate-exempt). OPEN: its other writes (publish/schedule `setMetadata`) still gate
  to `LH_RELAY_FUNDED`, and it can't self-pay gas (holds $LH, never the AlphaUSD fee token).
- **SessionRoom phase 2** — multi-identity rooms: ECIES-grant `K_room` (v1 live).
- **P2P teams** — 2-device E2E, mutable shared-FS, team UI.
- **Local Gemma** — shipped behind `browser-app-local`; a live WebGPU run pending.

## Filesystem trait

The 8 fs builtins call `crate::filesystem::Filesystem` (not `tokio::fs`), so they
run on wasm/OPFS too. Impls: Native / OPFS / Encrypted (AES-256-GCM at rest) /
Rooted (bashlite sandbox). **⛔ EncryptedFilesystem must NEVER seal the
`EXEMPT_FILES`: `.lh_wallet` (the seed IS the key root → sealing bricks identity),
the pre-wallet boot files (`.lh_owner`/`.lh_linked_owner`/`.lh_device_key`), and the
model artifacts.** Trait surface, the `LHE1‖nonce‖ct` at-rest format, and the full
EXEMPT list → `src/filesystem/CLAUDE.md`.

## Documentation SOP

**Drift-prone FACTS are GENERATED, not hand-copied** (`docs/SOP-doc-integrity.md`).
Chain addresses, the crate version, `$LH` pricing, the agent-tool list, and the
CLI list live in ONE place — `src/docs_manifest.rs` (chain facts DERIVED from
`registry::chain::{MAINNET,MODERATO}`, version from `CARGO_PKG_VERSION`). They
fill `<!-- GEN:key -->`…`<!-- /GEN:key -->` blocks in **web/skill.md** +
**web/llms.txt** via `cargo run --bin gen-docs` (`--check` =
drift-only). NEVER hand-edit a GEN block; change the fact in the manifest +
regenerate. Gates enforce it: a `cargo test` drift-test
(`docs_manifest::tests::no_doc_drift`, runs under `--features wallet`),
`build-web.sh` regenerates pre-build, and `release.{sh,ps1}` run
`gen-docs -- --check` in PRE-FLIGHT — **a version bump cannot ship stale docs.**

**README.md = a DERIVED COPY of web/skill.md** (#56: ONE doc; gen-docs writes
filled skill.md → README; edit skill.md only; guard `readme_skill_in_sync`).
Hand-written: **docs.rs** (`///`) · **CLAUDE.md** (under 40K) · **CHANGELOG.md**
· skill.md/llms.txt PROSE (only GEN-block facts generated).

**When to update what:** drift-prone fact (chain/version/pricing/tool/CLI) →
`docs_manifest.rs` + `gen-docs`; new pub API → `///`; new module → CLAUDE.md tree; new agent tool → `AGENT_TOOLS` in the
manifest + `llms.txt` prose + session prompt; new facet → CLAUDE.md on-chain +
`contracts/README.md` + `llms.txt`; release → CHANGELOG.

**Verify before release:** `cargo run --bin gen-docs -- --check` (the release
pre-flight does this) + `cargo doc --no-deps 2>&1 | grep "warning.*missing"`.
