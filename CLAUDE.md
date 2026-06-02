# CLAUDE.md

Project context for Claude Code sessions. Read this first.

## What this is

`localharness` is a Rust-native agent SDK for Google's Gemini API **and**
a self-sovereign browser-resident agent platform built on it. One crate;
`cargo add` gives an agent loop with streaming text, tool calling, hooks,
policies, triggers, MCP, and context compaction. Build with `browser-app`
on wasm32 and you also get the live IDE at `<name>.localharness.xyz`.

- [crates.io/crates/localharness](https://crates.io/crates/localharness) (current: **0.17.x**)
- [github.com/compusophy/localharness](https://github.com/compusophy/localharness)
- Native: stable Rust 1.85+, tokio-driven. wasm32: same crate, browser.
- Live: [`localharness.xyz`](https://localharness.xyz/) — marketing apex
  + wildcard `*.localharness.xyz` for per-user agents.
- On-chain registry: EIP-2535 Diamond on Tempo Moderato testnet at
  `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` (full reset 2026-06-01 —
  brand-new diamond + token + 6551 infra; every prior address abandoned).

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
├── wallet.rs              secp256k1 + BIP-39 + RLP (feature "wallet"; all targets)
├── registry.rs            JSON-RPC client for the Diamond + Tempo Tx submission +
│                          credit/session/x402/device helpers (feature "wallet")
├── x402_hook.rs           app-injected x402 signer for `call_agent` (feature "wallet")
├── tempo_tx.rs            Tempo Transaction (tx 0x76) encoder; see Tempo section
├── rustlite/              Rust-subset → wasm compiler (in-crate)
│   ├── mod.rs             compile(source) → wasm bytes top-level API
│   ├── token.rs           token types; lexer.rs byte-level lexer
│   ├── ast.rs             full AST; parser.rs recursive descent + precedence
│   ├── typecheck.rs       scope-based type resolution + mutability
│   ├── codegen.rs         wasm binary emitter (sections, opcodes, LEB128)
│   └── loader.rs          wasm32-only cartridge instantiation
├── app/                   browser-resident IDE (browser-app + wasm32)
│   ├── mod.rs             mount-time routing (see browser app section)
│   ├── templates.rs       all maud HTML
│   ├── dom.rs             web-sys helpers (swap_inner, …)
│   ├── events.rs          delegated click/keydown/submit/input dispatch
│   ├── chat.rs            chat-turn streaming + system prompt
│   ├── history.rs         OPFS-persisted conversation (tool-call replay)
│   ├── opfs.rs            file browser + inline editor; click-to-DISPLAY for .wasm/.rl/.html
│   ├── display.rs         framebuffer surface: runs wasm cartridges + rasterizes HTML; 5x7 font
│   ├── key_store.rs       Gemini API key in OPFS
│   ├── owner.rs           legacy local-UUID owner marker
│   ├── tenant.rs          hostname classifier (apex / tenant / other)
│   ├── wallet_store.rs    master wallet persisted to apex OPFS
│   ├── signer.rs          postMessage signer service at apex/?signer=1
│   ├── agent_rpc.rs       inter-agent RPC endpoint (?rpc=1 URL mode)
│   ├── encryption.rs      AES-256-GCM at-rest encryption + ECIES via WebCrypto
│   ├── system_prompt.rs   per-tenant custom system prompt (.lh_system_prompt.txt)
│   ├── tool_allowlist.rs  per-agent tool restriction (.lh_tool_allowlist.txt)
│   ├── sponsor.rs         embedded sponsor private key for fee_payer (testnet only)
│   └── verify.rs          subdomain-side iframe owner verification
└── backends/
    ├── gemini/
    │   ├── api.rs         GeminiClient + SSE decoder (CRLF + LF tolerant)
    │   ├── wire.rs        REST request/response types
    │   ├── loop.rs        run_turn — the inner agent loop
    │   ├── compaction.rs  history summarisation
    │   ├── tools/         built-in Tool impls (one per BuiltinTool; incl. call_agent,
    │   │                  compile_rustlite, render_html; run_cartridge drives DISPLAY)
    │   └── mod.rs         GeminiConnectionStrategy + GeminiConnection
    └── mcp/               stdio MCP client (native-only)

contracts/                 Foundry project for the on-chain registry
├── src/
│   ├── Diamond.sol        EIP-2535 proxy; interfaces/ IDiamond/Loupe/Cut/ERC165/173
│   ├── libraries/         LibDiamond + one LibXyzStorage per facet (slot convention below)
│   ├── facets/            DiamondCut, DiamondLoupe, Ownership, LocalharnessRegistry,
│   │                      ERC721, Tba, Feedback, MainIdentity, Redeem, Session,
│   │                      CreditMeter, X402, DeviceRegistry, Release, Pairing (see on-chain)
│   ├── erc6551/           vendored EIP-6551 reference (IRegistry, Registry, Account)
│   ├── upgradeInitializers/DiamondInit.sol  one-shot ERC-165 flag init
│   └── LocalharnessRegistry.sol             legacy flat contract (archived)
├── script/               DeployDiamond.s.sol + one Add<Facet>.s.sol cut script per facet
└── README.md             architecture write-up

web/                       static site for Vercel
├── index.html             bootstrap shell (CSS + #root + init())
└── pkg/                   wasm-pack output (gitignored; built locally, uploaded by deploy)

proxy/                     $LH credit proxy — SEPARATE Vercel project ("proxy") at
│                          https://proxy-tau-ten-15.vercel.app. The ONE accepted
│                          off-chain component. LIVE. (See Credit proxy section.)
├── api/gemini.ts          Vercel Edge: transparent Gemini passthrough, holds platform key
├── package.json / vercel.json / README.md / .gitignore

scripts/                   release.{ps1,sh} atomic release; build-web.{ps1,sh} wasm bundle;
                           probe-gemini.ps1; harvest-feedback.{ps1,sh} cast-logs wrapper

examples/tempo_tx_live.rs  live harness vs Moderato; source of truth for tempo_tx encoding

design/                    main-identity.md; agent-writes-rust.md (rustlite grammar+ABI);
                           launch-1.0.md (1.0 plan); paymaster.md (superseded by Tempo AA)

RELEASING.md / CHANGELOG.md / vercel.json (static-deploy) / .vercelignore
```

Historical design docs (`DESIGN.md`, `DESIGN_M5_PLUS.md`, `UPSTREAM.md`)
were dropped at 0.10.1 — every layer shipped. Preserved under git tags
`v0.1.0`–`v0.10.0`.

## Build / test / run

```sh
cargo build                                                   # native (default features)
cargo test                                                    # full test suite
cargo check --no-default-features --target wasm32-unknown-unknown  # wasm guardrail
./scripts/build-web.sh                                        # rebuild wasm bundle
vercel deploy --prod --yes                                    # deploy web/
```

## Cargo features

- `native` (default): `tokio` multi-thread + process + fs + io-util, plus
  `walkdir` and `tempfile`. Required for `run_command`, the MCP stdio
  bridge, and registering a `NativeFilesystem` by default (the 8 fs
  builtins: list_directory, view_file, find_file, search_directory,
  create_file, edit_file, delete_file, rename_file).
- `wallet` (off): exposes `pub mod wallet` (secp256k1 + BIP-39 + RLP) and
  `pub mod registry` (JSON-RPC client for the Diamond). Pulls `k256 + sha3
  + rand_core + bip39`. Works on every target — `sleep_ms` is cfg-gated to
  `tokio::time::sleep` (native) / `setTimeout` (wasm).
- `browser-app` (off): compiles `src/app/` as a wasm cdylib — the browser
  IDE. Pulls `maud`, `pulldown-cmark`, plus `wallet` transitively. No
  effect on native. Built by `scripts/build-web.{sh,ps1}` via
  `wasm-pack build --no-default-features --features browser-app`.
- (wasm targets) automatically drop `walkdir`/`tempfile` and add
  `wasm-bindgen-futures`, `uuid/js`, `getrandom/js` via target-cfg.

Library callers on wasm32 wanting only the SDK depend with
`default-features = false` and skip `browser-app`. Off-bundle consumers
querying the registry pick `default-features = false, features = ["wallet"]`.

## The wasm story (M2.5)

The crate compiles to `wasm32-unknown-unknown` because:

- `src/runtime.rs::spawn` cfg-gates `tokio::spawn` (native) vs.
  `wasm_bindgen_futures::spawn_local` (wasm).
- `src/runtime.rs::MaybeSendSync` is `Send + Sync` on native and empty on
  wasm. Every trait that used to require `: Send + Sync` now requires
  `: MaybeSendSync`.
- Every `#[async_trait]` is `cfg_attr`'d to use `?Send` on wasm so
  browser-fetch futures (which aren't `Send`) satisfy the trait methods.
- `Connection::subscribe_steps` returns a `StepStream` type alias mapping
  to `BoxStream` (native) or `LocalBoxStream` (wasm).
- `JoinHandle` storage and abort logic is cfg-gated; on wasm we
  fire-and-forget via `spawn_local`.
- Tools needing OS primitives are gated behind `feature = "native"`: the 8
  fs builtins, `run_command`, MCP. The 4 portable ones (`ask_question`,
  `finish`, `generate_image`, `start_subagent`) work on both targets.

When adding new traits or `tokio::spawn` calls, mirror these patterns or
wasm breaks silently (the gated modules don't trip in a default `cargo check`).

## Common gotchas

- **Gemini model IDs flip — verify against the live API, never trust
  memory.** `DEFAULT_MODEL` is `gemini-3.5-flash` (as of 2026-05-29).
  `gemini-2.5-flash` now 400s; in the 0.10.x era it was the reverse.
  Before changing/defending a model constant, `curl` the live
  `:generateContent` endpoint. If the user says a model is wrong, TEST
  THEIRS FIRST.
- **Gemini rejects union-type tool schemas with a 400 — it bricks ALL
  chat.** Function-declaration `input_schema` must use a single `type`
  (NOT `["string","null"]`), and no `additionalProperties` / `$schema` /
  `$ref` / `oneOf`/`anyOf`/`allOf`. `configure_agent` shipped a union
  type and 400'd every turn. Guard: `cargo test
  builtin_tool_schemas_have_no_union_types` (backends/gemini/tools/mod.rs)
  lints every builtin schema, network-free. `minimum`/`maximum`/nested
  objects+arrays are fine.
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

The script does pre-flight → version bump → cargo verify → commit → tag →
push → cargo publish → GH release in one shot. On mid-way failure consult
the recovery table in `RELEASING.md`; don't hand-fix.

## The browser app

Compiled as `src/app/`, gated on `feature = "browser-app"` + `wasm32`.

**Design rule: no imperative DOM manipulation.** All HTML comes from
`maud` templates; the only DOM ops are `set_inner_html` / `set_outer_html`
/ `insert_adjacent_html` targeted at fixed element ids (HTMX-style fragment
swaps). One delegated `click` / `keydown` / `submit` / `input` listener at
the document level handles every interaction by reading `data-action` /
`data-arg` off the target's ancestor chain. Zero `Closure::wrap` calls
outside those four listeners.

**Mount-time routing (`mod.rs::mount`):**

1. `?signer=1` → render minimal signer chrome, install postMessage
   listener (`signer::install_signer_listener`), return — the tab is now a
   cross-origin signing service. If no apex wallet exists yet, `paint_signer`
   renders `signer_no_identity` and `signer::build_response` errors on every
   challenge — never silently generate a wallet here.
2. Else classify hostname via `tenant::current()`:
   - `Host::Apex` (`localharness.xyz`) → identity-gated apex chrome.
     `paint_apex` calls `wallet_store::load()` (never creates) — fresh
     visitors see `identity_sidecar` with `[Create identity]` + `[Import
     existing seed]`, claim form `disabled`. Wallet creation only via
     `Action::CreateIdentity` / `Action::ImportSeed`, both re-running
     `paint_apex`.
   - `Host::Tenant(name)` → check `.lh_owner`: missing+`?claim=1` →
     auto-claim; missing+no hint → "claim this name" prompt; present →
     full chat app. Then `kick_verification` (background) queries on-chain
     owner via `registry::owner_of_name`, runs `verify::verify_owner`
     (iframe sign challenge), updates `#verify-pill`, and (visitors) swaps
     `#input-region` for a read-only banner. Fetches `tba_of_name` for 💰.
   - `Host::Other` (Vercel preview, localhost) → full chat app, no verify.

**Two surfaces per subdomain (public face vs studio).** Role-based routing
keyed on `owner.is_some()` (local claim, refined by verification):
- **Owner** → lands in the **studio** by default. Never auto-hijacked into
  fullscreen. Previews the public face via `?view=public` (a `[view public]`
  header link) which paints the fullscreen face with a `[studio]` escape
  (→ `?edit=1`).
- **Visitor** → only ever sees the **public face**. No studio, no edit door.

`paint_public_face` paints the resolved face. `resolve_public_face(name)`
reads the **on-chain choice** under `keccak256("localharness.public_face")`
(`registry::public_face_of`) — `directory` / `app` / `html` — preferring
the local working copy (owner previews unpublished edits) else published.
Returns a `PublicFace` enum:
- **`Cartridge(wasm)`** — `app.rl` (local) or `app_wasm_of`;
  `paint_cartridge_fullscreen` → `app_fullscreen` + `display::run_in_root_canvas`.
- **`Html(src)`** — `index.html` (local) or `public_html_of` (key
  `keccak256("localharness.public.html")`); `paint_html_fullscreen` →
  `app_fullscreen` + `display::render_html_in_root_canvas`.
- **`Directory`** — `paint_public_landing` (`templates::public_landing`):
  profile landing — name, owner (MAIN name when differs), TBA wallet, and
  the owner's other agents (siblings via `registry::list_owned_tokens`).

UNSET infers "cartridge if one exists, else directory". `owner_overlay`
gates the `[studio]` link. `Host::Other` uses `try_paint_app` (local
`app.rl` only, no on-chain resolution).

**Picker (admin → agent → "public face").** From `templates::admin_app_section`:
`[directory] [publish app] [publish html]` → `Action::SetPublicFace(choice)`
→ `events::run_set_public_face`. `directory` sets only the choice;
`app`/`html` compile/read local `app.rl`/`index.html` and publish it **plus**
set the choice in ONE sponsored Tempo tx (two `setMetadata` calls).
`refresh_public_face_status` reflects the current choice on admin open.

**Second-device owner upgrade.** A seed-bearing owner on their own
subdomain from a device WITHOUT `.lh_owner` is treated as a visitor.
`paint_tenant` then fires `redirect_to_studio_if_owner` (background): if
`verify::verify_owner` proves control via the apex signer, it navigates to
`?edit=1`. Skipped when the device already claims ownership (so a deliberate
`?view=public` never bounces). The agent makes a subdomain "become" an app
by writing the `run_cartridge` source to `app.rl` via `create_file` — only
on an explicit "make this my permanent app" request.

**Cross-visitor publishing (on-chain).** Local `app.rl`/`index.html` are
owner-device working copies; *visitors* see the published bytes, stored in
the registry diamond under `metadata(tokenId, key)` — no new facet, the
existing owner-gated `setMetadata(uint256,bytes32,bytes)`. Keys:
`keccak256("localharness.app.wasm")`, `keccak256("localharness.public.html")`,
`keccak256("localharness.public_face")`. Generic `registry::{metadata_bytes_of,
encode_set_metadata_bytes}` back the typed `{app_wasm_of, public_html_of,
public_face_of}` + `encode_set_*`. Published via a sponsored Tempo tx (owner
signs sender_hash through the apex iframe; sponsor pays).

**Identity-gate invariant.** `wallet_store::load_or_create` no longer
exists. The two callers are `wallet_store::load()` (pure read → `Option<
MasterWallet>`) and `wallet_store::create_and_persist()` (generates +
writes, only from `Action::CreateIdentity`). Don't reintroduce a
load-or-create helper — silent wallet generation on a marketing-page visit
was the bug the gate fixes.

**Device linking is now seed-adoption via QR (Option A).** The desktop
encrypts its seed under a one-time code; a QR fragment carries the
ciphertext to `localharness.xyz/?adopt=1#s=...`; the other device types the
code to import the SAME seed. This supersedes the old on-chain PairingFacet
device-key flow (per-origin `.lh_device_key`, ECIES-wrap-to-device Gemini
key), which is now dormant (facet still cut — see on-chain section).

Build: `wasm-pack build . --target web --out-dir web/pkg --release
--no-default-features --features browser-app`. wasm-opt is disabled in
`[package.metadata.wasm-pack.profile.release]` because the bundled wasm-opt
rejects post-MVP features modern rustc emits.

## The on-chain stack

The registry lives at one address forever — the diamond proxy at
`0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` on Tempo Moderato testnet
(chain id 42431, RPC `https://rpc.moderato.tempo.xyz`). (Fully reset
2026-06-01 — brand-new diamond, `$LH` token, 6551 infra; every prior
address abandoned and no longer referenced.) Facets are added/removed via
`diamondCut`; `registry::REGISTRY_ADDRESS` doesn't change.

**Per-facet addresses are deliberately not pinned** — they churn on every
re-cut; the diamond address is the only durable handle. Query the live
facet set via DiamondLoupeFacet (`facets()` / `facetAddress(selector)`).

**Conventions (stated once):** each facet's storage sits at a fresh
`keccak256("localharness.<facet>.storage.v1")` slot in a `LibXyzStorage`
library; each is cut via its own `script/Add<Facet>.s.sol`. To add a facet:
write the storage lib, the facet, forge build, a cut script (template
`AddTbaFacet.s.sol`), deploy. See `contracts/README.md`.

Currently cut in (name → surface/behavior):

- **DiamondCutFacet** — owner-only `diamondCut(...)` upgrades.
- **DiamondLoupeFacet** — introspection + `supportsInterface`.
- **OwnershipFacet** — EIP-173 `owner()` + `transferOwnership`.
- **LocalharnessRegistryFacet** — `register / ownerOfName / ownerOfId /
  idOfName / nameOfId / idOf / setMetadata / nextId / metadata / isTaken`.
  Mints emit `Transfer(0, owner, tokenId)`. **Cost-gated**: `register` can
  pull `registrationCost()` $LH via `transferFrom` (owner-only
  `setRegistrationCost`); **currently 0 (FREE)** — set 0 on 2026-05-28 when
  the cost==0 branch registers with no approve/claim.
- **ERC721Facet** — full ERC-721 + Metadata; every name is an NFT.
  `tokenURI(id)` → `https://<name>.localharness.xyz/`.
- **TbaFacet** — wraps EIP-6551. `tokenBoundAccount(id)` /
  `tokenBoundAccountByName(name)` return the counterfactual address;
  `createTokenBoundAccount(id)` deploys it (anyone, idempotent).
- **MainIdentityFacet** — `registerMain(uint256) / clearMain() /
  mainOf(address) / mainNameOf(address) / isMain(uint256)`. The holder's
  primary identity NFT; auto-set on first-claim. See `design/main-identity.md`.
- **FeedbackFacet** — `submitFeedback(string)` emits `FeedbackSubmitted(
  address, uint256, string)`. Event-only; harvest via `cast logs`. Gas is
  the spam filter; 2048-byte cap.
- **CreditsFacet** — distribution for the `LocalharnessCredits` TIP-20
  token: `claimDaily() / canClaim / dailyAllowance / lastClaimDay /
  creditsToken()`; owner setters `setCreditsToken / setDailyAllowance`.
  Diamond holds `ISSUER_ROLE`; day boundary = `block.timestamp / 86400`
  (UTC, no cron). Currency = "credits" (NOT fee-eligible). Daily-claim UI
  removed from bundle but facet stays for future streaming/subscription.
- **RedeemFacet** — bootstraps $LH into a fresh wallet: owner loads
  `addRedeemCodes(bytes32[],uint256)`, holder calls `redeem(string code)`
  (mints via `ISSUER_ROLE`, burns code); owner-only `disableRedeemCodes`.
- **SessionFacet** — coarse time-boxed $LH sessions: `openSession()` pulls
  `sessionPrice()` and sets `expiry = now + sessionDuration()`; proxy reads
  `sessionExpiryOf(address)`. **Currently `sessionDuration=3600,
  sessionPrice=0`** (free beta). Owner-tunable setters.
- **CreditMeterFacet** — per-request $LH metering: `depositCredits(uint256)`
  tops up; `creditOf(address)` reads; `meter(address,uint256)` debits
  (meter-key only); owner-only `setMeter(address)`.
- **X402Facet** — x402 EIP-712 "exact" settlement in $LH (agent-to-agent).
  `settle(...)` verifies (EOA `ecrecover` + EIP-1271, one-shot nonce) and
  moves $LH payer→payee; `authorizationState`; `x402DomainSeparator()`
  (read live — binds chainId + diamond, so the reset changed it).
- **DeviceRegistryFacet** — enumerable linked-device index read in ONE
  call: `linkDevice / unlinkDevice / devicesOf(address) / isDeviceLinked`.
  Replaces `SignerAdded` log scraping (Tempo RPC caps at 100k blocks).
- **ReleaseFacet** — `releaseName(uint256 tokenId)`: owner-only burn that
  frees a name; **refuses the caller's MAIN**.
- **PairingFacet** (dormant — superseded by QR seed-adoption). v2 selector
  `announcePairing(bytes32,bytes)` (old `(bytes32)` left as harmless
  orphan) emits `PairingAnnounced(bytes32 indexed codeHash, address indexed
  device, ...)`. Event-only. Powered zero-copy device linking: phone opened
  `?pair=CODE`, generated a device key, announced (sponsored), desktop
  filtered by codeHash and `addSigner`'d it. Device key stored per-origin in
  `.lh_device_key` (raw hex, NOT the seed).

**Gemini key sync (per-MAIN, on-chain).** The sealed Gemini key lives under
the owner's **MAIN tokenId** (`mainOf(owner)`, fallback the name's own id),
NOT per-subdomain — every subdomain shares ONE key. On tenant paint,
`events::try_auto_restore_gemini_key` fetches the blob and decrypts via the
apex iframe (`open_key_via_iframe`, seed-derived) BEFORE the api-key modal
shows. Saving (`save_api_key_pressed`) best-effort `auto_sync_gemini_key`s
to the MAIN slot (resolver `events::gemini_key_slot_id`). (A device-key-only
phone got an ECIES-wrapped-to-device copy under
`keccak256("localharness.gemini_key.dev."||device_addr)` via
`registry::set_device_wrapped_key_sponsored` — part of the now-dormant
pairing path.)

**ERC-6551 reference contracts** (separate addresses via
`TbaFacet::setTbaConfig`; redeployed fresh in the reset):
- Registry: `0x2795810e5dfC8bC92Ef7fc9557F6c0699E11c3B3`
- Account impl: `0x86be7c44d1940F4dE53A738153A12FaAEa68B5a7`
  (`MultiSignerAccount` — CALL-only; additional-signer set on top of the
  NFT holder + EIP-1271 `isValidSignature`, so a MAIN can be controlled by
  multiple device EOAs without sharing the seed. Signer management is
  owner-only; signers bound to the enrolling holder
  (`_signerEnroller[signer] == owner()`), so an NFT transfer revokes prior
  device signers; `isValidSignature` rejects high-s (EIP-2). The bundle
  reads TBA addresses via the diamond, so a registry/impl swap needs no
  bundle change — but TBAs minted under prior infra resolve differently.)

## Credit proxy + $LH sessions / metering (LIVE)

The proxy runs at `https://proxy-tau-ten-15.vercel.app` (separate Vercel
project "proxy", TS Edge Function `proxy/api/gemini.ts`). Platform `$LH`
credits are the **primary** usage path; **BYOK** (own Gemini key) is the
fallback. The proxy is the ONE accepted off-chain component / only server;
everything else stays Tempo + the user's browser.

It's a transparent Gemini passthrough (same path/request shape) holding the
platform `GEMINI_API_KEY` in env. Auth = Ethereum personal-sign in the
`x-goog-api-key` header as `address:timestamp:signature`. The proxy
verifies the sig, then gates on EITHER an active SessionFacet session
(`sessionExpiryOf`) OR a CreditMeterFacet balance (`creditOf`); per-request
mode debits via the meter key (viem, EIP-1559) before streaming Gemini.

Bundle helpers (`src/registry.rs`): `redeem_sponsored`,
`open_session_sponsored`, `session_expiry_of`, `session_price`,
`deposit_credits_sponsored`, `credit_balance_of`.

End-to-end: redeem a code → `$LH` in wallet → `openSession()` (coarse) or
`depositCredits()` (per-request meter) → bundle calls the proxy with a
signed header → proxy verifies + checks session/meter → streams Gemini.
BYOK skips the proxy and talks to Gemini directly.

## x402 agent-to-agent settlement (LIVE)

`src/x402_hook.rs` is an app-injected signer wired into `call_agent`: when
one agent calls another, the hook signs the EIP-712 authorization so the
inter-agent call settles in $LH via X402Facet (above). Client helpers in
`registry.rs`: `x402_domain_separator`, `x402_digest`, `sign_x402`,
`settle_x402_sponsored`, `x402_authorization_state`.

## Device index + name release (LIVE)

- **DeviceRegistryFacet** — enumerable linked-device list in ONE call
  (`devicesOf` / `isDeviceLinked`). `registry::remove_signer_sponsored`
  also unlinks the index. Reads: `registry::devices_of`, `is_device_linked`.
- **ReleaseFacet** — `releaseName(tokenId)` owner-only burn (refuses MAIN).
  Helpers: `registry::release_name_sponsored`, `release_name_calldata`;
  `consolidate_into_main_sponsored` releases all non-MAIN holdings in one
  sponsored batch.

## Agent tools + destructive-action convention

Two subdomain-management tools (declared in `chat.rs::start_session`):
- **`list_subdomains()`** — read-only; enumerates the owner's holdings.
- **`release_subdomain(name, confirmation)`** — DESTRUCTIVE. Burns the name
  (ReleaseFacet `releaseName`). Requires `confirmation == name` (typed in
  chat), refuses the caller's MAIN, NOT granted to subagents.

Hard convention: **destructive / irreversible actions require a typed
confirmation that is never auto-filled** — the agent must ask the user to
type the exact value before proceeding. Mirror this for future destructive tools.

## Tempo Transactions + sponsorship (post-0.10.24)

The user-facing claim flow uses Tempo's **native** AA tx type (`0x76`) so
users hold ZERO of anything — no native gas, no TIP-20, nothing.
`src/app/sponsor.rs` signs as `fee_payer` and pays fees in AlphaUSD on
every user tx.

### Wire format (live-verified — see `[[tempo-tx-findings]]`)

```text
0x76 || rlp([
    chain_id, mpfpg, mfpg, gas_limit,
    calls,                // [[to, value, input], ...]
    access_list,          // EIP-2930
    nonce_key, nonce,     // Tempo's 2D nonce
    valid_before, valid_after,
    fee_token,            // 0x80 (empty) in sender hash if sponsored
    fee_payer_signature,  // 0x00 placeholder in sender hash; 0x80 or
                          // rlp([v,r,s]) in serialized tx
    aa_authorization_list,
    key_authorization?,   // truly optional; omit when None
    sender_signature      // flat 65 bytes (r||s||v with v=0/1)
])
```

Sender hash: `keccak256(0x76 || rlp([1..14_without_sender_sig]))`.
Fee-payer hash: `keccak256(0x78 || rlp([1..10, fee_token, sender_address,
aa_authorization_list, key_authorization?]))`. The spec page is missing
`aa_authorization_list` at position 13 of the fee_payer hash — discovered
by diffing against `wevm/ox`'s `TxEnvelopeTempo`. Captured in memory so we
don't relearn.

### $LH is TIP-20-shaped credit, NOT fee-token-eligible

Tempo's `fee_token` validation requires TIP-20 compliance AND `currency()
== "USD"`. `LocalharnessCredits` at `0x90B84c7234Aae89BadA7f69160B9901B9bc37B17`
(fresh in the 2026-06-01 reset) implements the TIP-20 surface (memo
transfers, supply cap, roles) but returns `currency() == "credits"`, so the
chain rejects it as a fee_token — intentional ($LH is in-system credits,
not gas). **AlphaUSD** (`0x20c0000000000000000000000000000000000001`)
remains the sponsor's fee_token. $LH supply is controlled — diamond holds
`ISSUER_ROLE`; mint paths are `CreditsFacet.claimDaily()` and
`RedeemFacet.redeem(code)`. Pre-reset tokens are orphaned; no migration.

### Sponsor key

Lives in `src/app/sponsor.rs` as a const. Same address as the deployer for
now (testnet acceptable). **Rotate before mainnet** — to a dedicated
low-budget sponsor wallet (small blast radius) or a different scheme
(WebAuthn passkey per user, Stripe-backed top-up, etc.). Tempo access keys
CANNOT sign as `fee_payer` — confirmed by reading their open-source SDK,
see `[[access-key-fee-payer-finding]]`. The fee_payer signature must come
from the root key directly.

### Migration status (complete)

| Flow | Path |
|------|------|
| Apex first-claim (`run_apex_claim`) | sponsored tempo tx |
| Tenant first-claim (`signer.rs::run_claim_name`) | sponsored tempo tx via iframe |
| `claim_and_maybe_set_main_sponsored` | tempo tx batch |
| `lh_transfer` | `run_sponsored_tempo_call` (sender_hash via iframe) |
| `submit_feedback` | `run_sponsored_tempo_call` |
| publish app (`setMetadata`) | `run_sponsored_tempo_call` |
| add/remove device signer | `add_/remove_signer_sponsored` |
| `register_main_sponsored` | sponsored tempo tx |

The shared mechanism is the iframe signer's `lh-sign-digest` message
(tenant computes the sender_hash, apex wallet signs it, embedded sponsor
signs `fee_payer`). Every user-facing write goes through
`events::run_sponsored_tempo_call`. The self-paid `sign_and_submit_call` /
standalone `register_main` paths remain in `registry.rs` for off-bundle /
native callers but aren't used by the browser UI.

## What's planned

SDK runtime (0.2.x–0.6.x), browser IDE (0.7.x), and platform layer
(subdomains + master wallet + registry + ERC-721 + ERC-6551 + iframe signer
+ visitor lockdown, through 0.10.0) shipped; Tempo native AA shipped
post-0.10.24. Next:

- **MPP / x402 payment hooks** — pre-tool-call hook requiring payment to
  the agent's TBA, or agent-pays-agent over Stripe MPP (preferred) /
  Coinbase x402. Fits the existing `Hook` trait; plumbing exists.
- **ERC-8004 reputation + validation facets** — cut into the diamond;
  agents accrue reputation; validators stake to re-execute claims.
- **TBA-driven actions in the bundle** — UX for "send this tx from your
  agent's TBA"; contract surface ready, mostly a UI piece.
- **Second backend** (Anthropic/OpenAI/local) — abstractions in place,
  validation overdue.
- **Tool-call activity in restored transcripts** — `TranscriptEntry` drops
  FunctionCall/FunctionResponse on replay today.
- **At-rest encryption** — wallet-derived sym key over OPFS contents.

## Filesystem trait

The 8 fs builtins (`list_directory`, `view_file`, `find_file`,
`search_directory`, `create_file`, `edit_file`, `delete_file`,
`rename_file`) call `crate::filesystem::Filesystem`, not `tokio::fs`.
Surface: `read`, `write_atomic`, `metadata`, `read_dir`, `walk`, `delete`,
`rename` (default = read+write+delete; NativeFilesystem overrides with
`tokio::fs::rename`). Two impls ship:

- **`NativeFilesystem`** (`feature = "native"`): `tokio::fs` + `walkdir` +
  `tempfile`; atomicity via tempfile + rename.
- **`OpfsFilesystem`** (wasm32): OPFS via `web-sys`; atomicity via
  `FileSystemWritableFileStream.close()` swap.

`GeminiConnectionStrategy::connect` honors a caller-supplied `Filesystem`
via `with_filesystem`, else auto-installs `NativeFilesystem` on native (or
`None` on wasm — caller supplies OPFS; the browser app does). Plug-in impls
implement the trait and hand a `SharedFilesystem = Arc<dyn Filesystem>`.

## Documentation SOP

Five surfaces; keep them in sync on every change.

| Surface | File | Audience | What it covers |
|---------|------|----------|----------------|
| **docs.rs** | `///` comments in source | SDK consumers | Public API: every `pub` item needs a one-liner |
| **README.md** | repo root | GitHub visitors, crates.io | Quick start, features, architecture, links |
| **CLAUDE.md** | repo root | Claude Code sessions | Full internal context: repo layout, gotchas, plans |
| **llms.txt** | `web/llms.txt` | External agents, LLMs | Agent capabilities, RPC format, on-chain registry |
| **CHANGELOG.md** | repo root | Users tracking releases | Per-version changes (Keep-a-Changelog) |

**When to update what:** new pub API → `///` one-liner (+ README if feature
surface changes); new file/module → CLAUDE.md repo tree; new agent
tool/capability → `llms.txt` tool list + `chat.rs::start_session` prompt;
new facet/contract → CLAUDE.md on-chain + `llms.txt` registry; browser UX →
CLAUDE.md browser section; release → CHANGELOG entry.

**Single source of truth:** code comments (docs.rs) for API behavior — don't
duplicate in README; CLAUDE.md for internal architecture; llms.txt for
agent-facing capabilities (concise, machine-readable, no marketing); the
`chat.rs::start_session` system prompt for what the agent knows about itself.

**Verification before any release:**
```sh
cargo doc --no-deps 2>&1 | grep "warning.*missing"  # undocumented pub items
curl -s https://localharness.xyz/llms.txt | head -5  # verify llms.txt deployed
```
