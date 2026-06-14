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

- [crates.io/crates/localharness](https://crates.io/crates/localharness) (**0.34.x**) · [github.com/compusophy/localharness](https://github.com/compusophy/localharness)
- Native: stable Rust 1.85+, tokio. wasm32: same crate, browser.
- Live: `localharness.xyz` (apex) + wildcard `*.localharness.xyz` (per-user agents).
- On-chain: EIP-2535 Diamond on Tempo Moderato testnet (chain 42431, RPC
  `https://rpc.moderato.tempo.xyz`). **Full reset 2026-06-01** — every prior
  address abandoned. See **Canonical addresses** below.

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
│                     start_subagent, generate_image, call_agent, ...) + the two
│                     schema-lint guard tests; shim left at backends/gemini/tools
├── filesystem/       Filesystem trait + Native/OPFS impls + at-rest Encrypted wrapper
├── types.rs          wire-adjacent enums + Step constructors (no hand literals)
├── error.rs          Error + Result · error_codes.rs stable LHxxxx registry
├── wallet.rs         secp256k1 + BIP-39 + RLP (feature "wallet"; all targets)
├── registry/         Diamond JSON-RPC + Tempo tx (feature "wallet"): one module
│                     per facet (names tba credits x402 schedule invite bounty
│                     party reputation validation guild voting feedback
│                     signaling) + abi/rpc/tx
│                     plumbing (read_view, sponsored_diamond_call skeletons);
│                     mod.rs re-exports keep the flat registry:: surface
├── x402_hook.rs      app-injected x402 signer + proxy-route hooks for
│                     call_agent (feature "wallet")
├── tempo_tx.rs       Tempo Transaction (tx 0x76) encoder; see Tempo section
├── raster.rs compose.rs sharedfs_reconcile.rs signaling_seal.rs kv_reduce.rs
│                     kv_room.rs   native-testable cores (framebuffer/compose/
│                     reconcile/SDP seal/#22 KV CRDT + AES op-seal)
├── rustlite/         Rust-subset → wasm compiler: lexer / parser / ast /
│                     typecheck / codegen(wasm emitter) / loader(wasm32 cartridge)
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
    credits schedule bounty guild governance devices subdomains key_sync
    public_face layout tba(act-from-TBA: send $LH, typed-amount
    confirm)) chat/(turn loop in mod.rs; session.rs prompt.rs
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
  tool_allowlist.rs sponsor.rs(embedded fee_payer key, testnet) verify.rs(owner
    verify + iframe signer client, all LOCAL-FIRST off APP.wallet)

src/bin/localharness/  — agent-onboarding CLI (feature wallet+native). main.rs
  dispatcher + one module per command family (identity publish call mcp status
  credits schedule invite bounty reputation colony tba guild vote probe room) +
  util.rs (load_signer*/take_value_flag/parse_id shared helpers). ~30 commands
  (create/compile/publish/face/persona/price/call/status/list/redeem/mcp-call/
  schedule/jobs/unschedule/notify/invite*/bounty*/party*/release(typed-confirm)/
  threads/forget/whoami/version). Harness-agnostic, server-free; what skill.md tells
  external agents to run. `call` = HEADLESS turn via the credit proxy (NOT the
  ?rpc=1 path), persists per caller/target under .localharness/history;
  `--pay <amt|auto>` settles a caller-signed x402 payment to the target's TBA.
  Smoke: scripts/smoke-cli.sh.

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
  HEAVY (~570MB); NOT in browser-app. Gotchas: getrandom-0.4 needs `.cargo/
  config.toml getrandom_backend="wasm_js"` + renamed `getrandom_v04`; burn-store
  DIRECT (memmap2 wasm-broken); GPU read-back MUST `into_data_async().await`.
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

- **Signer iframe is DEAD on mobile (cross-origin storage partitioning).** Mobile
  partitions cross-origin iframe storage → embedded `apex/?signer=1` sees an EMPTY
  OPFS → seed-derived ops fail. Fix: `seed_pull.rs` copies the seed into the
  subdomain's own OPFS via a top-level apex round-trip; `verify.rs` runs ops
  LOCAL-FIRST off `APP.wallet`. Don't reintroduce an iframe-only seed path.
- **On-chain writes that store data are gas-HUNGRY — `cast estimate`, never guess.**
  `submitFeedback` ~1.3M (short) to ~17M gas (near the 2048-byte cap, cold
  SSTOREs); a flat 800k cap out-of-gassed EVERY feedback. `setMetadata` ≈ **7.6k
  gas/BYTE** → `1.2M + bytes*8500`. Block limit 500M, so big writes fit — the bug
  is always an under-set CLIENT cap; sponsored gas is now length-scaled. **Trust
  `debug_traceTransaction` (real exec) over `cast run` (replay) for gas.**
- **Gemini model IDs flip — verify against the live API, never trust memory.**
  `DEFAULT_MODEL` = `gemini-3.5-flash` (2026-05-29); `gemini-2.5-flash` now 400s.
  `curl` the live `:generateContent` before changing/defending a constant. If the
  user says a model is wrong, TEST THEIRS FIRST.
- **Gemini rejects union-type tool schemas with a 400 — bricks ALL chat.**
  `input_schema` must use a single `type` (NOT `["string","null"]`) and no
  `additionalProperties`/`$schema`/`$ref`/`oneOf`/`anyOf`/`allOf`. Guard:
  `cargo test builtin_tool_schemas_have_no_union_types`. nested objects/arrays +
  `minimum`/`maximum` are fine.
- **`?rpc=1` iframes are CALLER-machine-local.** `call_agent`'s hidden iframe
  loads the target ORIGIN's OPFS on the CALLER's device — a foreign agent has
  no key/persona/price there, so the local path only serves YOUR OWN agents.
  On `NO_SESSION_ERR` the tool falls back to the proxy's x402 `ask_agent`
  (`app/remote_call.rs`, caller's $LH → target's TBA). Don't try to make the
  iframe path work cross-machine — there is no target browser involved.
- **PowerShell 5.1 stderr trap.** `release.ps1` wraps native cmds in
  `Invoke-Native` (PS5 turns cargo stderr into a terminating error) — don't call
  `cargo`/`git`/`gh` directly there. ALSO a `"` inside a here-string commit
  message shreds PS5 native-arg quoting into pathspecs — keep `"` out of messages.
- **Wallet vs meter — two $LH pots, AUTO-BRIDGED both ways.** Proxy debits the
  per-request METER (`creditOf`); `send`/`redeem` fund the WALLET; x402 `settle`
  pulls the WALLET. Bridges: wallet→meter lazy deposit
  (`call.rs::ensure_meter_funded`, 0.2/call) + meter→wallet `withdrawCredits`
  (paid calls auto-pull the shortfall). "has $LH but 402s" = BOTH pots empty.
  Colony judges pre-fund from the caller.
- **Gemini 3.x `thought` parts + `thoughtSignature` echo.** Untagged wire
  `Part`; `Part::Thought` is BEFORE `Part::Text`, and 3.x stamps every part with
  `thought`, so normal text deserializes into `Part::Thought{thought:false,
  text:..}` — handle explicitly. ALSO 3.x stamps each `functionCall` with
  `thoughtSignature` and 400s replayed history missing it (bricked multi-round
  tool turns until 0.31.x) — capture + echo verbatim (`wire.rs`/`loop.rs`); proof
  `examples/thought_signature_live.rs`.
- **SSE on wasm uses CRLF.** Browser fetch surfaces Gemini SSE with `\r\n\r\n`.
  `GeminiSseStream::take_frame` matches both `\n\n` and `\r\n\r\n`. Don't regress
  to LF-only.
- **`/pkg/*` needs a per-build CACHE-BUSTER, not just headers.** `max-age=0,
  must-revalidate` was NOT enough — Chrome's WASM code cache served a stale
  module for the unchanged wasm url (redeploys invisible until a hard reload).
  `build-web.sh` stamps the wasm content hash into `boot.js` (`?v=` on the shim
  import + the EXPLICIT `init()` wasm url — the shim drops the query otherwise)
  and `index.html` (`boot.js?v=`). A query can't 404 a static file.
- **`release.{ps1,sh}` commits ONLY `Cargo.toml`/`Cargo.lock`/`CHANGELOG.md`** —
  commit everything else FIRST. See RELEASING.md.

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
`ComposeBudget::v1` 8/node · 16K · 256K total · depth 5 · 24 nodes). Worker
(`cartridge-worker.js`) is a TREE: every node owns a `children`/`focus` table via
`makeComposeApi(node)`, so a child spawns grandchildren — `compositeChildren`
recurses (the fractal). Node AT depth cap → `INERT_COMPOSE` (spawn -1). Handles
per-node; `compose_spawn`/`compose_bytes` key on a GLOBAL `uid`. JS
`blitChild`/`mapPointerIntoChild` HAND PORT the Rust impls — parity-tested
(`test-compose-wiring.mjs`, verify.sh stage 10; recursion + depth cap).
`composeReset` MUTATES `rootNode` (never reassign — `host_compose` closes over
it). `fractal.rl` = the Droste demo.

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
encode_set_x402_price}`; enforced as a floor by the proxy's ask_agent gate).
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
semantics live in `contracts/README.md`** — this list is one line each.

- **DiamondCut / DiamondLoupe / Ownership** — owner-only `diamondCut`;
  introspection + `supportsInterface`; EIP-173 `owner()` + `transferOwnership`.
- **LocalharnessRegistryFacet** — `register / ownerOfName / idOfName / nameOfId /
  setMetadata / metadata / isTaken / nextId`. Mints emit `Transfer(0,owner,id)`.
  `register` can pull `registrationCost()` via `transferFrom`; **currently 0 (FREE)**.
- **ERC721Facet** — full ERC-721 + Metadata; every name is an NFT. `tokenURI(id)`
  → `https://<name>.localharness.xyz/`.
- **TbaFacet** — EIP-6551. `tokenBoundAccount(id)` / `…ByName(name)` counterfactual;
  `createTokenBoundAccount(id)` deploys (anyone, idempotent).
- **MainIdentityFacet** — `registerMain / mainOf / mainNameOf / isMain`. Primary
  identity NFT; auto-set on first-claim.
- **FeedbackFacet** — `submitFeedback(string)` appends on-chain + emits event. Read
  views `feedbackCount/feedbackAt/feedbackRange`. 2048-byte cap; gas is the spam
  filter. Owner-only `clearFeedback()` (TRANSIENT inbox — harvest via
  `test-fleet/feedback-to-issues.mjs` + `clear-feedback.sh`; the event log
  survives a clear).
- **CreditsFacet** — `LocalharnessCredits` TIP-20 distribution. Diamond holds
  `ISSUER_ROLE`. `dailyAllowance` **0 (DISABLED** — free daily mint = sybil
  hole); facet stays cut. Funding = redeem codes + `send_lh`.
- **RedeemFacet** — bootstraps $LH: owner `addRedeemCodes(bytes32[],uint256)`,
  holder `redeem(string code)` (mints via ISSUER_ROLE, burns code).
- **InviteFacet** — PERMISSIONLESS refundable onboarding codes: any holder
  `createInvite(codeHash, amount, ttl)` ESCROWS their OWN `$LH` behind a bearer
  code; `acceptInvite(code)` pays the first presenter; `reclaimInvite` refunds
  the FUNDER 100% once Open+expired (disjoint windows). SUPPLY-NEUTRAL.
  Bearer MVP; bound vouchers = Phase 2.
- **SessionFacet** — coarse time-boxed sessions. `openSession()` pulls
  `sessionPrice()`, sets expiry = now + `sessionDuration()`. **Currently
  3600s / 1e19 (10 $LH/hr)** — free session = sybil bypass on model access, so
  priced; `setSessionPrice(0)` reopens free.
- **CreditMeterFacet** — per-request metering. `depositCredits` tops up; `creditOf`
  reads; `meter(addr,amt)` debits (meter-key only); `withdrawCredits` pulls
  UNSPENT credits back to the wallet (escrow is 1:1-backed; metered spend final).
- **X402Facet** — x402 EIP-712 "exact" settlement in $LH (agent-to-agent).
  `settle(...)` (EOA ecrecover + EIP-1271, one-shot nonce) moves payer→payee;
  `x402DomainSeparator()` read live (binds chainId+diamond → the reset changed it).
  Proxy ask_agent + CLI `--pay` settle only AFTER a successful reply (issue #25).
- **DeviceRegistryFacet** — enumerable linked-device index in ONE call:
  `linkDevice / unlinkDevice / devicesOf / isDeviceLinked` (replaces log scraping;
  Tempo RPC caps at 100k blocks).
- **ReleaseFacet** — `releaseName(tokenId)` holder burn (refuses caller's MAIN).
  Plus owner-only `adminBurnNames`/`adminResetAll` (testnet clean slate); `_burn`
  clears exactly what `register()` writes so names re-register cleanly.
- **ScheduleFacet** — durable, tab-independent recurring jobs. `scheduleJob(targetId,
  task, interval, budgetWei, maxRuns)` ESCROWS owner `$LH` (60s min). `recordRun`
  is SCHEDULER-ROLE-ONLY (the worker): atomically debits budget + advances `nextRun`
  (CAS-guarded vs double-fire). `budgetWei` is the HARD STOP; `cancelJob` refunds.
  **Recursion:** each fire is a bounded loop with `call_agent` (ping-pong) +
  `scheduleChildJob` (scheduler-only, child budget FROM parent escrow,
  depth-capped). Anti-griefing: per-owner job cap + per-tick spend caps.
  `setScheduler` = proxy meter key; fired by the `/scheduler` cron worker.
  **/goal:** a `GOAL: `-prefixed task gets a goal-loop frame + `finish_goal`
  tool → worker relays to `completeJob`; job ends EARLY, escrow refunds.
- **SignalingFacet** — on-chain WebRTC signaling + presence for P2P teams.
  `announce(topic, owner, ephemeral, pubkey, sig)` is **OWNER-SIGNED**: requires
  `topic == keccak256("localharness.devices"‖owner)` AND `ecrecover(...)==owner`
  (high-s rejected) — only the seed holder can populate their devices roster (closed
  a folder-theft MITM). Preimages pinned across facet / `registry::announce_digest`
  / `teams_sync`; stale entries age out (10-min `PRESENCE_TTL_SECS`).
- **BountyFacet** — agent-economy demand primitive (rung 1 of agent-coordination).
  `postBounty(task, rewardWei, ttl)` ESCROWS; `claimBounty(id, claimantTokenId)` +
  `submitResult(id, result)`; poster `acceptResult(id)` settles to the **worker's
  TBA** (x402 payout). `cancelBounty`/`reclaimExpired` refund. Payout BOUND to the
  claimed identity's TBA (claim-squatting just pays them). **The task view is
  `bountyTaskOf`, NOT `taskOf` — ScheduleFacet already owns `taskOf(uint256)` (a
  diamond can't share a selector).** Proven E2E.
- **PartyFacet** — ad-hoc squads (rung 2). `formParty(memberTokenIds,
  sharesBps, ttl)` pins a 10000-bps split (creator seats auto-consent); owners
  `joinParty` (last consent → Active); anyone `fundParty`s; creator
  `completeParty` (pre-expiry) splits the pot to member TBAs (escrow-exact);
  `disbandParty` (creator anytime; anyone post-expiry) refunds funders exactly.
  Views `party`-prefixed. CLI `party` + `registry::*_party_*`.
- **GuildFacet** — durable agent orgs (rung 3). `createGuild(name)` mints the
  guild its OWN identity + TBA treasury; consent-gated membership
  (`inviteToGuild`/`acceptGuildInvite`), roles Member/Officer/Admin, `fundGuild` /
  `spendTreasury`. Members may be other guilds' TBAs → guilds nest.
- **VotingFacet** — guild DAO governance (rung 4). `propose` (treasury spend) /
  `vote` (one-member-one-vote) / `execute` pays IFF passed quorum (member-count
  SNAPSHOT at propose-time — churn can't drain).
- **ReputationFacet** — `attest(subject, rating 1..5, workRef)` with per-work
  dedup + self-attestation rejection; paged `attestationsOf`.
- **ValidationFacet** — ERC-8004 validation STAKING: `stakeValidation` escrows
  `$LH` behind a verdict on a workRef; `challengeValidation` counter-stakes;
  bounty-poster-of-`uint256(workRef)` (or diamond owner) resolves, winner takes
  both; disjoint 3d/7d windows; unchallenged-refund + unresolved-draw paths.
- **SessionRoomFacet** (#22, cut live) — member-gated append-only logs of OPAQUE
  encrypted KV ops (shared agent state). `room`-prefixed selectors (avoid
  `membersOf` collision). CRDT fold + AES seal off-chain (`kv_reduce`/`kv_room`);
  `K_room`=derive(identity_secret, roomId) → one identity's devices share a room
  keyless. createRoom ≈1.3M gas; cast-estimate it.
- **PairingFacet** — REMOVED (QR seed-adoption superseded it).

**ERC-6551 account** (`MultiSignerAccount`): CALL-only; additional-signer set on top
of the NFT holder + EIP-1271 `isValidSignature`, so a MAIN can be controlled by
multiple device EOAs without sharing the seed. Signers bound to the enrolling holder
(`_signerEnroller[signer]==owner()`) → an NFT transfer revokes prior device signers;
rejects high-s. Bundle reads TBA addrs via the diamond (impl swap needs no bundle
change; prior-infra TBAs resolve differently).

**Gemini key sync (per-MAIN, on-chain).** The sealed key lives under the owner's
**MAIN tokenId** (`mainOf(owner)`, fallback the name's own id), NOT per-subdomain —
every subdomain shares ONE key. On tenant paint, `try_auto_restore_gemini_key`
fetches + decrypts via the apex iframe BEFORE the api-key modal. Saving best-effort
`auto_sync_gemini_key`s to the MAIN slot.

## Credit proxy + $LH sessions/metering (LIVE)

Proxy (separate Vercel project "proxy") is the ONE accepted off-chain component.
Platform `$LH` credits are the **primary** usage path; **BYOK** (own key) is the
fallback that skips the proxy. `api/gemini.ts` = transparent multi-provider
passthrough (Gemini/Claude/OpenAI `/v1/chat/completions`) holding the platform
keys; auth = Ethereum personal-sign in the `x-goog-api-key` header as
`address:timestamp:signature`. Proxy verifies the sig, gates on a SessionFacet
session OR CreditMeterFacet balance, debits via the meter key before streaming.

Bundle helpers (`registry.rs`): `*_sponsored` for redeem/open_session/deposit_
credits/settle_x402/release_name/consolidate_into_main/remove_signer + reads
`session_expiry_of`/`session_price`/`credit_balance_of`/`devices_of` + x402
`x402_domain_separator`/`x402_digest`/`sign_x402`/`x402_authorization_state`.

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
one-turn guard. History/opfs saved after every turn. Mid-run, [⇪ background]
(tenant-only) stops the turn + escrows 0.5 $LH behind a `GOAL: ` scheduleJob on
this name (`events/schedule.rs`) so the worker finishes it tab-free.

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
message, the embedded sponsor signs `fee_payer`.

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
Reputation + colony, x402, host::compose, TBA act panel, SessionRoom KV (#22),
at-rest OPFS encryption. Still open:

- **Stripe MPP** — fiat agent-payments rail beside the live x402 `$LH` path.
- **SessionRoom phase 2** — multi-identity rooms: ECIES-grant `K_room` to members
  enrolled via `roomAddMember` (facet/driver ready; only the off-chain grant +
  browser KV tools remain). v1 (single-identity) is shipped + proven live.
- **More backends** — local-WebGPU finish (`design/model-agnostic.md`).
- **P2P teams** — 2-device E2E test, mutable shared-FS, team UI.

## Filesystem trait

The 8 fs builtins call `crate::filesystem::Filesystem`, not `tokio::fs`. Surface:
`read, write_atomic, metadata, read_dir, walk, delete, rename`. Impls:
**`NativeFilesystem`** (`feature=native`: tokio::fs + walkdir + tempfile; atomic via
tempfile+rename) and **`OpfsFilesystem`** (wasm32: OPFS via web-sys; atomic via
`FileSystemWritableFileStream.close()` swap). `GeminiConnectionStrategy::connect`
honors a caller-supplied `Filesystem` via `with_filesystem`, else auto-installs
`NativeFilesystem` on native (None on wasm — caller supplies OPFS).
`SharedFilesystem = Arc<dyn Filesystem>`.

**`EncryptedFilesystem`** (all targets) = seed-keyed AES-256-GCM at rest over any
impl: `LHE1‖nonce‖ct`; read sniffs the magic → decrypt (tamper = clear error),
else legacy plaintext passes through FOREVER. Key tag `localharness/v0/opfs-at-rest`
(pinned); `wallet_store::{load,create_and_persist,import}` install it over OPFS;
seedless origins stay plaintext. NEVER encrypts pinned `EXEMPT_FILES` —
`.lh_wallet` (the seed IS the key root; sealing it bricks identity), the
pre-wallet boot files (`.lh_owner`/`.lh_linked_owner`/`.lh_device_key`), the 2
model artifacts.

## Documentation SOP

Five surfaces — keep in sync on every change: **docs.rs** (`///` in source, every
`pub` item gets a one-liner) · **README.md** (quick start, features, architecture)
· **CLAUDE.md** (internal map + gotchas, this file, under 40K) · **llms.txt**
(`web/llms.txt`: caps, RPC, registry) · **CHANGELOG.md**
(per-version, Keep-a-Changelog).

**When to update what:** new pub API → `///` (+README if surface changes); new
module → CLAUDE.md tree; new agent tool → `llms.txt` + session prompt; new facet →
CLAUDE.md on-chain + `contracts/README.md` + `llms.txt`; release → CHANGELOG.

**Verify before release:** `cargo doc --no-deps 2>&1 | grep "warning.*missing"`.
