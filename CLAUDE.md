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
  [`0x6c31c0…a30c`](https://moderato.tempo.xyz/address/0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c)
  (full reset 2026-06-01 — brand-new diamond + token + 6551 infra;
  every prior address is abandoned)

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
│                          (feature = "wallet"; works on every target).
│                          Includes Tempo Tx submission helpers:
│                          `submit_tempo_self_paid` / `_sponsored`
│                          + `claim_and_maybe_set_main_sponsored`.
│                          Also credit/session/x402/device helpers:
│                          `redeem_sponsored`, `open_session_sponsored`,
│                          `session_expiry_of`, `session_price`,
│                          `credit_balance_of`, `deposit_credits_sponsored`,
│                          `x402_domain_separator`/`x402_digest`/`sign_x402`/
│                          `settle_x402_sponsored`/`x402_authorization_state`,
│                          `devices_of`/`is_device_linked`,
│                          `consolidate_into_main_sponsored`,
│                          `release_name_sponsored`/`release_name_calldata`,
│                          `erc20_balance_of`, `remove_signer_sponsored`
│                          (now also unlinks the DeviceRegistry index).
├── x402_hook.rs           app-injected x402 signer for `call_agent` —
│                          signs the EIP-712 "exact" authorization so an
│                          agent-to-agent call settles in $LH (feature
│                          = "wallet").
├── tempo_tx.rs            Tempo Transaction (tx type 0x76) encoder —
│                          native AA with fee_token + fee_payer fields.
│                          Sign self-paid or sponsored; submit via
│                          standard `eth_sendRawTransaction`. See
│                          `[[tempo-tx-findings]]` for wire details.
├── rustlite/              Rust-subset → wasm compiler (in-crate)
│   ├── mod.rs             compile(source) → wasm bytes top-level API
│   ├── token.rs           token types (keywords, operators, literals)
│   ├── lexer.rs           byte-level lexer with string escapes
│   ├── ast.rs             full AST (structs, enums, fns, match, etc.)
│   ├── parser.rs          recursive descent with precedence climbing
│   ├── typecheck.rs       scope-based type resolution + mutability
│   ├── codegen.rs         wasm binary emitter (sections, opcodes, LEB128)
│   └── loader.rs          wasm32-only cartridge instantiation via WebAssembly
├── app/                   browser-resident IDE — gated on
│   ├── mod.rs             `browser-app` feature + wasm32 target
│   ├── templates.rs       all maud HTML
│   ├── dom.rs             web-sys helpers (swap_inner, …)
│   ├── events.rs          delegated click/keydown/submit/input dispatch
│   ├── chat.rs            chat-turn streaming
│   ├── history.rs         OPFS-persisted conversation (with tool-call replay)
│   ├── opfs.rs            file browser + inline editor. Click-to-DISPLAY for
│   │                      .wasm (run), .rl (compile+run), .html (render);
│   │                      .rl/.html rows get an explicit [edit] button.
│   │                      run_cartridge auto-saves source to cartridge.rl
│   ├── display.rs         framebuffer surface — runs wasm cartridges
│   │                      into a <canvas> via host_display.present
│   │                      (Orbital-style compositor; see Redox vision).
│   │                      Also rasterizes HTML to the framebuffer
│   │                      (render_html: block-level text, no JS/CSS) and
│   │                      holds the 5x7 bitmap font (A-Z, a-z, 0-9, punct)
│   ├── key_store.rs       Gemini API key in OPFS
│   ├── owner.rs           legacy local-UUID owner marker
│   ├── tenant.rs          hostname classifier (apex / tenant / other)
│   ├── wallet_store.rs    master wallet persisted to apex OPFS
│   ├── signer.rs          postMessage signer service at apex/?signer=1
│   ├── agent_rpc.rs       inter-agent RPC endpoint (?rpc=1 URL mode)
│   ├── encryption.rs      AES-256-GCM at-rest encryption via WebCrypto
│   ├── system_prompt.rs   per-tenant custom system prompt (.lh_system_prompt.txt)
│   ├── tool_allowlist.rs  per-agent tool restriction (.lh_tool_allowlist.txt)
│   ├── sponsor.rs         embedded sponsor private key for fee_payer
│   │                      signing on user-facing Tempo txs (testnet
│   │                      only — see security notes inside)
│   └── verify.rs          subdomain-side iframe owner verification
└── backends/
    ├── gemini/
    │   ├── api.rs         GeminiClient + SSE decoder (CRLF + LF tolerant)
    │   ├── wire.rs        REST request/response types
    │   ├── loop.rs        run_turn — the inner agent loop
    │   ├── compaction.rs  history summarisation
│   │                  built-in tools including call_agent (inter-agent
│   │                  RPC), compile_rustlite (compile + run rustlite), and
│   │                  render_html (HTML → framebuffer snapshot)
    │   ├── tools/         built-in Tool impls (one per BuiltinTool variant;
    │   │                  run_cartridge + render_html drive the DISPLAY)
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
│   │   ├── LibTbaConfigStorage.sol       TBA config (slot v1)
│   │   ├── LibRedeemStorage.sol          redeem-code → $LH amount (slot v1)
│   │   ├── LibSessionStorage.sol         credit-session price/duration/expiry
│   │   ├── LibCreditMeterStorage.sol     per-request $LH meter balances +
│   │   │                                 meter key (slot v1)
│   │   ├── LibX402Storage.sol            x402 EIP-712 domain + nonce state
│   │   │                                 (slot v1)
│   │   └── LibDeviceRegistryStorage.sol  enumerable linked-device index
│   │                                     (slot v1)
│   ├── facets/
│   │   ├── DiamondCutFacet.sol           owner-only upgrade
│   │   ├── DiamondLoupeFacet.sol         introspection + supportsInterface
│   │   ├── OwnershipFacet.sol            EIP-173 owner()/transfer
│   │   ├── LocalharnessRegistryFacet.sol register / ownerOfName / ...
│   │   ├── ERC721Facet.sol               ERC-721 + Metadata surface
│   │   ├── TbaFacet.sol                  ERC-6551 token-bound accounts
│   │   ├── FeedbackFacet.sol             submitFeedback(string) → event
│   │   ├── MainIdentityFacet.sol         registerMain/clearMain/mainOf
│   │   ├── RedeemFacet.sol               addRedeemCodes / redeem(string) →
│   │   │                                 mints $LH via ISSUER_ROLE
│   │   ├── SessionFacet.sol              openSession / sessionExpiryOf /
│   │   │                                 setSessionPrice/Duration — credit
│   │   │                                 sessions
│   │   ├── CreditMeterFacet.sol          depositCredits / meter[meter-only] /
│   │   │                                 creditOf / setMeter — per-request
│   │   │                                 $LH metering
│   │   ├── X402Facet.sol                 settle / authorizationState /
│   │   │                                 x402DomainSeparator — x402 "exact"
│   │   │                                 settlement in $LH (agent-to-agent)
│   │   ├── DeviceRegistryFacet.sol       linkDevice / unlinkDevice /
│   │   │                                 devicesOf / isDeviceLinked —
│   │   │                                 enumerable linked-device index
│   │   └── ReleaseFacet.sol              releaseName(tokenId) — owner-only
│   │                                     burn + free a name (refuses MAIN)
│   ├── erc6551/                          vendored EIP-6551 reference
│   │   ├── IERC6551Registry.sol
│   │   ├── ERC6551Registry.sol
│   │   └── ERC6551Account.sol
│   ├── upgradeInitializers/
│   │   └── DiamondInit.sol               one-shot init for ERC-165 flags
│   └── LocalharnessRegistry.sol          legacy flat contract (archived)
├── script/
│   ├── DeployDiamond.s.sol               from-scratch diamond deploy
│   ├── AddErc721Facet.s.sol              cut ERC-721 surface (migration)
│   ├── AddErc721Fresh.s.sol              cut ERC-721 (fresh diamond)
│   ├── AddTbaFacet.s.sol                 cut 6551 + helper
│   ├── AddFeedbackFacet.s.sol            cut submitFeedback(string)
│   ├── AddMainIdentityFacet.s.sol        cut MAIN identity surface
│   ├── AddRedeemFacet.s.sol              cut RedeemFacet
│   ├── AddSessionFacet.s.sol             cut SessionFacet
│   ├── AddCreditMeterFacet.s.sol         cut CreditMeterFacet
│   ├── AddX402Facet.s.sol                cut X402Facet
│   ├── AddDeviceRegistryFacet.s.sol      cut DeviceRegistryFacet
│   ├── AddReleaseFacet.s.sol             cut ReleaseFacet
│   └── Deploy.s.sol                      legacy flat deploy (archived)
└── README.md                             architecture write-up

web/                       static site for Vercel
├── index.html             bootstrap shell (CSS + #root + init())
└── pkg/                   wasm-pack output (gitignored; built locally
                           and uploaded by `vercel deploy`):
                           localharness.js + localharness_bg.wasm

proxy/                     $LH credit proxy — a SEPARATE Vercel project
│                          ("proxy") deployed at
│                          https://proxy-tau-ten-15.vercel.app . The ONE
│                          accepted off-chain component; everything else is
│                          Tempo + browser. LIVE.
├── api/gemini.ts          Vercel Edge Function: transparent Gemini
│                          passthrough holding the platform GEMINI_API_KEY
│                          in env. Auth = an Ethereum personal-sign in the
│                          x-goog-api-key header (address:timestamp:
│                          signature); gates on an active SessionFacet
│                          session OR a CreditMeterFacet balance; per-request
│                          mode debits via the meter key (viem EIP-1559)
├── package.json           proxy deps + build
├── vercel.json            edge runtime config
├── README.md              proxy setup + env vars
└── .gitignore             node_modules + Vercel build output

scripts/
├── release.{ps1,sh}       atomic release tool (see RELEASING.md)
├── build-web.{ps1,sh}     wasm-pack build → web/pkg/
├── probe-gemini.ps1       isolate request-shape vs. response-parse bugs
└── harvest-feedback.{ps1,sh}  cast logs wrapper for FeedbackSubmitted events

examples/
└── tempo_tx_live.rs       end-to-end live harness against Moderato — runs
                           self-paid native / self-paid TIP-20 / sponsored
                           scenarios with the deployer key from .env.
                           Source of truth for verifying tempo_tx encoding.

design/
├── main-identity.md       MAIN identity + multi-device linking design
├── agent-writes-rust.md   rustlite compiler design: grammar EBNF, cartridge ABI
├── launch-1.0.md          grand plan for the 1.0.0 public launch:
│                          phases, workstreams, the 4 gating decisions,
│                          roadmap, definition-of-done, risks
└── paymaster.md           paymaster architecture (superseded by Tempo
                           native AA — see Update section at the bottom)

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

   **Two surfaces per subdomain (public face vs studio).** Every
   subdomain has a visitor-facing **public face** (a fullscreen
   cartridge) and an owner-only **studio** (the workshop chrome). Routing
   is role-based, keyed on `owner.is_some()` (this device's local
   ownership claim, refined later by verification):
   - **Owner** → lands in the **studio** by default. Never auto-hijacked
     into a fullscreen app. Previews the public face via `?view=public`
     (a `[view public]` link in the tenant header), which paints the
     fullscreen cartridge with a `[studio]` escape link (→ `?edit=1`).
   - **Visitor** → only ever sees the **public face**. No studio, no edit
     door.
   `paint_public_face` paints the resolved face for a tenant.
   `resolve_public_face(name)` reads the **on-chain choice** under
   `keccak256("localharness.public_face")` (`registry::public_face_of`) —
   one of `directory` / `app` / `html` — and gathers content (local
   working copy first so the owner previews unpublished edits, else the
   published copy). It returns a `PublicFace` enum:
   - **`Cartridge(wasm)`** — `app.rl` (local) or published wasm
     (`app_wasm_of`); `paint_cartridge_fullscreen` → `app_fullscreen` +
     `display::run_in_root_canvas`.
   - **`Html(src)`** — `index.html` (local) or published HTML
     (`public_html_of`, key `keccak256("localharness.public.html")`);
     `paint_html_fullscreen` → `app_fullscreen` +
     `display::render_html_in_root_canvas`.
   - **`Directory`** — `paint_public_landing` (`templates::public_landing`):
     a profile/directory landing — name, owner (MAIN name when it differs),
     TBA wallet, and a directory of the owner's other agents (siblings via
     `registry::list_owned_tokens`, self excluded).
   An UNSET choice infers "cartridge if one exists, else directory" so
   subdomains that published a cartridge before the picker shipped keep
   showing it. `owner_overlay` gates the `[studio]` link (set only when
   the owner is previewing). `Host::Other` (localhost/preview) uses
   `try_paint_app` — the local `app.rl` only, no on-chain resolution.

   **Picker (admin → agent → "public face").** The owner chooses the face
   from `templates::admin_app_section`: `[directory] [publish app]
   [publish html]` → `Action::SetPublicFace(choice)` →
   `events::run_set_public_face`. `directory` sets the choice only;
   `app`/`html` compile/read the local `app.rl`/`index.html` and publish it
   **plus** set the choice in ONE sponsored Tempo tx (two `setMetadata`
   calls). `refresh_public_face_status` reflects the current choice on
   admin open.

   **Second-device owner upgrade.** A seed-bearing owner hitting their
   own subdomain from a device WITHOUT the local `.lh_owner` marker is
   treated as a visitor (lands on the public face). `paint_tenant` then
   fires `redirect_to_studio_if_owner` in the background: if
   `verify::verify_owner` proves control via the apex signer, it
   navigates to `?edit=1` (the studio). Skipped when the device already
   claims ownership, so a deliberate `?view=public` preview never bounces. The agent makes a
   subdomain "become" an app by writing the same source it passes to
   `run_cartridge` to `app.rl` via `create_file` — but only on an
   explicit "make this my permanent app" request. (Earlier a MAIN was
   hard-blocked from fullscreen; that special-case was dropped once the
   owner always lands in the studio — the guarantee now comes from
   role-based routing, not a per-name exception.)

   **Cross-visitor publishing (on-chain).** Local `app.rl`/`index.html`
   are the owner-device working copies; *visitors* see the published
   bytes. Stored in the registry diamond under `metadata(tokenId, key)` —
   no new facet, the existing owner-gated `setMetadata(uint256,bytes32,
   bytes)` holds them. Keys: `keccak256("localharness.app.wasm")`
   (cartridge), `keccak256("localharness.public.html")` (HTML),
   `keccak256("localharness.public_face")` (the choice string). Generic
   `registry::{metadata_bytes_of, encode_set_metadata_bytes}` back the
   typed `{app_wasm_of, public_html_of, public_face_of}` +
   `encode_set_*`. The owner publishes via the **admin → agent → "public
   face"** picker (`events::run_set_public_face`), a sponsored Tempo tx
   (owner signs the sender_hash through the apex iframe; sponsor pays).

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
`0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` on Tempo Moderato
testnet (chain id 42431, RPC `https://rpc.moderato.tempo.xyz`).
(The system was fully reset 2026-06-01 — a brand-new diamond, `$LH`
token, and 6551 infra. Every prior address is abandoned and no longer
referenced by the bundle.) Facets are added/removed via `diamondCut`;
the wasm bundle's `registry::REGISTRY_ADDRESS` constant doesn't change.

**Per-facet addresses are deliberately not pinned here.** Facet
implementation addresses churn on every re-cut; the diamond address is
the only durable handle. Query the live facet set via the
DiamondLoupeFacet (`facets()` / `facetAddress(selector)`). The list
below names what is cut in, not where each facet's code lives.

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
- **Gemini key sync (per-MAIN, on-chain).** The sealed Gemini API key
  lives under the owner's **MAIN tokenId** (`mainOf(owner)`, falling back
  to the name's own id), NOT per-subdomain — so every subdomain an owner
  holds shares ONE key ("the subdomain IS the primary owner"). On a
  tenant paint, `events::try_auto_restore_gemini_key` fetches that blob
  and decrypts it via the apex iframe (`open_key_via_iframe`, seed-derived
  key) BEFORE the api-key modal would show, so a new subdomain on any
  seed-bearing device never re-prompts. Saving a key
  (`save_api_key_pressed`) best-effort `auto_sync_gemini_key`s it to the
  MAIN slot. Slot resolver: `events::gemini_key_slot_id`. NOTE: a phone
  linked by *device key only* (pairing, no seed) can't decrypt the
  seed-sealed blob, so it gets an **ECIES-wrapped-to-device** copy
  instead (built): on pairing the phone announces its compressed pubkey
  (`PairingAnnounced(bytes32,address,bytes,uint256)`), the desktop reads
  the MAIN's seed-sealed key, decrypts with the local seed, re-wraps it
  to the device pubkey via `encryption::ecies_seal` (ephemeral k256 ECDH
  → keccak → AES-GCM; `wallet::{pubkey_compressed,ephemeral_keypair,
  ecdh_shared_key}`), and posts the blob under
  `keccak256("localharness.gemini_key.dev."||device_addr)` on the MAIN
  tokenId (`registry::set_device_wrapped_key_sponsored`). The phone polls
  `wrapped_device_key_of`, decrypts with its device key
  (`ecies_open`), and saves locally — never touching the seed. Needs the
  k256 `ecdh` feature. v2 pairing facet adds the
  `announcePairing(bytes32,bytes)` selector (old `(bytes32)` left as a
  harmless orphan).
- **PairingFacet** — `announcePairing(bytes32 codeHash)` emits
  `PairingAnnounced(bytes32 indexed codeHash, address indexed device,
  uint256 timestamp)`. Event-only, no storage (like Feedback). Powers
  zero-copy device linking: the desktop (master wallet) shows a one-time
  code; the phone opens `<name>.localharness.xyz/?pair=CODE`, generates a
  fresh device key, and calls `announcePairing(keccak256(code))` as a
  SPONSORED tempo tx (so its device key is `msg.sender`, pays nothing).
  The desktop filters logs by that codeHash topic, learns the device
  address from the indexed `device` topic, and enrolls it via
  `addSigner` on the TBA — no 0x ever copied between machines. Two-way
  challenge: the Tempo sender sig proves device-key control; knowing the
  code proves co-presence. Cut via `script/AddPairingFacet.s.sol` (v2:
  `announcePairing(bytes32,bytes)` selector); device key stored
  per-tenant-origin in `.lh_device_key` (raw hex, NOT the master seed).
- **FeedbackFacet** — `submitFeedback(string text)` emits
  `FeedbackSubmitted(address sender, uint256 timestamp, string text)`.
  No storage, just events; harvest off-chain via `cast logs` (see
  `scripts/harvest-feedback.{sh,ps1}`). Anyone can submit; gas IS the
  spam filter. 2048-byte upper bound on text.
- **MainIdentityFacet** — `registerMain(uint256) / clearMain() /
  mainOf(address) / mainNameOf(address) / isMain(uint256)`. Records
  which of a holder's subdomain NFTs is their primary identity. No
  fee yet (sybil-resistance layer is later). Auto-set by the bundle
  on first-claim. See `design/main-identity.md`.
- **LocalharnessRegistryFacet (cost-gated)** — `register(name)` can
  pull `registrationCost()` LH from the caller into the diamond via
  `transferFrom` (caller must approve the diamond first; bundle
  batches `approve` + `register` in one Tempo tx). Owner-only
  `setRegistrationCost(uint256)` knob; zero disables. **Currently set
  to 0 (registration is FREE / ungated)** — set to 0 on 2026-05-28
  because the daily-claim onboarding was broken and new users had no
  LH to pay the old 50 LH cost; the bundle's `cost == 0` branch then
  registers with no approve/claim. The credit token + `CreditsFacet`
  stay on-chain for the future (streaming/subscription model), but the
  daily-claim UI was removed from the bundle. Re-cut on 2026-05-26 via
  `script/SwapRegistryFacetAddCost.s.sol`; cost-gate storage at
  `keccak256("localharness.registration_cost.storage.v1")`.
- **CreditsFacet** — distribution layer for the `LocalharnessCredits`
  TIP-20-shaped credit token. Surface: `claimDaily() / canClaim(addr)
  / dailyAllowance() / lastClaimDay(addr) / creditsToken()`. Owner-
  only setters: `setCreditsToken(addr) / setDailyAllowance(amount)`.
  Diamond holds `ISSUER_ROLE` on the token, so `claimDaily` is the
  only path to fresh supply. Day boundary = `block.timestamp / 86400`
  (UTC-aligned, no cron). See `contracts/src/LocalharnessCredits.sol`
  for the token's TIP-20 surface (currency = "credits", not USD —
  explicitly NOT fee-token-eligible).
- **RedeemFacet** — cut, live. Bootstraps $LH into a fresh wallet via
  one-time codes. Owner
  loads `keccak256(code) -> $LH amount` via `addRedeemCodes(bytes32[],
  uint256)`; a holder calls `redeem(string code)`, which mints the
  mapped amount of $LH to the caller through the diamond's `ISSUER_ROLE`
  and burns the code; owner-only `disableRedeemCodes`. Storage at
  `keccak256("localharness.redeem.storage.v1")` (`LibRedeemStorage`).
  Cut via `script/AddRedeemFacet.s.sol`.
- **SessionFacet** — cut, live. Coarse, time-boxed $LH credit sessions
  for the proxy.
  `openSession()` pulls `sessionPrice()` $LH from the caller into the
  diamond via `transferFrom` (caller approves first) and sets
  `expiry = block.timestamp + sessionDuration()`. View
  `sessionExpiryOf(address)` is what the credit proxy reads to gate
  access. Owner-tunable `setSessionPrice` / `setSessionDuration`.
  **Currently `sessionDuration = 3600`, `sessionPrice = 0`** (free in
  beta). Storage at `keccak256("localharness.session.storage.v1")`
  (`LibSessionStorage`). Cut via `script/AddSessionFacet.s.sol`.
- **CreditMeterFacet** — cut, live. Per-request $LH metering, the
  fine-grained alternative to coarse sessions. `depositCredits(uint256)`
  pulls $LH into the diamond and credits the caller's balance;
  `creditOf(address)` reads it; `meter(address,uint256)` debits a
  balance and is callable ONLY by the configured meter key; owner-only
  `setMeter(address)`. The proxy's meter key EOA is `setMeter`'d +
  funded. Storage at `keccak256("localharness.credit_meter.storage.v1")`
  (`LibCreditMeterStorage`). Cut via `script/AddCreditMeterFacet.s.sol`.
- **X402Facet** — cut, live. True x402 (EIP-712 "exact" scheme)
  payment SETTLEMENT in $LH
  for agent-to-agent flows. `settle(...)` verifies an EIP-712
  authorization (EOA `ecrecover` + EIP-1271 `isValidSignature`,
  one-shot nonce) and moves $LH from payer to payee; `authorizationState`
  reports nonce usage; `x402DomainSeparator()` exposes the domain (read
  it live — the separator binds chainId + the diamond address, so the
  reset changed it). Storage at `keccak256("localharness.x402.storage.v1")`
  (`LibX402Storage`). Cut via `script/AddX402Facet.s.sol`.
- **DeviceRegistryFacet** — cut, live. Enumerable linked-device index,
  read in ONE call:
  `linkDevice / unlinkDevice / devicesOf(address) / isDeviceLinked`.
  Replaces scraping `SignerAdded` logs, which Tempo's RPC caps at 100k
  blocks. Storage at `keccak256("localharness.device_registry.storage.v1")`
  (`LibDeviceRegistryStorage`). Cut via
  `script/AddDeviceRegistryFacet.s.sol`.
- **ReleaseFacet** — cut, live. `releaseName(uint256 tokenId)` —
  owner-only burn that frees a
  name for re-registration; **refuses the caller's MAIN**. Cut via
  `script/AddReleaseFacet.s.sol`.

ERC-6551 reference contracts (separate addresses, configured via
`TbaFacet::setTbaConfig`; redeployed fresh in the 2026-06-01 reset):
- Registry: `0x2795810e5dfC8bC92Ef7fc9557F6c0699E11c3B3`
- Account impl: `0x86be7c44d1940F4dE53A738153A12FaAEa68B5a7`
  (`MultiSignerAccount` — CALL-only; an additional-signer set on top of
  the NFT holder + EIP-1271 `isValidSignature`, so a MAIN can be
  controlled by multiple device EOAs without sharing the seed. Signer
  management is owner-only and additional signers are bound to the
  enrolling holder (`_signerEnroller[signer] == owner()`), so an NFT
  transfer silently revokes the prior holder's device signers;
  `isValidSignature` rejects high-s (EIP-2). The bundle reads TBA
  addresses via the diamond's `tokenBoundAccount`, so a registry/impl
  swap needs no bundle change — but TBAs minted under prior infra
  resolve to different counterfactual addresses than current mints.)

Adding a new facet: write `LibXyzStorage` at a fresh
`keccak256("localharness.xyz.storage.v1")` slot, write the facet,
forge build, write a one-off cut script following `AddTbaFacet.s.sol`
as a template, deploy. See `contracts/README.md` for the full
walkthrough.

## Credit proxy + $LH sessions / metering (LIVE)

**Status: deployed and live.** The proxy runs at
`https://proxy-tau-ten-15.vercel.app` and the RedeemFacet,
SessionFacet, and CreditMeterFacet are cut into the diamond (addresses
in the on-chain section above).

Platform `$LH` credits are the **primary** usage path; **BYOK**
(bring-your-own Gemini key) is the second option. The proxy is the ONE
accepted off-chain component and the **only server in the system**;
everything else stays Tempo + the user's browser.

Pieces:

- **`proxy/`** — a separate Vercel project ("proxy", TypeScript Edge
  Function `proxy/api/gemini.ts`) at `https://proxy-tau-ten-15.vercel.app`.
  A transparent Gemini passthrough: same path/request shape as Gemini,
  the platform `GEMINI_API_KEY` held in env only. Auth is an Ethereum
  personal-sign carried in the `x-goog-api-key` header as
  `address:timestamp:signature`. The proxy verifies the signature, then
  gates on EITHER an active SessionFacet session (`sessionExpiryOf`) OR a
  CreditMeterFacet balance (`creditOf`). In per-request mode it debits
  via the meter key (viem, EIP-1559) before streaming Gemini back.
- **RedeemFacet** (cut, live) — bootstraps $LH via one-time
  `redeem(code)` codes the owner pre-loads with `addRedeemCodes`. How a
  fresh wallet gets its first credits with zero off-chain payment rails.
- **SessionFacet** (cut, live) — `openSession()` spends `sessionPrice()`
  $LH for a coarse time-boxed window (`expiry = now + sessionDuration()`);
  the proxy gates on `sessionExpiryOf`. Currently free in beta
  (`sessionPrice = 0`, `sessionDuration = 3600`).
- **CreditMeterFacet** (cut, live) — fine-grained per-request metering:
  `depositCredits` tops up a $LH balance, the proxy's meter key debits
  it per request via `meter(...)`, `creditOf` reads the balance.
- **`src/registry.rs`** — sponsored client helpers the bundle uses to
  drive the above: `redeem_sponsored`, `open_session_sponsored`,
  `session_expiry_of`, `session_price`, `deposit_credits_sponsored`,
  `credit_balance_of`.

End-to-end flow: redeem a code → `$LH` in wallet → either `openSession()`
(coarse window) or `depositCredits()` (per-request meter) → bundle calls
the proxy with a signed `x-goog-api-key` header → proxy verifies the
signature, checks session OR meter balance, and streams Gemini. BYOK
skips the proxy entirely and talks to Gemini directly with the user's
own key.

## x402 agent-to-agent settlement (LIVE)

The **X402Facet** (cut, live) settles agent-to-agent payments in `$LH`
using the x402 "exact" scheme over EIP-712. A paying agent signs an
authorization; the payee (or anyone) calls `settle(...)`, which verifies
the signature (EOA `ecrecover` + EIP-1271 for contract/TBA signers),
consumes a one-shot nonce, and moves `$LH` from payer to payee.
`authorizationState` reports nonce usage; `x402DomainSeparator()` exposes
the domain (read it live — the separator binds chainId + the diamond
address, so the 2026-06-01 reset changed it).

In the bundle, **`src/x402_hook.rs`** is an app-injected signer wired
into `call_agent`: when one agent calls another, the hook signs the
EIP-712 authorization so the inter-agent call can settle in `$LH`.
Client helpers in `registry.rs`: `x402_domain_separator`, `x402_digest`,
`sign_x402`, `settle_x402_sponsored`, `x402_authorization_state`.

## Device index + name release (LIVE)

- **DeviceRegistryFacet** (cut, live) gives the bundle an enumerable
  linked-device list read in ONE call (`devicesOf` / `isDeviceLinked`),
  replacing `SignerAdded` log scraping that Tempo's RPC caps at 100k
  blocks. Linking/unlinking is `linkDevice` / `unlinkDevice`;
  `registry::remove_signer_sponsored` now also unlinks the index.
  Client reads: `registry::devices_of`, `registry::is_device_linked`.
- **ReleaseFacet** (cut, live) frees a name: `releaseName(tokenId)` is
  an owner-only burn that returns the name to the available pool. It
  **refuses the caller's MAIN** (you can't release your primary
  identity). Client helpers: `registry::release_name_sponsored`,
  `registry::release_name_calldata`. `registry::consolidate_into_main_sponsored`
  releases an owner's non-MAIN holdings in one sponsored batch.

## New agent tools + destructive-action convention

Two subdomain-management tools were added to the agent surface (declared
in `chat.rs::start_session`):

- **`list_subdomains()`** — read-only; enumerates the owner's holdings.
- **`release_subdomain(name, confirmation)`** — DESTRUCTIVE. Burns the
  name (calls ReleaseFacet `releaseName`). It requires
  `confirmation == name` (a typed confirmation the user must type in
  chat), **refuses the caller's MAIN**, and is **not granted to
  subagents**.

The system prompt now carries a hard convention: **destructive /
irreversible actions require a typed confirmation that is never
auto-filled** — the agent must ask the user to type the exact value
(e.g. the subdomain name) before proceeding. Mirror this for any future
destructive tool.

## Tempo Transactions + sponsorship (post-0.10.24)

The user-facing claim flow uses Tempo's **native** account-abstraction
tx type (`0x76`) so users hold ZERO of anything — no native gas, no
TIP-20 stablecoin, nothing. The bundle's `src/app/sponsor.rs`
signs as `fee_payer` and pays fees in AlphaUSD on every user tx.

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
Fee-payer hash: `keccak256(0x78 || rlp([1..10, fee_token,
sender_address, aa_authorization_list, key_authorization?]))`. The
spec page is missing `aa_authorization_list` at position 13 of the
fee_payer hash — discovered by diffing against `wevm/ox`'s
`TxEnvelopeTempo`. Captured in memory so we don't relearn.

### $LH is TIP-20-shaped credit, NOT fee-token-eligible

Tempo's `fee_token` validation requires TIP-20 compliance AND
`currency() == "USD"`. Our `LocalharnessCredits` at
`0x90B84c7234Aae89BadA7f69160B9901B9bc37B17` (fresh in the 2026-06-01
reset) implements the TIP-20 surface — memo transfers, supply cap,
roles — but returns `currency() == "credits"`, so the chain explicitly
rejects it as a fee_token. That's intentional: $LH is in-system
credits, not gas. **AlphaUSD**
(`0x20c0000000000000000000000000000000000001`) remains the sponsor's
fee_token. $LH supply is controlled — the diamond holds `ISSUER_ROLE`,
and the mint paths are `CreditsFacet.claimDaily()` and
`RedeemFacet.redeem(code)`. Tokens from any pre-reset deploy are
orphaned; balances do not migrate.

### Sponsor key

Lives in `src/app/sponsor.rs` as a const. Same address as the
deployer for now (testnet acceptable). **Rotate before mainnet** —
either to a dedicated low-budget sponsor wallet (small extraction
blast radius) or to a different key-management scheme entirely
(WebAuthn passkey per user, Stripe-backed top-up, etc.). Tempo
access keys CANNOT sign as `fee_payer` — confirmed by reading
their open-source SDK, see `[[access-key-fee-payer-finding]]`. The
fee_payer signature must come from the root key directly.

### Migration status

| Flow | Path | State |
|------|------|-------|
| Apex first-claim (`run_apex_claim`) | sponsored tempo tx | ✅ |
| Tenant first-claim (`signer.rs::run_claim_name`) | sponsored tempo tx via iframe | ✅ |
| `claim_and_maybe_set_main_sponsored` | tempo tx batch | ✅ |
| `lh_transfer` | `run_sponsored_tempo_call` (sender_hash via iframe) | ✅ |
| `submit_feedback` | `run_sponsored_tempo_call` | ✅ |
| publish app (`setMetadata`) | `run_sponsored_tempo_call` | ✅ |
| add/remove device signer | `add_/remove_signer_sponsored` | ✅ |
| `register_main_sponsored` | sponsored tempo tx | ✅ |

The migration is complete: the iframe signer's `lh-sign-digest`
message (the tenant computes the sender_hash, the apex wallet signs it,
the embedded sponsor signs `fee_payer`) is the shared mechanism. Every
user-facing write goes through `events::run_sponsored_tempo_call`, so
users hold zero of anything. The self-paid `sign_and_submit_call` /
standalone `register_main` paths remain in `registry.rs` for off-bundle
/ native callers but aren't used by the browser UI.

## What's planned

The SDK runtime (0.2.x–0.6.x) and the in-tree browser IDE (0.7.x)
shipped. The platform layer (subdomains + master wallet + on-chain
registry + ERC-721 NFTs + ERC-6551 token-bound wallets + iframe
signer + visitor lockdown) shipped through 0.10.0. The Tempo native
AA migration shipped post-0.10.24. What's next:

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

## Documentation SOP

Five documentation surfaces; keep them in sync on every change.

| Surface | File | Audience | What it covers |
|---------|------|----------|----------------|
| **docs.rs** | `///` comments in source | SDK consumers | Public API: every `pub` item needs a one-liner |
| **README.md** | repo root | GitHub visitors, crates.io | Quick start, features, architecture, links |
| **CLAUDE.md** | repo root | Claude Code sessions | Full internal context: repo layout, gotchas, plans |
| **llms.txt** | `web/llms.txt` | External agents, LLMs | Agent capabilities, RPC format, on-chain registry |
| **CHANGELOG.md** | repo root | Users tracking releases | Per-version changes (Keep-a-Changelog) |

### When to update what

- **New pub API item** → add `///` doc comment (one-liner) + update
  README if it changes the feature surface.
- **New file or module** → update CLAUDE.md repo layout tree.
- **New agent capability / tool** → update `llms.txt` tool list +
  `chat.rs::start_session` system prompt.
- **New on-chain facet or contract** → update CLAUDE.md on-chain
  section + `llms.txt` registry section.
- **Browser app UX change** → update CLAUDE.md browser app section.
- **Release** → CHANGELOG.md entry (the release script stamps the
  date). README version badge auto-updates from crates.io.

### Single source of truth rules

- **Code comments** are truth for API behaviour → docs.rs renders
  them. Don't duplicate API docs in README.
- **CLAUDE.md** is truth for internal architecture → don't duplicate
  in README or llms.txt.
- **llms.txt** is truth for agent-facing capabilities → keep it
  concise, machine-readable, no marketing.
- **System prompt** in `chat.rs::start_session` is truth for what
  the agent knows about itself → update when tools change.

### Verification

Before any release:
```sh
cargo doc --no-deps 2>&1 | grep "warning.*missing"  # catch undocumented pub items
curl -s https://localharness.xyz/llms.txt | head -5  # verify llms.txt deployed
```
