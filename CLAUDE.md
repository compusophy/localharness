# CLAUDE.md

Project context for Claude Code sessions. Read this first.

## What this is

`localharness` is a Rust-native agent SDK for Google's Gemini API
**and** a self-sovereign browser-resident agent platform built on top
of it. One crate; `cargo add` and you have an agent loop with streaming
text, tool calling, hooks, policies, triggers, MCP integration, and
context compaction. Build with the `browser-app` feature on wasm32 and
you also get the live IDE at `<name>.localharness.xyz`.

- Published on [crates.io/crates/localharness](https://crates.io/crates/localharness)
  (current: **0.10.x**)
- Repo at [github.com/compusophy/localharness](https://github.com/compusophy/localharness)
- Native target: stable Rust 1.85+, tokio-driven
- wasm32 target: same crate compiles to the browser
- Live demo: [`localharness.xyz`](https://localharness.xyz/) —
  marketing apex + wildcard `*.localharness.xyz` for per-user agents
- On-chain registry: EIP-2535 Diamond on Tempo Moderato testnet at
  [`0x6f2858…2930`](https://moderato.tempo.xyz/address/0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930)
  (fresh deploy 2026-05-25; supersedes the previous diamond at
  `0xed7a2d…c656d` which carried abandoned test registrations)

## Repo layout

```
src/                       library crate
├── lib.rs                 re-exports + module roots
├── agent.rs               Agent facade (Layer 1)
├── conversation.rs        Conversation + ChatResponse (Layer 2)
├── connections/           Connection / ConnectionStrategy traits (Layer 3)
├── content.rs             Content, Media, Part (user-facing message types)
├── tools.rs               Tool trait + ToolRunner + ClosureTool
├── hooks.rs               6 hook traits + HookRunner
├── policy.rs              Predicate / Policy / Decision + workspace_only
├── triggers.rs            Trigger trait + TriggerRunner + every()
├── runtime.rs             cfg-gated spawn helper + MaybeSendSync marker
├── filesystem/            Filesystem trait + Native + OPFS impls
├── types.rs               wire-adjacent enums (BuiltinTool, Step, etc.)
├── error.rs               Error + Result
├── wallet.rs              secp256k1 keypair + BIP-39 + RLP encoding
│                          (feature = "wallet"; works on every target)
├── registry.rs            JSON-RPC client for the on-chain Diamond
│                          (feature = "wallet"; works on every target)
├── app/                   browser-resident IDE — gated on
│   ├── mod.rs             `browser-app` feature + wasm32 target
│   ├── templates.rs       all maud HTML
│   ├── dom.rs             web-sys helpers (swap_inner, …)
│   ├── events.rs          delegated click/keydown/submit/input dispatch
│   ├── chat.rs            chat-turn streaming
│   ├── history.rs         OPFS-persisted conversation
│   ├── opfs.rs            file browser + inline editor
│   ├── key_store.rs       Gemini API key in OPFS
│   ├── owner.rs           legacy local-UUID owner marker
│   ├── tenant.rs          hostname classifier (apex / tenant / other)
│   ├── wallet_store.rs    master wallet persisted to apex OPFS
│   ├── signer.rs          postMessage signer service at apex/?signer=1
│   └── verify.rs          subdomain-side iframe owner verification
└── backends/
    ├── gemini/
    │   ├── api.rs         GeminiClient + SSE decoder (CRLF + LF tolerant)
    │   ├── wire.rs        REST request/response types
    │   ├── loop.rs        run_turn — the inner agent loop
    │   ├── compaction.rs  history summarisation
    │   ├── tools/         13 built-in tools
    │   └── mod.rs         GeminiConnectionStrategy + GeminiConnection
    └── mcp/               stdio MCP client (native-only)

contracts/                 Foundry project for the on-chain registry
├── src/
│   ├── Diamond.sol                       EIP-2535 proxy
│   ├── interfaces/                       IDiamond, IDiamondCut,
│   │                                     IDiamondLoupe, IERC165, IERC173
│   ├── libraries/
│   │   ├── LibDiamond.sol                proxy storage + cut impl
│   │   ├── LibRegistryStorage.sol        registry state (slot v1)
│   │   └── LibTbaConfigStorage.sol       TBA config (slot v1)
│   ├── facets/
│   │   ├── DiamondCutFacet.sol           owner-only upgrade
│   │   ├── DiamondLoupeFacet.sol         introspection + supportsInterface
│   │   ├── OwnershipFacet.sol            EIP-173 owner()/transfer
│   │   ├── LocalharnessRegistryFacet.sol register / ownerOfName / ...
│   │   ├── ERC721Facet.sol               ERC-721 + Metadata surface
│   │   └── TbaFacet.sol                  ERC-6551 token-bound accounts
│   ├── erc6551/                          vendored EIP-6551 reference
│   │   ├── IERC6551Registry.sol
│   │   ├── ERC6551Registry.sol
│   │   └── ERC6551Account.sol
│   ├── upgradeInitializers/
│   │   └── DiamondInit.sol               one-shot init for ERC-165 flags
│   └── LocalharnessRegistry.sol          legacy flat contract (archived)
├── script/
│   ├── DeployDiamond.s.sol               from-scratch diamond deploy
│   ├── AddErc721Facet.s.sol              cut ERC-721 surface
│   ├── AddTbaFacet.s.sol                 cut 6551 + helper
│   └── Deploy.s.sol                      legacy flat deploy (archived)
└── README.md                             architecture write-up

web/                       static site for Vercel
├── index.html             bootstrap shell (CSS + #root + init())
└── pkg/                   wasm-pack output (gitignored; built locally
                           and uploaded by `vercel deploy`):
                           localharness.js + localharness_bg.wasm

scripts/
├── release.{ps1,sh}       atomic release tool (see RELEASING.md)
├── build-web.{ps1,sh}     wasm-pack build → web/pkg/
└── probe-gemini.ps1       isolate request-shape vs. response-parse bugs

RELEASING.md               step-by-step + recovery table
CHANGELOG.md               per-version changes (Keep-a-Changelog)
vercel.json                static-deploy config (no build step)
.vercelignore              keep target/ + Cargo.* out of the upload
```

The historical design docs (`DESIGN.md` 0.2.x SDK plan,
`DESIGN_M5_PLUS.md` M5+ platform plan, `UPSTREAM.md` Python upstream
history) were dropped from the tree at 0.10.1 — every layer they
sketched shipped. Anything you need from them is preserved under git
tags `v0.1.0`–`v0.10.0`.

## Build / test / run

```sh
cargo build                                                   # native (default features)
cargo test                                                    # full test suite
cargo check --no-default-features --target wasm32-unknown-unknown  # wasm guardrail
./scripts/build-web.sh                                        # rebuild wasm bundle
vercel deploy --prod --yes                                    # deploy web/
```

## Cargo features

- `native` (default): enables `tokio` multi-thread + process + fs +
  io-util, plus the `walkdir` and `tempfile` deps. Required for
  `run_command` and the MCP stdio bridge, and is what lets the 8 fs
  builtins (list_directory, view_file, find_file, search_directory,
  create_file, edit_file, delete_file, rename_file) register a
  `NativeFilesystem` by default.
- `wallet` (off by default): exposes `pub mod wallet` (secp256k1 +
  BIP-39 + RLP) and `pub mod registry` (JSON-RPC client for the
  Diamond). Pulls in `k256 + sha3 + rand_core + bip39`. Works on
  every target — `sleep_ms` is cfg-gated to `tokio::time::sleep` on
  native, `setTimeout` on wasm.
- `browser-app` (off by default): compiles the `src/app/` module into
  the crate as a wasm cdylib — the browser IDE. Pulls in `maud` for
  HTML templating, `pulldown-cmark` for markdown, plus the `wallet`
  feature transitively. Has no effect on a native build. Built by
  `scripts/build-web.{sh,ps1}` via `wasm-pack build
  --no-default-features --features browser-app`.
- (wasm targets) automatically drop `walkdir`/`tempfile` and add
  `wasm-bindgen-futures`, `uuid/js`, `getrandom/js` via target-cfg.

Library callers on wasm32 who only want the SDK (not the browser app)
depend with `default-features = false` and skip `browser-app`.
Off-bundle consumers (CLI indexers, back-ends) that want to query the
on-chain registry pick `default-features = false, features = ["wallet"]`.

## The wasm story (M2.5)

The crate compiles to `wasm32-unknown-unknown` because:

- `src/runtime.rs::spawn` cfg-gates `tokio::spawn` (native) vs.
  `wasm_bindgen_futures::spawn_local` (wasm).
- `src/runtime.rs::MaybeSendSync` is `Send + Sync` on native and
  empty on wasm. Every trait that used to require `: Send + Sync`
  now requires `: MaybeSendSync`.
- Every `#[async_trait]` is `cfg_attr`'d to use `?Send` on wasm so
  browser-fetch futures (which aren't `Send`) can satisfy the trait
  method signatures.
- `Connection::subscribe_steps` returns a `StepStream` type alias
  that maps to `BoxStream` (native) or `LocalBoxStream` (wasm).
- `JoinHandle` storage and abort logic is cfg-gated; on wasm we
  fire-and-forget via `spawn_local`.
- Tools that need OS primitives are gated behind `feature = "native"`:
  the 8 fs builtins (list/view/find/search/create/edit/delete/rename),
  `run_command`, MCP. The 4 portable ones (`ask_question`, `finish`,
  `generate_image`, `start_subagent`) work on both targets.

When adding new traits or `tokio::spawn` calls, mirror these patterns
or wasm will break silently (the gated modules don't trip in a default
`cargo check`).

## Common gotchas

- **PowerShell 5.1 stderr trap.** `release.ps1` wraps native commands
  in `Invoke-Native` because PS5 turns every cargo stderr line into a
  terminating error. Don't call `cargo`/`git`/`gh` directly inside
  the script.
- **Gemini 3.x `thought: false` parts.** The wire `Part` enum is
  untagged; `Part::Thought { thought: bool, .. }` is declared
  *before* `Part::Text { text }`. Gemini 3.x stamps every part with a
  `thought` field, so a normal text part deserializes into
  `Part::Thought { thought: false, text: Some(...), .. }`. Consumers
  must handle that variant explicitly.
- **SSE on wasm uses CRLF.** Browser fetch surfaces Gemini's SSE
  with `\r\n\r\n` frame separators. `GeminiSseStream::take_frame`
  now matches both `\n\n` and `\r\n\r\n`. Don't regress to LF-only.
- **`max-age=immutable` on `/pkg/*` was a footgun.** `vercel.json`
  uses `max-age=0, must-revalidate` so redeploys actually take effect
  without forcing a hard-reload. Add a version query string before
  re-enabling long caching.
- **The release script only commits `Cargo.toml` + `Cargo.lock` +
  `CHANGELOG.md`.** Anything else that needs to ship in a release
  must be committed *before* invoking the script. See RELEASING.md.

## Release process

```sh
# 1. Land all the feature work as normal commits.
# 2. Edit CHANGELOG.md - add `## [X.Y.Z]` heading (no date - script adds).
# 3. Run the atomic release script.
./scripts/release.sh X.Y.Z          # bash / git-bash
pwsh scripts/release.ps1 -Version X.Y.Z   # PowerShell on Windows
```

The script does pre-flight checks → version bump → cargo verify →
commit → tag → push → cargo publish → GH release in one shot. If it
fails mid-way, consult the recovery table in `RELEASING.md`; don't
hand-fix.

## The browser app

Compiled into the crate as `src/app/`, gated on `feature = "browser-app"`
plus `target_arch = "wasm32"`. Module list in the repo-layout block
above; the per-module summaries below cover the load-bearing pieces.

**Design rule: no imperative DOM manipulation.** All HTML comes from
`maud` templates; the only DOM operations are `set_inner_html` /
`set_outer_html` / `insert_adjacent_html` targeted at fixed element
ids (HTMX-style fragment swaps). One delegated `click` listener, one
`keydown`, one `submit`, one `input` listener at the document level
handle every interaction by reading `data-action` and `data-arg`
attributes off the event target's ancestor chain. There are zero
`Closure::wrap` calls outside of those four listeners.

Mount-time routing in `mod.rs::mount`:

1. If `?signer=1` → render minimal signer chrome, install
   postMessage listener (`signer::install_signer_listener`), return.
   The tab is now a cross-origin signing service. If no wallet has
   been created at the apex origin yet, `paint_signer` renders the
   `signer_no_identity` chrome and `signer::build_response` errors
   on every challenge — we never silently generate a wallet here.
2. Else, classify hostname via `tenant::current()`:
   - `Host::Apex` (`localharness.xyz`) → identity-gated apex chrome.
     `paint_apex` calls `wallet_store::load()` (never creates) — fresh
     visitors see the `identity_sidecar` with `[Create identity]` +
     `[Import existing seed]`, and the claim form is rendered with
     `disabled` input + submit. Wallet creation only happens via the
     explicit `Action::CreateIdentity` or `Action::ImportSeed`
     dispatch paths, both of which re-run `paint_apex` so the form
     unlocks and the "your agents" list fetches in the background.
   - `Host::Tenant(name)` → check `.lh_owner` marker:
     - Missing + `?claim=1` → auto-claim, paint full app.
     - Missing + no hint → paint "claim this name" prompt.
     - Present → paint full chat app.
     Then `kick_verification` runs in the background: queries
     on-chain owner via `registry::owner_of_name`, runs
     `verify::verify_owner` (iframe sign challenge), updates the
     `#verify-pill` and (if visitor) swaps `#input-region` for a
     read-only banner. Also fetches `tba_of_name` for the 💰 pill.
   - `Host::Other` (Vercel preview, localhost) → paint full chat
     app, no verification.

**Identity-gate invariant.** `wallet_store::load_or_create` no longer
exists. The two callers are `wallet_store::load()` (pure read,
returns `Option<MasterWallet>`) and `wallet_store::create_and_persist()`
(generates + writes, only invoked from `Action::CreateIdentity`).
Don't reintroduce a load-or-create helper — silent wallet generation
on a marketing-page visit was the bug the gate fixes.

Build: `wasm-pack build . --target web --out-dir web/pkg --release
--no-default-features --features browser-app`. wasm-opt is disabled in
`[package.metadata.wasm-pack.profile.release]` because the wasm-pack-
bundled wasm-opt rejects post-MVP features that modern rustc emits.

## The on-chain stack

The registry lives at one address forever — the diamond proxy at
`0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930` on Tempo Moderato
testnet (chain id 42431, RPC `https://rpc.moderato.tempo.xyz`).
(Predecessor diamond at `0xed7a2d…c656d` carried abandoned test
registrations and is no longer referenced by the bundle.)
Facets are added/removed via `diamondCut`; the wasm bundle's
`registry::REGISTRY_ADDRESS` constant doesn't change.

Currently cut in:

- **DiamondCutFacet** — owner-only `diamondCut(...)` (upgrades).
- **DiamondLoupeFacet** — introspection + `supportsInterface`.
- **OwnershipFacet** — EIP-173 `owner()` + `transferOwnership`.
- **LocalharnessRegistryFacet** — `register / ownerOfName /
  ownerOfId / idOfName / nameOfId / idOf / setMetadata / nextId /
  metadata / isTaken`. Mints emit `Transfer(0, owner, tokenId)` so
  the ERC-721 facet stays consistent.
- **ERC721Facet** — full ERC-721 + Metadata. Every name is an NFT.
  `tokenURI(id)` → `https://<name>.localharness.xyz/`.
- **TbaFacet** — wraps EIP-6551. `tokenBoundAccount(id)` and
  `tokenBoundAccountByName(name)` return the deterministic
  counterfactual account address. `createTokenBoundAccount(id)`
  actually deploys it (anyone can call, idempotent).

ERC-6551 reference contracts (separate addresses, configured via
`TbaFacet::setTbaConfig`):
- Registry: `0xc7cadc487eeb06fe8807104443b2f76b45c041d6`
- Account impl: `0x8ad49e86b2da342a20c49538ef727eeab304d7f4`
  (CALL-only — DELEGATECALL is explicitly disabled to avoid the
  self-destruct footgun).

Adding a new facet: write `LibXyzStorage` at a fresh
`keccak256("localharness.xyz.storage.v1")` slot, write the facet,
forge build, write a one-off cut script following `AddTbaFacet.s.sol`
as a template, deploy. See `contracts/README.md` for the full
walkthrough.

## What's planned

The SDK runtime (0.2.x–0.6.x) and the in-tree browser IDE (0.7.x)
shipped. The platform layer (subdomains + master wallet + on-chain
registry + ERC-721 NFTs + ERC-6551 token-bound wallets + iframe
signer + visitor lockdown) shipped through 0.10.0. What's next:

- **MPP / x402 payment hooks.** A pre-tool-call hook that requires
  a payment to the agent's TBA before the LLM call executes, or an
  agent-pays-agent flow over Stripe's MPP (preferred per user) or
  Coinbase's x402. Either fits behind the existing `Hook` trait;
  the on-chain plumbing exists already.
- **ERC-8004 reputation + validation facets.** Cut into the diamond
  alongside the existing registry facets. Lets agents accrue
  reputation that other agents can read; validators stake to
  re-execute claims.
- **TBA-driven actions in the bundle.** UX for "let your agent send
  this transaction from its TBA" — the master wallet signs a
  TBA.execute payload, the bundle wires the RPC. Mostly a UI piece;
  the contract surface is ready.
- **Second backend** (Anthropic, OpenAI, or local). The
  `Connection` / `ConnectionStrategy` abstractions are in place;
  validating them with a non-Gemini implementation is overdue.
- **Tool-call activity in restored transcripts.** `TranscriptEntry`
  drops FunctionCall / FunctionResponse on replay today — the
  agent's context is correct but the user can't see prior tool use.
- **At-rest encryption.** Wallet-derived sym key over OPFS contents
  so XSS-equivalent attacks on origins can't trivially exfiltrate.

## Filesystem trait

The 8 fs-shaped builtins (`list_directory`, `view_file`, `find_file`,
`search_directory`, `create_file`, `edit_file`, `delete_file`,
`rename_file`) call into `crate::filesystem::Filesystem` instead of
`tokio::fs` directly. The trait surface:

- `read`, `write_atomic`, `metadata`, `read_dir`, `walk`, `delete`,
  `rename` (default impl is read + write + delete; NativeFilesystem
  overrides with `tokio::fs::rename` for atomicity)

Two implementations ship:

- **`NativeFilesystem`** (gated on `feature = "native"`): `tokio::fs`
  + `walkdir` + `tempfile`; atomicity via tempfile + rename.
- **`OpfsFilesystem`** (wasm32 only): Origin Private File System via
  `web-sys`; atomicity via `FileSystemWritableFileStream.close()` swap.

`GeminiConnectionStrategy::connect` honors a caller-supplied
`Filesystem` via `with_filesystem`, otherwise auto-installs
`NativeFilesystem` on native (or `None` on wasm, where the caller is
expected to supply OPFS — the browser app does so). Plug-in impls
(mocks for tests, custom backends) implement the trait and hand a
`SharedFilesystem = Arc<dyn Filesystem>` via the builder.
