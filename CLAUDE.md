# CLAUDE.md

Project context for Claude Code sessions. Read this first.

## What this is

`localharness` is a Rust-native, **model-agnostic** agent SDK (Gemini is the one
shipping backend today, behind a `Connection`/`ConnectionStrategy` seam; Anthropic
next) **and** a self-sovereign browser-resident agent platform built on it. ONE
crate;
`cargo add` gives an agent loop with streaming text, tool calling, hooks,
policies, triggers, MCP, and context compaction. Build with `browser-app` on
wasm32 and you also get the live IDE at `<name>.localharness.xyz`.

- [crates.io/crates/localharness](https://crates.io/crates/localharness) (current: **0.25.x**)
- [github.com/compusophy/localharness](https://github.com/compusophy/localharness)
- Native: stable Rust 1.85+, tokio-driven. wasm32: same crate, browser.
- Live: `localharness.xyz` (marketing apex) + wildcard `*.localharness.xyz`
  (per-user agents).
- On-chain registry: EIP-2535 Diamond on Tempo Moderato testnet at
  `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c`. **Full reset 2026-06-01** —
  brand-new diamond + token + 6551 infra; every prior address abandoned.

## Repo layout

```
src/                  library crate
├── lib.rs            re-exports + module roots
├── agent.rs          Agent facade (Layer 1)
├── conversation.rs   Conversation + ChatResponse (Layer 2)
├── connections/      Connection / ConnectionStrategy traits (Layer 3)
├── content.rs        Content, Media, Part (user message types)
├── tools.rs          Tool trait + ToolRunner + ClosureTool
├── hooks.rs          6 hook traits + HookRunner
├── policy.rs         Predicate / Policy / Decision + workspace_only
├── triggers.rs       Trigger trait + TriggerRunner + every()
├── runtime.rs        cfg-gated spawn helper + MaybeSendSync marker
├── filesystem/       Filesystem trait + Native + OPFS impls
├── types.rs          wire-adjacent enums (BuiltinTool, Step, etc.)
├── error.rs          Error + Result
├── wallet.rs         secp256k1 + BIP-39 + RLP (feature "wallet"; all targets)
├── registry.rs       JSON-RPC client for the Diamond + Tempo tx submission +
│                     credit/session/x402/device helpers (feature "wallet")
├── x402_hook.rs      app-injected x402 signer for call_agent (feature "wallet")
├── tempo_tx.rs       Tempo Transaction (tx 0x76) encoder; see Tempo section
├── rustlite/         Rust-subset → wasm compiler (in-crate): mod.rs compile()
│                     top-level; token/lexer; ast/parser; typecheck; codegen
│                     (wasm emitter); loader (wasm32-only cartridge instantiation)
├── app/              browser-resident IDE (browser-app + wasm32)
│   ├── mod.rs        mount-time routing (see browser app section)
│   ├── templates.rs  all maud HTML
│   ├── dom.rs        web-sys helpers (swap_inner, …)
│   ├── events.rs     delegated click/keydown/submit/input dispatch
│   ├── chat.rs       chat-turn streaming + system prompt
│   ├── history.rs    OPFS-persisted conversation (tool-call replay)
│   ├── opfs.rs       file browser + inline editor; click-to-DISPLAY .wasm/.rl/.html
│   ├── display.rs    framebuffer: runs wasm cartridges (host_display draw API +
│   │                 host_net WebSocket API) + rasterizes HTML; 5x7 font
│   ├── key_store.rs  Gemini API key in OPFS
│   ├── owner.rs      self-correcting on-chain-derived owner hint (.lh_owner)
│   ├── tenant.rs     hostname classifier (apex / tenant / other)
│   ├── wallet_store.rs  master wallet persisted to apex OPFS
│   ├── signer.rs     postMessage signer service at apex/?signer=1
│   ├── seed_pull.rs  local-seed-per-origin: copy the seed into a subdomain's
│   │                 OWN OPFS via a top-level apex round-trip (mobile fix —
│   │                 the signer iframe is partitioned-dead on mobile)
│   ├── agent_rpc.rs  inter-agent RPC endpoint (?rpc=1 URL mode)
│   ├── encryption.rs AES-256-GCM at-rest + ECIES via WebCrypto
│   ├── shared_fs.rs  cross-subdomain encrypted apex store (scaffold); webrtc.rs
│   │                 (RtcPeerConnection P2P over STUN) + sharedfs_sync.rs
│   │                 (union-reconcile) + teams_sync.rs (Layer-5 connect-and-sync
│   │                 orchestration; SDP ECIES-sealed to the peer's ephemeral pubkey)
│   │                 = the agent-team P2P collaboration layer, signaling via the
│   │                 on-chain SignalingFacet (compile/forge-verified; "sync my
│   │                 devices" UI shipped; 2-device E2E pending)
│   ├── system_prompt.rs  per-tenant custom prompt (.lh_system_prompt.txt)
│   ├── self_docs.rs  agent self-knowledge: embedded runtime summary (injected
│   │                 into the system prompt) + read_self_docs tool (fetches
│   │                 live llms.txt, falls back to the summary)
│   ├── tool_allowlist.rs per-agent tool restriction (.lh_tool_allowlist.txt)
│   ├── sponsor.rs    embedded sponsor private key for fee_payer (testnet only)
│   └── verify.rs     subdomain-side owner verification + the iframe signer
│                     client (sign challenge / tempo-tx / seal+open key) —
│                     each LOCAL-FIRST: runs on `APP.wallet` when the seed is
│                     local, else falls back to the apex iframe
├── bin/
│   └── localharness.rs  agent-onboarding CLI (feature wallet+native):
│                     `create <name>` (sponsored claim, persists key) /
│                     `compile <src.rl> [out.wasm]` (local compile-check, no
│                     write; rejects oversize + no-`frame`/`render`-entry) /
│                     `publish <name> <src.rl>` (compile cartridge + set it as the
│                     subdomain's on-chain public face) / `persona <name> <text>`
│                     (publish on-chain system prompt) / `call [--as me] [--fresh]
│                     <name> <msg>` (HEADLESS turn via the credit proxy, signed by
│                     the caller key, runs under the target's on-chain persona —
│                     NOT the browser ?rpc=1 postMessage path; conversation
│                     persists per caller/target under .localharness/history) /
│                     `list` (owned subdomains) / `redeem <code>` (mint $LH to your
│                     wallet — funding) / `mcp-call [--pay] <target> <msg>` (x402
│                     client for the hosted /mcp endpoint; auto-approve + sign +
│                     settle) / `threads`+`forget` (manage saved conversations) /
│                     `whoami [--json]` (profile) / `version`.
│                     Harness-agnostic, server-free entry — what web/skill.md
│                     tells external agents to run. Smoke: scripts/smoke-cli.sh.
└── backends/
    ├── gemini/       api.rs (GeminiClient + SSE decoder, CRLF+LF tolerant);
    │                 wire.rs (REST types); loop.rs (run_turn inner loop);
    │                 compaction.rs; tools/ (one Tool impl per BuiltinTool, incl.
    │                 call_agent, compile_rustlite, render_html, run_cartridge→DISPLAY);
    │                 mod.rs (GeminiConnectionStrategy + GeminiConnection)
    ├── anthropic/    Claude Messages API backend (feature "anthropic"): mod.rs
    │                 (Strategy+Connection), api.rs, wire.rs, loop.rs, compaction.rs
    ├── mcp/          stdio MCP client (native-only)
    └── local/        in-browser Gemma 3 270M via Burn/wgpu (feature "local"):
                      gemma.rs (model), weights.rs (safetensors loader),
                      tokenizer.rs, generate.rs (async greedy decode),
                      tool_parse.rs (tool_code-fence parser), connection.rs
                      (Connection seam + bounded tool loop), mod.rs

contracts/   Foundry project for the on-chain registry
├── src/      Diamond.sol (EIP-2535 proxy) + interfaces/; libraries/ (LibDiamond +
│             one LibXyzStorage per facet); facets/ (DiamondCut, DiamondLoupe,
│             Ownership, LocalharnessRegistry, ERC721, Tba, Feedback, MainIdentity,
│             Redeem, Session, CreditMeter, X402, DeviceRegistry, Release, Pairing;
│             DRAFTS not yet cut: OwnedTokens (tokensOfOwner enumerable index),
│             Signaling (on-chain WebRTC signaling mailbox + topic presence),
│             Team (agent teams by mutual invite+accept));
│             erc6551/ (vendored ref); upgradeInitializers/DiamondInit.sol;
│             LocalharnessRegistry.sol (legacy flat, archived)
├── script/   DeployDiamond.s.sol + one Add<Facet>.s.sol cut script per facet
└── README.md architecture write-up

web/          static site for Vercel: index.html (bootstrap shell) + pkg/
              (wasm-pack output, gitignored, built locally, uploaded by deploy);
              llms.txt (full agent spec, leads with the quickstart) + skill.md
              (the paste-to-your-agent onboarding front door; subset of llms.txt)
proxy/        $LH credit proxy — SEPARATE Vercel project ("proxy") at
              https://proxy-tau-ten-15.vercel.app. The ONE accepted off-chain
              component. LIVE. api/gemini.ts = Vercel Edge Gemini passthrough;
              api/mcp.ts = networked MCP-over-HTTP endpoint (`/mcp`, ask_agent)
              gated by TRUE x402 per-call settlement (EIP-712 verify vs live
              x402DomainSeparator + X402Facet.settle; payee = target agent's TBA)
scripts/      release.{ps1,sh}; build-web.{ps1,sh}; probe-gemini.ps1;
              harvest-feedback.{ps1,sh}; clear-feedback.sh (owner-only feedback GC);
              issue-to-pr.sh (verify-gated GitHub issue→PR harness, colony rung-2);
              test-fleet/ (12 QA personas + run-fleet.sh + feedback-to-issues.mjs)
examples/tempo_tx_live.rs  live harness vs Moderato; source of truth for tempo_tx
design/       main-identity.md; agent-writes-rust.md; launch-1.0.md (1.0 spec —
              1.0=mainnet, betas=testnet); beta-plan.md; paymaster.md;
              invites.md (user-created escrow invite codes — DESIGN, not built);
              agent-scheduling.md (on-chain ScheduleFacet + Vercel-Cron worker so
              agents run recursive jobs without a tab — DESIGN, not built)
RELEASING.md / CHANGELOG.md / vercel.json / .vercelignore
```

Historical design docs (`DESIGN.md`, `DESIGN_M5_PLUS.md`, `UPSTREAM.md`) dropped
at 0.10.1 — every layer shipped. Preserved under git tags `v0.1.0`–`v0.10.0`.

## Build / test / run

```sh
cargo build                                                        # native
cargo test                                                         # full suite
cargo check --no-default-features --target wasm32-unknown-unknown  # wasm guardrail
./scripts/build-web.sh                                             # rebuild wasm bundle
vercel deploy --prod --yes                                         # deploy web/
```

## Cargo features

- `native` (default): tokio (multi-thread/process/fs/io-util) + walkdir +
  tempfile. Required for `run_command`, MCP stdio bridge, and the default
  `NativeFilesystem` (the 8 fs builtins: list_directory, view_file, find_file,
  search_directory, create_file, edit_file, delete_file, rename_file).
- `wallet` (off): exposes `pub mod wallet` (secp256k1+BIP-39+RLP) + `pub mod
  registry` (Diamond JSON-RPC). Pulls k256+sha3+rand_core+bip39. All targets —
  `sleep_ms` cfg-gated to tokio (native) / setTimeout (wasm).
- `browser-app` (off): compiles `src/app/` as a wasm cdylib (the browser IDE).
  Pulls maud, pulldown-cmark, +wallet, +anthropic transitively. No native effect.
  Built via `wasm-pack build --no-default-features --features browser-app`.
- `anthropic` (off): compiles the second LLM backend `src/backends/anthropic/`
  (Claude Messages API as a `ConnectionStrategy`). PURELY ADDITIVE — pulls no new
  deps, leaves the default build + Gemini backend untouched. BYOK
  (`Agent::start_anthropic`) or, via the multi-provider credit proxy, platform
  `$LH` credits. Pulled in transitively by `browser-app` so the in-tab model
  selector can build the Claude path.
- `local` (off): compiles the in-browser local-model backend `src/backends/local/`
  — Gemma 3 270M via Burn's `wgpu`/WebGPU backend (no proxy, no `$LH`, no key).
  HEAVY (pulls burn 0.21 + burn-store + tokenizers; ~570MB opt-in weights to OPFS).
  NATIVE-VALIDATED — loads the real `unsloth/gemma-3-270m` checkpoint and emits
  coherent text. Additive and OFF by default — NOT pulled by `browser-app`; build
  the in-tab path explicitly with `--features browser-app,local`. Gotchas: burn
  drags in getrandom 0.4 → needs `.cargo/config.toml` `getrandom_backend="wasm_js"`
  + a renamed `getrandom_v04` dep; burn-store is a DIRECT dep (NOT burn's `store`
  feature) so `memmap2` (wasm-broken) stays out of the graph; generate's GPU
  read-back MUST be `into_data_async().await` (sync `into_data` panics on wasm).
- wasm targets auto-drop walkdir/tempfile, add wasm-bindgen-futures, uuid/js,
  getrandom/js via target-cfg.

SDK-only wasm callers: `default-features = false`, skip `browser-app`.
Off-bundle registry consumers: `default-features = false, features = ["wallet"]`.

## The wasm story (M2.5)

The crate compiles to `wasm32-unknown-unknown`:

- `runtime.rs::spawn` cfg-gates `tokio::spawn` (native) vs `spawn_local` (wasm).
- `runtime.rs::MaybeSendSync` is `Send + Sync` on native, empty on wasm. Every
  trait that required `: Send + Sync` now requires `: MaybeSendSync`.
- Every `#[async_trait]` is `cfg_attr`'d to `?Send` on wasm (browser-fetch
  futures aren't `Send`).
- `Connection::subscribe_steps` → `StepStream` alias = BoxStream (native) /
  LocalBoxStream (wasm). `JoinHandle` storage + abort cfg-gated; wasm
  fire-and-forgets via `spawn_local`.
- Only `run_command` + the MCP stdio bridge are gated behind `feature =
  "native"`. The 8 fs builtins are NOT native-gated — they register whenever a
  `Filesystem` is supplied (`BuiltinDeps.fs`), so they run on wasm32 over OPFS
  (the browser app supplies `OpfsFilesystem`) exactly as on native. Guarded by
  `fs_builtins_gate_on_filesystem_not_native` (backends/gemini/tools/mod.rs).
  The portable client-free tools (`ask_question`, `finish`, `start_subagent`,
  `generate_image`) work on both with no filesystem.

Adding new traits or `tokio::spawn` calls? Mirror these patterns or wasm breaks
silently (gated modules don't trip a default `cargo check`).

## Common gotchas

- **The signer iframe is DEAD on mobile (cross-origin storage partitioning).**
  Every seed-derived op on a subdomain (owner verify, key seal/open, tempo-tx
  sign) historically embedded `apex/?signer=1` in a hidden iframe and read the
  seed from apex OPFS. Mobile browsers partition cross-origin iframe storage →
  the embedded apex sees an EMPTY OPFS → every op fails (apex itself works,
  being top-level). Fix: `seed_pull.rs` copies the seed into the subdomain's own
  OPFS via a top-level apex round-trip, and `verify.rs` runs every op LOCAL-FIRST
  off `APP.wallet`. Don't reintroduce an iframe-only path for a seed op.
- **On-chain writes that store data are gas-HUNGRY — `cast estimate`, never
  guess a limit.** Live: `submitFeedback` is ~1.3M gas for a short note and
  ~17M near the 2048-byte cap (the facet stores the full string in cold
  SSTOREs). A flat 800k cap silently out-of-gassed EVERY feedback (local mirror
  saved, chain reverted → `feedbackCount` stuck at 0). Sponsored gas is now
  length-scaled. Same lesson as redeem (600k OOG). Block limit is 500M, so
  big writes fit — the bug is always an under-set client cap, not the chain.
  `setMetadata` (publish app/html) is the SAME ~7.6k gas/BYTE cost (measured
  via `debug_traceTransaction`: a 476-byte app's storage call used 3.61M). The
  old `1.3M + words*40k` (~1.25k/byte) was ~6x too low; now `1.2M + bytes*8500`.
  **Trust `debug_traceTransaction` (real exec) over `cast run` (replay) for
  gas** — `cast run` reported 364k for that call and sent a whole session
  chasing a phantom AA-validation bug.
- **Gemini model IDs flip — verify against the live API, never trust memory.**
  `DEFAULT_MODEL` = `gemini-3.5-flash` (as of 2026-05-29). `gemini-2.5-flash`
  now 400s; in the 0.10.x era it was the reverse. Before changing/defending a
  model constant, `curl` the live `:generateContent` endpoint. If the user says
  a model is wrong, TEST THEIRS FIRST.
- **Gemini rejects union-type tool schemas with a 400 — bricks ALL chat.**
  Function-declaration `input_schema` must use a single `type` (NOT
  `["string","null"]`) and no `additionalProperties`/`$schema`/`$ref`/`oneOf`/
  `anyOf`/`allOf`. Guard: `cargo test builtin_tool_schemas_have_no_union_types`
  (backends/gemini/tools/mod.rs), network-free. `minimum`/`maximum`/nested
  objects+arrays are fine.
- **PowerShell 5.1 stderr trap.** `release.ps1` wraps native commands in
  `Invoke-Native` because PS5 turns every cargo stderr line into a terminating
  error. Don't call `cargo`/`git`/`gh` directly inside the script.
- **Gemini 3.x `thought: false` parts.** The wire `Part` enum is untagged;
  `Part::Thought { thought: bool, .. }` is declared BEFORE `Part::Text`. Gemini
  3.x stamps every part with `thought`, so a normal text part deserializes into
  `Part::Thought { thought: false, text: Some(...), .. }`. Handle it explicitly.
- **SSE on wasm uses CRLF.** Browser fetch surfaces Gemini SSE with `\r\n\r\n`
  frame separators. `GeminiSseStream::take_frame` matches both `\n\n` and
  `\r\n\r\n`. Don't regress to LF-only.
- **`max-age=immutable` on `/pkg/*` was a footgun.** `vercel.json` uses
  `max-age=0, must-revalidate` so redeploys take effect without a hard-reload.
  Add a version query string before re-enabling long caching.
- **The release script only commits `Cargo.toml` + `Cargo.lock` +
  `CHANGELOG.md`.** Anything else shipping in a release must be committed BEFORE
  invoking the script. See RELEASING.md.

## Release process

```sh
# 1. Land feature work as normal commits.
# 2. Edit CHANGELOG.md — add `## [X.Y.Z]` heading (no date; script adds).
# 3. Run the atomic release script:
./scripts/release.sh X.Y.Z                  # bash / git-bash
pwsh scripts/release.ps1 -Version X.Y.Z     # PowerShell on Windows
```

Pre-flight → version bump → cargo verify → commit → tag → push → cargo publish →
GH release in one shot. On mid-way failure consult the recovery table in
`RELEASING.md`; don't hand-fix.

## The browser app

Compiled from `src/app/`, gated on `feature = "browser-app"` + wasm32.

**Design rule: no imperative DOM manipulation.** All HTML comes from `maud`
templates; the only DOM ops are `set_inner_html`/`set_outer_html`/
`insert_adjacent_html` targeted at fixed element ids (HTMX-style fragment swaps).
One delegated `click`/`keydown`/`submit`/`input` listener at document level
handles every interaction via `data-action`/`data-arg` on the target's ancestor
chain. Zero `Closure::wrap` calls outside those four listeners.

**Mount-time routing (`mod.rs::mount`):**

1. `?signer=1` → minimal signer chrome + postMessage listener
   (`signer::install_signer_listener`), return — the tab is now a cross-origin
   signing service. No apex wallet yet → `paint_signer` renders
   `signer_no_identity` and every challenge errors; NEVER silently generate a
   wallet here.
2. Else classify hostname via `tenant::current()`:
   - **`Host::Apex`** → identity-gated apex chrome. `paint_apex` calls
     `wallet_store::load()` (never creates) — fresh visitors see
     `identity_sidecar` with `[Create identity]`+`[Import existing seed]`, claim
     form disabled. Wallet creation only via `Action::CreateIdentity` /
     `Action::ImportSeed`.
   - **`Host::Tenant(name)`** → check `.lh_owner`: missing+`?claim=1` →
     auto-claim; missing+no hint → "claim this name" prompt; present → full chat
     app. Then `kick_verification` (background) queries on-chain owner
     (`registry::owner_of_name`), runs `verify::verify_owner` (iframe sign
     challenge), updates `#verify-pill`, and (visitors) swaps `#input-region`
     for a read-only banner. Fetches `tba_of_name` for 💰.
   - **`Host::Other`** (Vercel preview, localhost) → full chat app, no verify.

**Two surfaces per subdomain (public face vs studio)**, keyed on
`owner.is_some()` (local claim, refined by verification):
- **Owner** → lands in the **studio** by default, never auto-hijacked into
  fullscreen. Previews via `?view=public` (a `[view public]` header link →
  fullscreen face with a `[studio]` escape → `?edit=1`).
- **Visitor** → only ever the **public face**. No studio, no edit door.

`paint_public_face` paints the resolved face. `resolve_public_face(name)` reads
the on-chain choice under `keccak256("localharness.public_face")`
(`registry::public_face_of`) — `directory`/`app`/`html` — preferring local
working copy (owner previews unpublished edits) else published. `PublicFace` enum:
- **`Cartridge(wasm)`** — `app.rl` (local) or `app_wasm_of`;
  `paint_cartridge_fullscreen` → `display::run_in_root_canvas`.
- **`Html(src)`** — `index.html` (local) or `public_html_of` (key
  `keccak256("localharness.public.html")`); → `render_html_in_root_canvas`.
- **`Directory`** — `paint_public_landing`: profile (name, owner MAIN name when
  differs, TBA wallet, sibling agents via `registry::list_owned_tokens` — each
  rendered as a discoverable card with a truncated on-chain-persona preview,
  `registry::personas_of` batch-fetching all personas in one `eth_call`).

UNSET infers "cartridge if one exists, else directory". `owner_overlay` gates
the `[studio]` link. `Host::Other` uses `try_paint_app` (local `app.rl` only).

**Picker (admin → "public face").** From `templates::admin_app_section`:
`[directory] [publish app] [publish html]` → `Action::SetPublicFace` →
`events::run_set_public_face`. `directory` sets only the choice; `app`/`html`
compile/read local `app.rl`/`index.html` and publish it **plus** set the choice
in ONE sponsored Tempo tx (two `setMetadata` calls). `refresh_public_face_status`
reflects the current choice on admin open.

**Second-device owner upgrade.** A seed-bearing owner on their own subdomain
from a device WITHOUT `.lh_owner` is treated as a visitor. `paint_tenant` fires
`redirect_to_studio_if_owner` (background): if `verify::verify_owner` proves
control via the apex signer, it navigates to `?edit=1`. Skipped when the device
already claims ownership (so a deliberate `?view=public` never bounces). The
agent makes a subdomain "become" an app by writing `run_cartridge` source to
`app.rl` via `create_file` — only on an explicit "make this my permanent app".

**Cross-visitor publishing (on-chain).** Local `app.rl`/`index.html` are
owner-device working copies; *visitors* see published bytes stored in the
diamond under `metadata(tokenId, key)` via the owner-gated
`setMetadata(uint256,bytes32,bytes)` — no new facet. Keys:
`keccak256("localharness.app.wasm")`, `…public.html`, `…public_face`,
`…persona` (the headless-`call` system prompt, `registry::persona_of` /
`encode_set_persona`). Generic
`registry::{metadata_bytes_of, encode_set_metadata_bytes}` back the typed
`{app_wasm_of, public_html_of, public_face_of}`. Published via a sponsored Tempo
tx (owner signs sender_hash through the apex iframe; sponsor pays).

**Identity-gate invariant.** `wallet_store::load_or_create` no longer exists.
Two callers: `wallet_store::load()` (pure read → `Option<MasterWallet>`) and
`create_and_persist()` (generates+writes, only from `Action::CreateIdentity`).
Don't reintroduce load-or-create — silent wallet generation on a marketing-page
visit was the bug the gate fixes.

**Device linking is seed-adoption via QR (Option A).** Desktop encrypts its seed
under a one-time code; a QR fragment carries the ciphertext to
`localharness.xyz/?adopt=1#s=...`; the other device types the code to import the
SAME seed. Supersedes the old on-chain PairingFacet device-key flow (per-origin
`.lh_device_key`, ECIES-wrap-to-device Gemini key), now dormant (facet still cut).

Build: `wasm-pack build . --target web --out-dir web/pkg --release
--no-default-features --features browser-app`. wasm-opt is disabled in
`[package.metadata.wasm-pack.profile.release]` because the bundled wasm-opt
rejects post-MVP features modern rustc emits.

## The on-chain stack

Registry = the diamond proxy at `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` on
Tempo Moderato testnet (chain id 42431, RPC `https://rpc.moderato.tempo.xyz`).
**Fully reset 2026-06-01** — brand-new diamond, `$LH` token, 6551 infra; every
prior address abandoned. Facets churn via `diamondCut`; the diamond address is
the only durable handle (`registry::REGISTRY_ADDRESS`). **Per-facet addresses
deliberately not pinned** — query live via DiamondLoupeFacet (`facets()` /
`facetAddress(selector)`).

**Conventions:** each facet's storage sits at `keccak256("localharness.<facet>
.storage.v1")` in a `LibXyzStorage` lib; each cut via its own
`script/Add<Facet>.s.sol` (template `AddTbaFacet.s.sol`). See `contracts/README.md`.

Currently cut in:

- **DiamondCutFacet** — owner-only `diamondCut`. **DiamondLoupeFacet** —
  introspection + `supportsInterface`. **OwnershipFacet** — EIP-173 `owner()` +
  `transferOwnership`.
- **LocalharnessRegistryFacet** — `register / ownerOfName / ownerOfId / idOfName
  / nameOfId / idOf / setMetadata / nextId / metadata / isTaken`. Mints emit
  `Transfer(0, owner, tokenId)`. Cost-gated: `register` can pull
  `registrationCost()` $LH via `transferFrom` (owner-only `setRegistrationCost`);
  **currently 0 (FREE)** — cost==0 branch registers with no approve/claim.
- **ERC721Facet** — full ERC-721 + Metadata; every name is an NFT.
  `tokenURI(id)` → `https://<name>.localharness.xyz/`.
- **TbaFacet** — wraps EIP-6551. `tokenBoundAccount(id)` /
  `tokenBoundAccountByName(name)` return counterfactual; `createTokenBoundAccount
  (id)` deploys (anyone, idempotent).
- **MainIdentityFacet** — `registerMain / clearMain / mainOf / mainNameOf /
  isMain`. Holder's primary identity NFT; auto-set on first-claim.
- **FeedbackFacet** — `submitFeedback(string)` appends to an on-chain
  `Entry[]` in `LibFeedbackStorage`
  (`keccak256("localharness.feedback.storage.v1")`) AND emits
  `FeedbackSubmitted`. Read via state views `feedbackCount()` /
  `feedbackAt(i)` / `feedbackRange(start,count)` — `harvest-feedback.{sh,ps1}`
  now reads state (no `cast logs` / 100k-block window). Gas is the spam
  filter; 2048-byte cap. **GC:** owner-only `clearFeedback()` (added via
  `script/AddFeedbackClear.s.sol`) wipes the array — on-chain feedback is a
  TRANSIENT inbox (harvest/bridge off-chain via `scripts/test-fleet/
  feedback-to-issues.mjs`, then `scripts/clear-feedback.sh`). The immutable
  `FeedbackSubmitted` event log windows out naturally, so `localharness
  feedback` still shows recent notes after a clear.
- **CreditsFacet** — `LocalharnessCredits` TIP-20 distribution: `claimDaily /
  canClaim / dailyAllowance / lastClaimDay / creditsToken`; owner setters.
  Diamond holds `ISSUER_ROLE`; day = `block.timestamp / 86400` (UTC). Currency =
  "credits" (NOT fee-eligible). Daily-claim UI removed + `dailyAllowance` set to
  **0 on-chain (DISABLED** — sybil risk: free accounts × free daily mint = infinite
  credits); facet stays cut/wired (re-enable by setting an allowance). Funding is
  now redeem codes (`scripts/add-redeem-codes.sh`, tiers 10/100/1000) + `send_lh`.
- **RedeemFacet** — bootstraps $LH: owner loads `addRedeemCodes(bytes32[],
  uint256)`, holder calls `redeem(string code)` (mints via `ISSUER_ROLE`, burns
  code); owner-only `disableRedeemCodes`.
- **SessionFacet** — coarse time-boxed $LH sessions: `openSession()` pulls
  `sessionPrice()`, sets `expiry = now + sessionDuration()`; proxy reads
  `sessionExpiryOf`. **Currently `sessionDuration=3600, sessionPrice=1e19` (10 $LH/hr).**
  `sessionPrice` was 0 (free beta) but that was a sybil bypass of the redeem-code
  gate (free session ⇒ free model access with no `$LH`), so it's now priced — the
  proxy's session-OR-meter gate both require `$LH` now. So `call`/browser chat need
  funding (redeem code / `send_lh`) for unfunded identities. Owner-tunable
  (`setSessionPrice(0)` reopens free sessions).
- **CreditMeterFacet** — per-request $LH metering: `depositCredits(uint256)` tops
  up; `creditOf(address)` reads; `meter(address,uint256)` debits (meter-key
  only); owner-only `setMeter`.
- **X402Facet** — x402 EIP-712 "exact" settlement in $LH (agent-to-agent).
  `settle(...)` verifies (EOA `ecrecover` + EIP-1271, one-shot nonce), moves $LH
  payer→payee; `authorizationState`; `x402DomainSeparator()` (read live — binds
  chainId + diamond, so the reset changed it).
- **DeviceRegistryFacet** — enumerable linked-device index in ONE call:
  `linkDevice / unlinkDevice / devicesOf / isDeviceLinked`. Replaces `SignerAdded`
  log scraping (Tempo RPC caps at 100k blocks).
- **ReleaseFacet** — `releaseName(tokenId)`: holder burn that frees a name
  (**refuses the caller's MAIN**). Plus diamond-owner-only (EIP-173) admin
  reset: `adminBurnNames(uint256[])` / `adminResetAll()` force-burn names
  regardless of holder (testnet clean slate); a shared `_burn` clears exactly
  what `register()` writes (name↔id, ownerOfId, ERC721 owner/balance/approval,
  MAIN pointer) so names re-register cleanly.
- **PairingFacet** (dormant — superseded by QR seed-adoption). v2 selector
  `announcePairing(bytes32,bytes)` emits `PairingAnnounced(codeHash, device, …)`.
  Event-only. Old device-key path: phone opened `?pair=CODE`, generated a device
  key (stored per-origin in `.lh_device_key`, raw hex NOT seed), announced; desktop
  filtered by codeHash and `addSigner`'d it.

**Gemini key sync (per-MAIN, on-chain).** The sealed Gemini key lives under the
owner's **MAIN tokenId** (`mainOf(owner)`, fallback the name's own id), NOT
per-subdomain — every subdomain shares ONE key. On tenant paint,
`events::try_auto_restore_gemini_key` fetches the blob and decrypts via the apex
iframe (`open_key_via_iframe`, seed-derived) BEFORE the api-key modal shows.
Saving (`save_api_key_pressed`) best-effort `auto_sync_gemini_key`s to the MAIN
slot (`events::gemini_key_slot_id`). (Dormant pairing path: device-key-only phone
got an ECIES-wrapped copy under `keccak256("localharness.gemini_key.dev."||
device_addr)` via `registry::set_device_wrapped_key_sponsored`.)

**ERC-6551 reference contracts** (set via `TbaFacet::setTbaConfig`; redeployed
fresh in the reset):
- Registry: `0x2795810e5dfC8bC92Ef7fc9557F6c0699E11c3B3`
- Account impl: `0x86be7c44d1940F4dE53A738153A12FaAEa68B5a7`
  (`MultiSignerAccount` — CALL-only; additional-signer set on top of the NFT
  holder + EIP-1271 `isValidSignature`, so a MAIN can be controlled by multiple
  device EOAs without sharing the seed. Signer mgmt owner-only; signers bound to
  the enrolling holder (`_signerEnroller[signer] == owner()`), so an NFT transfer
  revokes prior device signers; `isValidSignature` rejects high-s (EIP-2). Bundle
  reads TBA addresses via the diamond, so a registry/impl swap needs no bundle
  change — but TBAs minted under prior infra resolve differently.)

## Credit proxy + $LH sessions / metering (LIVE)

Proxy at `https://proxy-tau-ten-15.vercel.app` (separate Vercel project "proxy",
TS Edge Function `proxy/api/gemini.ts`). Platform `$LH` credits are the
**primary** usage path; **BYOK** (own Gemini key) is the fallback. The proxy is
the ONE accepted off-chain component / only server; everything else stays Tempo +
the user's browser.

Transparent Gemini passthrough (same path/request shape) holding the platform
`GEMINI_API_KEY` in env. Auth = Ethereum personal-sign in the `x-goog-api-key`
header as `address:timestamp:signature`. Proxy verifies the sig, gates on EITHER
an active SessionFacet session (`sessionExpiryOf`) OR a CreditMeterFacet balance
(`creditOf`); per-request mode debits via the meter key (viem, EIP-1559) before
streaming Gemini.

Bundle helpers (`src/registry.rs`): `redeem_sponsored`, `open_session_sponsored`,
`session_expiry_of`, `session_price`, `deposit_credits_sponsored`,
`credit_balance_of`.

E2E: redeem code → $LH in wallet → `openSession()` (coarse) or `depositCredits()`
(per-request meter) → bundle calls proxy with signed header → proxy verifies +
checks session/meter → streams Gemini. BYOK skips the proxy, talks to Gemini direct.

## x402 agent-to-agent settlement (LIVE)

`src/x402_hook.rs` is an app-injected signer wired into `call_agent`: when one
agent calls another, the hook signs the EIP-712 authorization so the inter-agent
call settles in $LH via X402Facet. Client helpers (`registry.rs`):
`x402_domain_separator`, `x402_digest`, `sign_x402`, `settle_x402_sponsored`,
`x402_authorization_state`.

## Device index + name release (LIVE)

- **DeviceRegistryFacet** — enumerable linked-device list in ONE call
  (`devicesOf` / `isDeviceLinked`). `registry::remove_signer_sponsored` also
  unlinks the index. Reads: `registry::devices_of`, `is_device_linked`.
- **ReleaseFacet** — `releaseName(tokenId)` owner-only burn (refuses MAIN).
  Helpers: `registry::release_name_sponsored`, `release_name_calldata`;
  `consolidate_into_main_sponsored` releases all non-MAIN holdings in one batch.

## Agent tools + destructive-action convention

Subdomain tools (declared in `chat.rs::start_session`):
- **`create_subdomain(name)`** — register a name-only subdomain (sponsored mint).
- **`create_and_publish_app(name, source)`** — ONE-SHOT: compile the rustlite
  `source`, register `name`, then publish `app.wasm` bytes + `public_face="app"`
  to the new tokenId in ONE sponsored Tempo tx (same mechanism as the admin
  publish-app flow). Closes the per-origin gap where the agent could register a
  name but not populate another subdomain's app from the current tab. Compiles
  FIRST so a bad cartridge fails before any on-chain write.
- **`list_subdomains()`** — read-only; enumerates the owner's holdings.
- **`release_subdomain(name, confirmation)`** — DESTRUCTIVE. Burns the name
  (ReleaseFacet `releaseName`). Requires `confirmation == name` (typed in chat),
  refuses the caller's MAIN, NOT granted to subagents.
- **`send_lh(recipient, amount)`** — transfer real `$LH` to a `0x…` address or a
  subdomain name's on-chain OWNER (sponsored ERC-20 transfer via
  `run_sponsored_tempo_call`; `classify_recipient` in `encoding.rs` splits
  address vs name). Owner-only, NOT granted to subagents; amount > 0. The
  free-form "pay X" tool (agents already pay each other via `call_agent`/x402).
- **`read_self_docs()`** — read-only. Returns the agent's own runtime docs:
  fetches the live `https://localharness.xyz/llms.txt`, falls back to an
  embedded summary (`self_docs::RUNTIME_SUMMARY`) offline. The same summary is
  injected into every system prompt (`self_docs::system_prompt_digest`) so the
  agent has grounded priors about its own platform/SDK and can self-diagnose.

**Continuous execution (`chat.rs::run_send`).** One user message drives the
agent until the goal is done, not one step. `run_send` loops over
`stream_turn(agent, TurnInput)`: the first turn carries the user's prompt (with
a user bubble); when a turn ends with tool activity but **no** completion signal
(`TurnOutcome::Incomplete`) it auto-continues with an internal
`AUTO_CONTINUE_NUDGE` (no user bubble) — no per-step nudge from the user.
`stream_turn` classifies each turn: `Finished` (model called `finish`),
`FinalAnswer` (pure text, no tool call → don't spam continues), `Incomplete`,
`Empty`, `Error`, `Cancelled`. Bounded by `MAX_AUTO_CONTINUATIONS = 10`;
respects `TURN_CANCEL` (stop button) every iteration and the `TURN_ACTIVE`
one-turn-at-a-time guard across the whole run. History/opfs are saved after
every turn so progress shows incrementally.

**Ownership = on-chain, not a local cache.** `.lh_owner` (owner.rs) is no
longer a random device UUID — it stores the on-chain owner ADDRESS this device
last *proved* it controls (written only after a `VerifyResult::VerifiedOwner`).
The registry is the sole authority: every tenant load re-verifies; the hint
only decides which face paints FIRST and `kick_verification` deletes it
(`owner::forget` + repaint public face) the moment the chain disagrees — so it
can never lie past the initial frame. `owner::remember(addr)` / `forget()` /
`current_owner()` (claim()/release() are gone).

Hard convention: **destructive / irreversible actions require a typed
confirmation that is never auto-filled** — the agent must ask the user to type
the exact value before proceeding. Mirror this for future destructive tools.

## Tempo Transactions + sponsorship (post-0.10.24)

User-facing claim flow uses Tempo's **native** AA tx type (`0x76`) so users hold
ZERO of anything — no native gas, no TIP-20. `src/app/sponsor.rs` signs as
`fee_payer` and pays fees in AlphaUSD on every user tx.

### Wire format (live-verified — see `[[tempo-tx-findings]]`)

```text
0x76 || rlp([
    chain_id, mpfpg, mfpg, gas_limit,
    calls,                // [[to, value, input], ...]
    access_list,          // EIP-2930
    nonce_key, nonce,     // Tempo's 2D nonce
    valid_before, valid_after,
    fee_token,            // 0x80 (empty) in sender hash if sponsored
    fee_payer_signature,  // 0x00 placeholder in sender hash; 0x80 or rlp([v,r,s]) serialized
    aa_authorization_list,
    key_authorization?,   // truly optional; omit when None
    sender_signature      // flat 65 bytes (r||s||v with v=0/1)
])
```

Sender hash: `keccak256(0x76 || rlp([1..14_without_sender_sig]))`.
Fee-payer hash: `keccak256(0x78 || rlp([1..10, fee_token, sender_address,
aa_authorization_list, key_authorization?]))`. The spec page is missing
`aa_authorization_list` at position 13 of the fee_payer hash — discovered by
diffing against `wevm/ox`'s `TxEnvelopeTempo`. Captured in memory so we don't
relearn.

### $LH is TIP-20-shaped credit, NOT fee-token-eligible

Tempo's `fee_token` validation requires TIP-20 compliance AND `currency()=="USD"`.
`LocalharnessCredits` at `0x90B84c7234Aae89BadA7f69160B9901B9bc37B17` (fresh in
the reset) implements the TIP-20 surface (memo transfers, supply cap, roles) but
returns `currency()=="credits"`, so the chain rejects it as a fee_token —
intentional ($LH = in-system credits, not gas). **AlphaUSD**
(`0x20c0000000000000000000000000000000000001`) remains the sponsor's fee_token.
$LH supply controlled — diamond holds `ISSUER_ROLE`; mint paths are
`CreditsFacet.claimDaily()` and `RedeemFacet.redeem(code)`. Pre-reset tokens
orphaned; no migration.

### Sponsor key

Lives in `src/app/sponsor.rs` as a const — the **dedicated low-budget sponsor**
`0x0AFf88Ad13eF24caC5BeFD0F9Dc3A05DF79a922C` (rotated 2026-05-25). It is NOT the
deployer/owner: the diamond owner (EIP-173 `owner()`, the key for `diamondCut` +
any owner-gated admin call like `adminResetAll`) is `0x313b1659F5037080aA0C113D386
C5954F348EF1e` and is **not in the repo** — only the holder can cut/upgrade. The
embedded sponsor only pays user fees in AlphaUSD; if the bundle is extracted, loss
is capped at its balance. **Rotate again before mainnet** (passkey, Stripe top-up,
etc.). Tempo access keys CANNOT sign as `fee_payer` (confirmed reading their
open-source SDK — `[[access-key-fee-payer-finding]]`); fee_payer must come from the
root key.

### Migration status (complete)

Every user-facing write goes through `events::run_sponsored_tempo_call`. The
shared mechanism is the iframe signer's `lh-sign-digest` message (tenant computes
sender_hash, apex wallet signs it, embedded sponsor signs `fee_payer`). Sponsored
flows: apex/tenant first-claim, `claim_and_maybe_set_main_sponsored`,
`lh_transfer`, `submit_feedback`, publish app (`setMetadata`), add/remove device
signer, `register_main_sponsored`. Self-paid `sign_and_submit_call` /
`register_main` remain in `registry.rs` for off-bundle/native callers, unused by
the browser UI.

## What's planned

SDK runtime (0.2.x–0.6.x), browser IDE (0.7.x), platform layer (through 0.10.0),
and Tempo native AA (post-0.10.24) shipped. Next:

- **MPP / x402 payment hooks** — pre-tool-call hook requiring payment to the
  agent's TBA, or agent-pays-agent over Stripe MPP (preferred) / Coinbase x402.
  Fits the existing `Hook` trait.
- **ERC-8004 reputation + validation facets** — cut into the diamond; agents
  accrue reputation; validators stake to re-execute claims.
- **TBA-driven actions in the bundle** — UX for "send this tx from your agent's
  TBA"; contract surface ready, mostly UI.
- **Second backend** — ✅ DONE (0.23.0). Anthropic (`src/backends/anthropic/`,
  feature `anthropic`) ships as a second `ConnectionStrategy`; the credit proxy
  (`proxy/api/gemini.ts`) is now MULTI-PROVIDER (routes Gemini `/v1beta/*` +
  Anthropic `/v1/messages`, per-model `$LH` pricing, both platform keys); CLI
  `call --model <id>` + a browser model selector (`src/app/model.rs`) pick Gemini
  or Claude — on platform credits, no per-user provider key. Remaining: OpenAI /
  local-WebGPU backends + the own coding model — see `design/model-agnostic.md`
  Phases D–F.
- **Tool-call activity in restored transcripts** — ✅ DONE. `TranscriptEntry`
  carries `tool_calls` (with results/errors); `history.rs::paint_entries` replays
  them with the live `tool_call_block`/`tool_call_result` templates. Backends
  project their wire history into it (`project_history`). Backward-compatible
  (old text-only saves still load).
- **At-rest encryption** — wallet-derived sym key over OPFS contents.

## Filesystem trait

The 8 fs builtins call `crate::filesystem::Filesystem`, not `tokio::fs`. Surface:
`read`, `write_atomic`, `metadata`, `read_dir`, `walk`, `delete`, `rename`
(default = read+write+delete; NativeFilesystem overrides rename with
`tokio::fs::rename`). Two impls:

- **`NativeFilesystem`** (`feature = "native"`): tokio::fs + walkdir + tempfile;
  atomicity via tempfile + rename.
- **`OpfsFilesystem`** (wasm32): OPFS via web-sys; atomicity via
  `FileSystemWritableFileStream.close()` swap.

`GeminiConnectionStrategy::connect` honors a caller-supplied `Filesystem` via
`with_filesystem`, else auto-installs `NativeFilesystem` on native (None on wasm
— caller supplies OPFS; the browser app does). Plug-in impls hand a
`SharedFilesystem = Arc<dyn Filesystem>`.

## Documentation SOP

Five surfaces; keep in sync on every change.

| Surface | File | Covers |
|---------|------|--------|
| docs.rs | `///` comments in source | Public API: every `pub` item gets a one-liner |
| README.md | repo root | Quick start, features, architecture, links |
| CLAUDE.md | repo root | Full internal context: layout, gotchas, plans |
| llms.txt | `web/llms.txt` | Agent capabilities, RPC format, on-chain registry |
| CHANGELOG.md | repo root | Per-version changes (Keep-a-Changelog) |

**When to update what:** new pub API → `///` one-liner (+README if surface
changes); new file/module → CLAUDE.md repo tree; new agent tool → `llms.txt` +
`chat.rs::start_session` prompt; new facet → CLAUDE.md on-chain + `llms.txt`
registry; browser UX → CLAUDE.md browser section; release → CHANGELOG.

**Single source of truth:** code comments for API behavior; CLAUDE.md for
internal architecture; llms.txt for agent-facing capabilities (concise,
machine-readable); `chat.rs::start_session` prompt for what the agent knows about
itself.

**Verify before any release:**
```sh
cargo doc --no-deps 2>&1 | grep "warning.*missing"   # undocumented pub items
curl -s https://localharness.xyz/llms.txt | head -5  # verify llms.txt deployed
```
