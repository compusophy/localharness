# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.10.27]

### Added

- **Rustlite compiler** (`src/rustlite/`). In-crate Rust-subset compiler
  that takes source code and emits wasm bytes. Full pipeline: lexer →
  AST → recursive-descent parser → typechecker → codegen. Supports
  structs, enums (unit/tuple/struct variants), functions, let/mut,
  assignment, if/else, match with pattern destructuring, while/loop/
  break/continue, binary/unary ops, method-call desugaring, string
  literals with data-segment interning, tail expressions. No references,
  no lifetimes, no traits, no generics, no closures — by design (arena-
  per-invocation memory model). 27 tests. ~2300 lines.
- **Per-agent tool allowlist** (studio v2). OPFS-persisted
  `.lh_tool_allowlist.txt` restricts which built-in tools the agent
  exposes. Admin UI: checkbox grid of all 13 builtins, save/reset.
  Empty = unrestricted. Takes effect on next session start.
- `NodeList` web-sys feature for checkbox query in the allowlist UI.

### Changed

- README status line updated to reflect rustlite compiler and tool
  allowlist features.

## [0.10.26] - 2026-05-26

Big architectural sweep — MultiSignerAccount, credit token + cost gates,
composable subdomains, the first agent-differentiation hook. Everything
ships through the same diamond at `0x6f2858…2930`; bundle still runs
zero-gas / zero-stablecoin from the user's perspective via sponsored
Tempo txs.

### Added (contracts)

- **MultiSignerAccount.sol** at `0x100967d751C97265F3ee93244fAeE8caf29cB48D`.
  Replaces the vanilla ERC-6551 account impl via
  `TbaFacet.setTbaConfig`. Adds an `authorizedSigners` mapping +
  EIP-1271 `isValidSignature` on top of the standard execute / token
  / owner surface. NFT holder is always implicit signer; extra signers
  added via `addSigner` from any already-authorized address. Same TBA
  can be controlled from multiple device EOAs without sharing the
  seed.
- **LocalharnessCredits.sol** at `0xC1FC0452670049953ED64f2B177beBed4090A5bc`.
  TIP-20-shaped in-system credit token. `currency() == "credits"`
  (NOT "USD") — explicitly NOT fee-token-eligible by design; AlphaUSD
  stays as the sponsor's fee channel. Full ERC-20 + memo variants
  (`transferWithMemo` / `mintWithMemo` / `burnWithMemo`) + supplyCap
  + ISSUER_ROLE. Replaces the orphaned standalone ERC-20 at
  `0xcC8A300658…`.
- **CreditsFacet** cut into the diamond. Diamond holds ISSUER_ROLE
  on the token; `claimDaily()` is the only path to fresh supply. One
  claim per address per UTC day (`block.timestamp / 86400`). Default
  100 LH/day, owner-tunable via `setDailyAllowance`.
- **LocalharnessRegistryFacet** re-cut with cost gate + treasury:
  `setRegistrationCost` / `registrationCost` (default 50 LH per
  register), `_chargeRegistrationCost` pulls via `transferFrom` into
  the diamond's own balance, plus owner-only `withdrawTreasury` +
  `treasuryBalance` for recycling accumulated fees.
- **MainIdentityFacet** re-cut with optional cost gate:
  `setMainCost` / `mainCost` (default 0 — sybil deterrent layer
  available when owner wants to ramp).

### Added (browser app)

- **Composable subdomains.** `?embed=1` paints any subdomain as a
  minimal identity card (own origin, own OPFS, own signer iframe);
  `?compose=a,b,c` renders a host shell of sibling iframes at depth
  1, auto-resized via postMessage. Try
  `localharness.xyz/?compose=name1,name2,name3` against real names.
- **Linked devices** section in apex admin: paste a phone-side
  address, click add, sponsored `tba.addSigner` fires. Brother test
  ready.
- **Daily credits** section in apex admin: live balance pill + claim
  button. Identity creation auto-claims first-day credits.
- **Agent act panel** in the apex agents list: click [act] on any
  owned agent, open inline send-LH form. Submits sponsored
  `tba.execute(credits, 0, transfer(...), 0)` — proves "agents own
  wallets" end-to-end.
- **Custom system prompt** per agent (studio MVP). Tenant admin grows
  an "agent prompt" textarea; `chat::start_session` appends the
  saved content under an `=== Owner instructions ===` header.
  First real agent-differentiation hook.

### Changed

- **Every user-initiated chain call is sponsored Tempo tx.** The
  per-turn payment in `chat.rs::collect_payment_if_required`
  migrated off the legacy `lh-sign-tx` iframe path onto sponsored
  Tempo. Visitor still spends their own LH; sponsor pays the gas in
  AlphaUSD.
- **Gas budgets recalibrated** on every sponsored flow after
  observing live `out of gas` reverts. `register` 500k → 2M
  (eth_estimateGas reports ~1.32M inner). Proportional bumps on
  `register_main_sponsored`, `lh_transfer`, `submit_feedback`.
- **Create button surfaces failure visibly** — red `✗ failed` /
  `need N more LH` label cleared on next keystroke. Silent reset
  to disabled invited frustrated re-clicks; now every click has a
  visible outcome.
- **Apex placeholder** copy `pick a name` → `choose a name`.

### Removed

- **Legacy `lh-sign-tx` iframe path.** No remaining callers after
  the per-turn payment migration. Deleted from both
  `signer.rs::build_tx_response` (and the `field_string` /
  `field_u128` / `is_address_shape` helpers) and
  `verify.rs::sign_tx_via_iframe` + `SignTxRequest`. The
  `lh-sign-digest` raw-32-byte path (sponsored Tempo) is the sole
  tx-signing channel through the apex iframe.
- **`run_bootstrap_funding`** in events.rs — `tempo_fundAddress`
  gas drip + old `LocalharnessToken.faucet` were both made obsolete
  by sponsored Tempo + CreditsFacet. Replaced with
  `run_initial_credit_claim` which fires one sponsored
  `claimDaily()` on identity creation.
- **`token_faucet_self`** in registry.rs — the new credit token
  has no `faucet(address)` method; SDK callers use
  `claim_daily_sponsored` against the diamond instead.

### Fixed

- **wasm bundle hosts examples without breaking `cargo test`** —
  `examples/tempo_tx_live.rs` now declares
  `required-features = ["wallet"]` in Cargo.toml. Surfaced by the
  release script running plain `cargo test` (no `--features
  wallet`) during its verify step.

## [0.10.25] - 2026-05-25

Sponsored Tempo tx is now the default for every user-initiated
on-chain call from the bundle. Users hold zero of anything — no
native gas, no TIP-20 stablecoin — and still transfer `$LH`,
submit feedback, and change their MAIN identity.

### Added (browser app)

- **`lh-sign-digest` iframe-signer message.** The apex iframe now
  signs raw 32-byte digests with the master wallet. The tenant
  builds a Tempo Transaction locally (sender_hash via
  `tempo_tx::TempoTx::sender_hash`), hands the digest to the
  iframe, gets back a 65-byte signature, signs the fee_payer hash
  with the bundle sponsor key, and submits — no encoding
  duplication on the iframe side. Auto-approve at the iframe;
  consent is collected at the tenant origin per the existing
  trust model.
- **`run_sponsored_tempo_call`** in `src/app/events.rs` — the
  shared orchestrator that `lh_transfer` and `submit_feedback`
  now route through. Verifies the iframe signature recovers to
  the expected sender address before letting the sponsor pay.
- **`register_main_sponsored`** in `src/registry.rs`. Pair with
  `register_main` for the legacy self-paid case;
  `claim_and_maybe_set_main_sponsored` now delegates to the
  shared helper.

### Changed (browser app)

- **`run_lh_transfer` migrated** off the legacy EIP-155 iframe
  path onto sponsored Tempo tx. Sending `$LH` to another address
  no longer requires the sender to hold native gas — fees are
  paid in AlphaUSD by the bundle sponsor.
- **`submit_feedback_onchain` migrated** the same way. On-chain
  feedback is free to the user.
- **Sponsor key rotated** off the deployer wallet onto a
  dedicated low-budget testnet wallet
  (`0x0AFf88Ad13eF24caC5BeFD0F9Dc3A05DF79a922C`). The new wallet
  is funded with ~1M AlphaUSD via `tempo_fundAddress`; extraction
  blast radius is now bounded to that balance rather than the
  deployer's full holdings. Old sponsor funds remain claimable
  from the deployer key.

## [0.10.24] - 2026-05-25

UX cleanup: silent validation + uniform header padding.

### Removed (browser app)

- **All explanatory validation strings.** "name must be 3-32 chars,
  a-z 0-9 -" deleted from `Action::ApexClaim`. "need at least 3
  chars" / "max 32 chars" deleted from `on_apex_input`. The
  `create_subdomain` agent tool's error message no longer recites
  the rule either. The user has asked for this cleanup multiple
  times — captured durably as feedback-no-explanatory-validation
  so it won't get reintroduced.

### Added (browser app)

- **Submit-button gating.** Apex's `<button#create-btn>` renders
  `disabled` initially; `on_apex_input` flips the attribute via a
  new `set_create_button_enabled` helper based on the silent length
  check. The button BEING disabled IS the validation feedback —
  no text needed.

### Changed (style)

- **Header + footer get uniform 16px padding** (`.header-inner` /
  `.footer-inner` were `4px 16px` → now `16px`). The admin button
  now sits with equal breathing room on all four sides instead of
  pressing against the top/bottom border. Same for the feedback
  button in the footer.
- **Button padding `5px 12px` → `10px 12px`.** Closer to balanced
  proportions — the SSOT button is less "portrait-aspect" rectangle.
  Affects every button in the app (admin, create, send, reset,
  feedback, etc.).

## [0.10.23] - 2026-05-25

Fresh diamond, fresh start. New deployer key, new diamond address,
zero test registrations carried over.

### Changed (on-chain)

- **New registry diamond** at
  `0x6f2858b4b10bf8d4ea372a446e69bea8fbce2930` on Tempo Moderato.
  Deployed via `DeployDiamond.s.sol` + `AddErc721Fresh.s.sol` +
  `AddTbaFacet.s.sol`. Fresh `nextId=1`, no inherited state.
  Owner is a fresh testnet key
  (`0x313b1659F5037080aA0C113D386C5954F348EF1e`) generated for
  this redeploy; old admin EOA `0x81E9c327…` retains ownership of
  the abandoned previous diamond at `0xed7a2d…c656d` but the
  bundle no longer references it.
- **New ERC-6551 registry + account impl** redeployed alongside
  the TBA facet, wired via `TbaFacet.setTbaConfig`. The bundle
  reads them through `tbaRegistry()` / `tbaAccountImpl()` — no
  bundle-side address constants to maintain.

### Changed (bundle)

- **`src/registry.rs::REGISTRY_ADDRESS`** points at the new diamond.
- **`CLAUDE.md` header + diamond section** updated with the new
  address + history note about the predecessor.

### Removed

- **`WipeFacet.sol` + `AddWipeFacet.s.sol`** dropped. Were added
  in 0.10.22 to nuke the old diamond's storage, but a fresh
  redeploy makes them moot. If we ever need a wipe again, restore
  from `git show v0.10.22`.

### Added (contracts)

- **`contracts/script/AddErc721Fresh.s.sol`** — variant of the
  existing `AddErc721Facet.s.sol` migration script that skips the
  "remove old selectors" step. Use for cutting ERC-721 onto a
  freshly-deployed diamond (no migration needed). Kept for the
  next time a fresh deploy is required.

## [0.10.22] - 2026-05-25

Subdomain IS the identity primitive. No more "create wallet first,
then claim a name" pre-step. Wallets without subdomains shouldn't
exist; the apex claim form now folds wallet generation into the
same submit.

### Changed (browser app)

- **Apex chrome is one step, not two.** `apex_step_identity` and
  the `[Create identity] / [Import seed]` button pair are gone. The
  apex page renders the claim form unconditionally — fresh visitors
  and returning visitors see the same surface. `apex_step_agents`
  renamed to `apex_claim`. Seed import lives in the admin dropdown
  for the recovery / cross-device case (already shipped in 0.10.20).
- **`run_apex_claim` auto-generates a wallet on first submit.** If
  no wallet exists when the user hits create, the flow generates
  one (`wallet_store::create_and_persist`), stashes it in App
  state, faucet-funds it, and registers the name — all inside the
  same async future. Removes the previous "wallet not loaded —
  refresh" dead-end where a partial create-identity sequence left
  the user stuck.

### Added (contracts)

- **`WipeFacet.sol`** at `contracts/src/facets/`. Owner-only
  `wipeRegistry(uint256 maxIds)` iterates `1..nextId`, deletes
  per-token mappings (ownerOfId, nameOfId, idOfName, tokenApprovals),
  decrements balanceOf for each previous owner, and resets nextId
  to 1 when the wipe covers everything. Pass `maxIds=0` to nuke all;
  non-zero for chunked wipes if the block gas limit comes up.
  Emits `RegistryWiped(from, to)`. Testnet-only nuke button.
- **`AddWipeFacet.s.sol`** cut script — follows the `AddTbaFacet.s.sol`
  template. Deploys the facet, cuts `wipeRegistry.selector` onto the
  existing diamond at `$DIAMOND`. Run with:

  ```
  DIAMOND=0xed7a2d170ab2d41721c9bd7368adbff6df0c656d \
  EVM_PRIVATE_KEY=0x... \
  forge script script/AddWipeFacet.s.sol \
      --rpc-url tempo_moderato --broadcast
  ```

  Then call `wipeRegistry(0)` from the same key.

### Note on what's still incomplete

- The wipe doesn't iterate `metadata[tokenId][key]` (Solidity can't
  enumerate map keys without a key index). Metadata for nuked tokens
  is orphaned in storage; reads return empty bytes per default. No
  user-visible impact.
- After a wipe, existing client devices with a `.lh_wallet` and a
  `.lh_owner` marker pointing at a now-extinct token will show stale
  state until the user resets local OPFS via admin → reset. Acceptable
  for testnet but flagged.

## [0.10.21] - 2026-05-25

Agents grow teeth: `create_subdomain` + `spawn_recursive_subagent`,
plus a system prompt rewrite so the model stops gaslighting users
about what it can do.

### Added (browser app)

- **`create_subdomain(name)` agent tool** — closure tool registered
  in `chat.rs::start_session`. The agent itself can register a new
  `<name>.localharness.xyz` on-chain via the apex signer iframe. The
  apex claim flow, exposed as an agent capability. Returns
  `{ name, url, owner, tx_hash }`.
- **`spawn_recursive_subagent(system_instructions, prompt)` agent
  tool** — closure tool that spins up a full `Agent::start_gemini`
  with the same key + filesystem + tool surface as the parent
  (including itself). Drives the subagent through `chat()` until
  completion and returns the final text response. Coexists with the
  existing one-shot `start_subagent`; pick recursive when the
  subagent needs tools, one-shot for pure text reasoning.

### Changed (browser app)

- **System prompt rewrite** at `chat.rs:235`. Switched from a
  paragraph blob to a structured catalogue with explicit
  affirmation ("you DO have these tools") for `delete_file` and
  every other builtin. Lists `create_subdomain`,
  `spawn_recursive_subagent`, and `start_subagent` under
  "Platform". Fixes the prior agent habit of saying "I cannot
  delete files" when the tool was registered all along —
  `gemini-2.5-flash` hallucinates tool availability if the
  prompt isn't authoritative.

### Added (research)

- **`design/main-identity.md`** — design note for sybil-resistant
  "MAIN" identity. Frames the problem (the 0.10.20 first-claim-is-
  primary inversion makes parallel MAINs trivially cheap), surveys
  candidate mechanisms (cost-locked MAIN, reputation-bound MAIN,
  social-graph anchoring, third-party PoP, accept parallel MAINs),
  proposes a hybrid for 1.0.0. No implementation; document the
  design space before shipping any MAIN flag on chain.

### Note on what's still incomplete

- Recursion-depth control on `spawn_recursive_subagent` is implicit
  (each call costs Gemini tokens; deeper trees fail organically).
  An explicit `max_depth` arg or ToolContext-based counter would
  be safer for adversarial prompts.
- Cross-device pairing tested only in concept — paste-seed-on-mobile
  flow needs a live two-device run.

## [0.10.20] - 2026-05-25

Self-sovereign tenant chrome + inline first-claim + $LH transfer UI.

The big shift: tenants no longer bounce to the apex page for anything.
Seed reveal, seed import, identity creation, name registration, token
transfers — all run inline from the subdomain via an extended signer-
iframe protocol. The first subdomain a fresh visitor claims becomes
their primary identity; subsequent claims on other names reuse the
same wallet across the family.

### Added (browser app)

- **Extended apex signer protocol** (`src/app/signer.rs`) with four new
  message types: `lh-reveal-seed`, `lh-create-wallet` (ensure-semantic
  by default; pass `overwrite=true` to force regenerate),
  `lh-import-seed`, `lh-claim-name`. Runs at apex origin so OPFS reads
  / writes / claim flow stay on the apex side; replies go back via
  postMessage to the tenant subdomain.
- **`verify::reveal_seed_via_iframe` / `create_wallet_via_iframe` /
  `import_seed_via_iframe` / `claim_name_via_iframe`** — client-side
  wrappers around the new signer messages. Reuse the existing
  `signer_iframe_request` lifecycle (`lh-signer-ready` ping +
  correlation-id-filtered listener).
- **`Action::ClaimOnChain`** — tenant-side first-claim. Ensures the
  apex wallet exists (without overwriting an existing one), then
  registers the name on-chain via the iframe, then sets the local
  OPFS marker, then re-paints as owner. Replaces the previous
  "claim on apex" bounce link.
- **$localharness transfer UI** in the financial card.
  `lh_transfer_form` template + `Action::LhTransfer` handler — types
  in a recipient (default to the agent's TBA) + an amount, signs
  `transfer(address,uint256)` via the iframe signer, submits via
  `submit_and_wait_receipt`. Refreshes the card balance on success.

### Changed (browser app)

- **`admin_dropdown_tenant` is self-contained** — seed reveal + seed
  import sit inside the tenant admin alongside the API key + reset.
  No more "manage at apex →" copout. Identity actions still run at
  the apex origin under the hood (via the iframe), but the user never
  has to navigate there.
- **`unclaimed` template** now shows a `[claim <name>]` button that
  fires `Action::ClaimOnChain` instead of linking to apex. The
  inline-claim flow handles wallet creation automatically when the
  visitor has no apex identity yet.
- **`Action::RevealSeed` / `ImportSeed` / `CreateIdentity` are
  context-aware** — apex: direct OPFS access (existing path). Tenant:
  routes through the signer iframe so the wallet stays at apex.
  Cross-device pairing falls out of import-on-tenant: paste your
  desktop seed in mobile's tenant admin, the wallet lands at apex
  origin on the mobile device.

### Note on what's still incomplete

- The transfer form is bare HTML — no dedicated CSS, picks up the
  inherited form styles. Visual polish landed at a later pass.
- TIP-20 spec validation: the contract at
  `0xcC8A300658dC8d0648D984A5066Af3F8E75e0936` accepts ERC-20-style
  `transfer(address,uint256)` calldata (the bundle has been using
  `balanceOf(address)` against the same selector since 0.10.x).
  Calling it "TIP-20" reflects the chain it runs on; the wire
  surface is ERC-20-compatible.
- Owner's own $LH balance isn't displayed yet — the financial card
  still shows only the agent's TBA balance. Send-from-owner works;
  see-your-own-balance is a one-line addition next pass.

## [0.10.19] - 2026-05-24

Mobile rebuild + permanent feedback footer.

### Changed (browser app)

- **Sticky permanent footer is back** with a centered `feedback`
  button. Same min-height + padding pattern as the header. Lives
  on every page (apex, tenant, unclaimed).
- **Mobile is now single-pane with a tab bar.** Below 900px the
  vertical-stack rails are replaced by a `[files][edit][chat]
  [agent]` tab bar at the top of main. Exactly one panel shows
  at a time; CSS uses a `tab-<name>` class on `#layout` to
  switch. `chat` (default) shows transcript + terminal stacked.
- **Mobile viewport cutoff fixed.** `html` / `body` / `#root`
  use `100dvh` (dynamic viewport height) instead of `100vh` so
  the bottom doesn't get hidden under Safari's resizing address
  bar or Android's gesture affordances.
- **Terminal stays inside the chat tab** on mobile (no more
  `position: fixed` overlay hack). Always reachable when the
  user picks the chat tab.

### Added (browser app)

- **`Action::FeedbackOpen` / `FeedbackClose` / `FeedbackSubmit`**
  — feedback button opens an inline modal (no JS dialog) with a
  textarea + submit. Submit appends `{ISO-timestamp}\t{TEXT}` as
  a line to `.lh_feedback.txt` in this origin's OPFS. User can
  copy it off later. **On-chain `FeedbackFacet` submission is
  the next step** — needs a contract deploy + bundle wiring;
  parked here for the next session.
- **`Action::ShowTab(name)`** — mobile tab switcher. Pure DOM
  class flip on `#layout` (`tab-files` / `tab-edit` / `tab-chat`
  / `tab-agent`) + toggles `.active` on the matching tab button.

### Note on what's still incomplete

- Antigravity-style top-right icon toggles (replacing the four
  full-strip rails with small icon buttons) — separate session;
  needs SVG icons + a redesign of how the panels signal their
  state when "off."
- On-chain feedback contract (`FeedbackFacet.sol`) — needs a
  deploy + bundle wiring. For now feedback just lives in
  per-origin OPFS.

## [0.10.18] - 2026-05-24

File delete + rename — both as agent tools and as an in-list
delete affordance. The agent now actually has the tools the user
expected when they said "we can't even delete files can we??"

### Added (SDK)

- **`Filesystem::rename(from, to)`** trait method. Default impl is
  read + write_atomic + delete (works for any backend, no atomicity
  but safe). `NativeFilesystem` overrides with `tokio::fs::rename`
  for true atomic moves on the same filesystem.
- **`BuiltinTool::DeleteFile`** + **`BuiltinTool::RenameFile`**
  variants. Both wired into `register_builtins` via the existing
  `fs_tool!` macro — works on every backend that supplies a
  filesystem (native, OPFS).
- **`backends::gemini::tools::DeleteFile`** — wraps
  `Filesystem::delete`. Recursive for directories. Tested
  (deletes existing file; errors on missing path).
- **`backends::gemini::tools::RenameFile`** — wraps
  `Filesystem::rename`. Rejects identical from/to. Tested
  (renames file; rejects same-path).

### Added (browser app)

- **In-list file delete affordance.** Hovering a row in the file
  list reveals a small × button on the right. Click deletes the
  file in one shot — no per-row confirm dialog (mistakes can be
  re-created; the wipe button is the heavyweight "everything"
  confirm flow if you want to nuke the whole origin).
- **System prompt updated** to mention `delete_file` and
  `rename_file` as available tools — and to NEVER delete the
  internal `.lh_*` dotfiles + confirm before deletes unless
  explicitly asked.

## [0.10.17] - 2026-05-24

Big polish pass: ALL chatty status text dead, button + font
unification, panel headers de-duped, mobile terminal-as-sticky-
footer, subdomain identity moved to the agent tab, owner address
exposed in admin. Plus apex declutter.

### Changed (apex)

- **Agents list reduced to bare names.** No token id (`#3`), no 💰
  emoji, no TBA address, no `.localharness.xyz` suffix. Just the
  subdomain name as a link, centered, top-aligned. Hover colors
  accent.
- **Create form: input + button stacked centered.** Equal 24px
  spacing above + below the input. Button is a wide CTA
  (min-width 200px, 12/32 padding). Centered horizontally.
- **No "3–32 chars" hint, no `.localharness.xyz` suffix chip.**
  The button rejects invalid input directly; no bloat copy.
- **Input centered text** so the typed name reads as the visual
  focal point.

### Changed (browser app)

- **Header strips to brand + admin only.** Subdomain name moved
  off the header into the agent tab's first line. Header is now
  `[localharness]` left, `[admin]` right, nothing in the middle.
- **Panel headers de-duped.** Files + agent columns no longer have
  their own internal `panel-title` (`files` / `agent`) — the rail
  label outside the panel IS the title. The `col_side` helper
  returns body-only.
- **`refresh` + `wipe` buttons removed from the files header.**
  Admin reset already handles wipe; the file list auto-refreshes
  after navigation + saves.
- **Agent tab gets `name` row** at the top showing the subdomain
  (which the header lost). Plus `owner`, `wallet`, `balance` as
  before.
- **Admin (tenant) shows the owner address** (recovered from
  verify state) + a `manage at apex →` link so seed reveal /
  import is reachable from a subdomain.
- **Terminal is a sticky footer on mobile.** Below 900px the page
  scrolls freely, but the terminal panel + rail are
  `position: fixed` at the bottom of the viewport, always
  reachable. Side panels (files / agent) get a 40vh max-height
  so they stop overflowing the page.

### Fixed

- **No more "thinking…" / "starting session…" / "done · ttft N
  ms" status writes.** The terminal status stays empty in normal
  use; only fills on errors or payment-flow events.
- **Terminal pinned to bottom on desktop** via `margin-top: auto`
  so it never floats up when the edit panel is closed.

### Style

- **Single button archetype across the whole app.** Transparent
  bg, `--border` border, `--muted` text, 11px uppercase,
  letter-spacing 0.06em. Hover lights up to `--fg`. All
  per-component button overrides (admin-button, panel-button,
  pricing-edit button, identity-actions button, …) deleted.
  `button.ghost` is now a legacy alias that means nothing —
  same as base.
- **Two font sizes everywhere:** 13px mono body + 11px uppercase
  chrome. The previous 10/11/12/13/14/16px scatter is gone.
- **`button.danger`** is just a colour swap (`--error`) of the
  base, not a different geometry.

## [0.10.16] - 2026-05-24

Side-panel SSOT + clicking terminal now collapses the whole chat
column + `view` rebrands as `edit` (files always open in the editor).

### Changed (browser app)

- **New `col_side(header, body, extra_class)` template** — the
  SSOT for both files (left) and agent (right) side panels.
  Same structure end-to-end: `[panel-header][panel-body]`,
  same padding, same header treatment, same scroll behavior.
  Files no longer has its own special highlighted container —
  it matches agent exactly.
- **Old `.fs-panel` wrapper deleted.** That's what was giving the
  files column a separately-styled inset box with its own border
  + background while agent column had nothing. Both panels now
  share `.col-side` chrome.
- **Terminal rail collapses the whole chat.** Click `terminal` and
  both the transcript AND the input row disappear — leaving the
  editor (if expanded) to take the whole center column. Was only
  hiding the input box before.
- **`view` rail renamed to `edit`.** The top-center panel is the
  editor. Clicking a file in the file list now opens it directly
  in editable mode (no read-only viewer step). `open_file`
  delegates to `edit_file`.
- **Editor template rebuilt** (`opfs_editor`) — own header with
  file path + save/close, full-height textarea, no nested
  `fs-viewer-wrap`. Reads as a real text editor surface.

## [0.10.15] - 2026-05-24

Follow-up minimalism. Three small things caught in live testing.

### Changed

- **All "ready · …" status writes deleted.** History restore was
  still writing `ready · restored prior session · N messages` —
  caught now (history.rs:55, mod.rs ×2, events.rs ×1). The
  terminal status renders empty until something actually needs
  reporting.
- **Chat box has a container again.** `.terminal-row` gets back
  its border + background + padding so the input reads as a real
  input field. Focus colors the border accent.
- **Files-list hover softened.** Was a full-width background
  highlight; now just colors the row text accent on hover, no
  background fill.
- **Pricing UI removed from the agent card.** User: "i have NO
  idea what the PRICING window does on the AGENT thing." The
  pricing data + payment loop are still wired (`.lh_pricing.json`
  + `chat::run_send` payment gate); just no chrome surface for
  setting / showing it. Comes back when there's a clearer UX.

## [0.10.14] - 2026-05-24

Minimalism pass. Bloat out, structure cleaner, header rebuilt.

### Changed (browser app)

- **Header is a three-zone grid:** `[localharness] [<subdomain>]
  [admin]`. Brand left, subdomain center (just the name — e.g.
  `rty`), admin button top-right. The version tag + verify-pill
  + TBA-pill that used to live in the header are all gone from
  it.
- **Version moves to admin dropdown bottom.** `0.10.14` shows
  in a small uppercase line at the bottom of the admin footer.
- **TBA pill 💰 retired** from the header. The agent's TBA now
  appears only in the agent tab. (No emoji either way.)
- **Owner address moves to the agent tab.** New `owner` row at
  the top of the agent card showing the on-chain owner of this
  subdomain (linked to explorer). Was in the verify pill tooltip
  before — now first-class.
- **Agent tab `coming` section removed** — was AI-slop filler.
- **Terminal stripped to bare prompt.** No placeholder text in
  the textarea (no `message · enter to send · shift+enter for
  newline`), no `ready` baseline status, no `new` button. Just
  `>` + textarea + `→`. Status only shows when there's something
  to say.
- **Send button is now `→`** instead of the word "send".

### Style

- **Zero border-radius across the entire app.** Buttons, inputs,
  cards, panels, pills, code blocks — all squared corners.
  Wholesale `border-radius: 0 !important` rule kills any
  per-component rounding.
- **Custom monochrome scrollbar.** Thin (8px), no rounding, uses
  `--border` for the thumb with a `--bg` "border" to give the
  illusion of inset. Hover bumps to `--muted`. Styled for both
  Chromium (`::-webkit-scrollbar`) and Firefox (`scrollbar-color`).
- **Uniform 16px panel padding** carried over from 0.10.13.

## [0.10.13] - 2026-05-24

### Fixed

- **Page no longer grows with chat length.** The transcript now
  scrolls internally instead of expanding `main` → expanding `#root`
  → forcing the whole page to grow. Added `min-height: 0` to the
  flex chain (`main.layout` + `.col-chat`) and `overflow: hidden`
  on `main.layout` so the transcript's `overflow-y: auto` actually
  kicks in.

### Changed (browser app)

- **Terminal + view tabs are inset between files and agent
  columns.** Previously the terminal panel + rail sat OUTSIDE the
  five-column row, spanning full width. Now the center `col-chat`
  owns its own vertical stack — `[view-rail][view-panel?]
  [transcript][terminal-panel?][terminal-rail]` — and the files
  + agent rails extend the full viewport height around it. The
  rails frame the center; the center owns its own top/bottom rails.
- **New `view` top rail and panel** mirroring the terminal at the
  bottom. The file viewer no longer lives inside the file
  explorer column — clicking a file in the file list opens it in
  the top-center view panel (auto-expands if collapsed). Click
  the `view` rail to toggle.
- **Terminal styling softer / less boxy.** Removed the top border
  on `.terminal-panel` so the input flows continuously out of the
  transcript above instead of feeling like a separate walled
  surface. "The terminal input is part of the conversation" —
  first pass at this; the input still has its own row but no
  longer reads as a different container.

## [0.10.12] - 2026-05-24

### Changed (browser app)

- **All three rails are now consistent.** Files (left), agent
  (right), and terminal (bottom) all share the same pattern: the
  rail IS a `<button>`, the whole strip is the click target. No
  nested button-inside-div, no special title bar with a minimize
  glyph. Hover lights up the full rail.
- **Terminal rail moved to bottom-most position.** Lives below the
  terminal panel, full-width, mirrors the side-rail visual treatment
  but rotated horizontal. Click anywhere on the rail to toggle the
  panel above. The previous title-bar + `—` toggle pattern is gone.
- **`main` is a flex column now:** `[main-row]` (five-col stretch) +
  `[terminal-panel]` (shown when not collapsed) + `[terminal-rail]`
  (always visible, bottom-most). Matches the "the outermost
  elements ARE the tabs" mental model.

## [0.10.11] - 2026-05-24

Three real bugs + UX cleanup. The agent was returning 400s on
every send — discovered while diagnosing why the user couldn't get
a reply.

### Fixed

- **`gemini-3.5-flash` doesn't exist on the public Gemini API.**
  Was returning 400 Bad Request on every `streamGenerateContent`
  call. Switched `DEFAULT_MODEL` to `gemini-2.5-flash` which the API
  actually serves. Image model swap too:
  `gemini-2.0-flash-exp-image-generation`.
- **Agent had no system instructions.** Bare `with_capabilities` +
  no system prompt meant the model had no priors about the
  localharness environment — prompts like "what is pricing" produced
  blind tool calls. `start_session` now passes a per-agent
  system instruction telling it what subdomain it's running as,
  what the OPFS surface looks like, and that it's talking to its
  owner. Conversational replies should now happen instead of every
  message triggering `list_directory`.
- **Password-field-not-in-form warning** in console silenced —
  wrapped the gemini key input in `<form onsubmit="return false">`.

### Changed (browser app)

- **No global footer.** Removed it entirely. The terminal moved
  out of the footer and now lives inside `col-chat` at the bottom,
  inset between the files (left) and agents (right) columns —
  the user's requested layout.
- **Terminal is collapsible.** New title bar at the top of the
  terminal with a `—` toggle button that flips `terminal-collapsed`
  on `#layout`; CSS hides the input row, leaving just the bar.
  Mirrors the `files` / `agent` collapse pattern.
- **Removed the `new` button.** Conversation reset wasn't earning
  its space in the terminal row. Will come back somewhere more
  appropriate if needed (likely admin dropdown).
- **Terminal margins tightened.** Status line above the input row,
  prompt glyph `>` followed by the textarea, send button on the
  right. Padding 8/12 instead of the previous mismatched stretch.
- **Transcript uses a `::before { flex: 1 }` spacer** to push turns
  to the bottom of the scroll area. Newest message always sits
  directly above the terminal prompt the user is typing in.

## [0.10.10] - 2026-05-24

Major chrome refactor toward the terminal-style AI-OS vision. The
footer becomes the primary input surface (a terminal prompt). A
right-side **agent** column mirrors the left files column, both
collapsible via edge rails. API key moves to admin. Pricing card
absorbed into the new financial column.

### Changed (browser app)

- **Footer is now the terminal.** The footer hosts the prompt
  textarea + send button. `>` glyph prefix. Plain Enter sends;
  Shift+Enter inserts a newline. Status line sits above the
  prompt row. Removed the dummy `feedback` button — too valuable
  a position to spend on something that doesn't do anything yet.
- **Five-column tenant layout:** `[files-rail] [col-fs] [col-chat]
  [col-financial] [agent-rail]`. Rails always visible, panels
  collapse via class flips on `#layout` (no DOM re-render). Right
  rail is labeled "agent".
- **Financial column** ships the agent's ERC-6551 TBA address
  (linked to the explorer), the agent's **$localharness balance**
  (`token_balance_of(tba)`), and (for the owner) inline pricing
  edit; visitors see read-only `<N> $LH/turn`. Plus a "coming"
  section listing the future surface area (allowance, streaming,
  agent-to-agent payments).
- **Chat column is just the transcript** — input region moved out
  to the terminal footer. Transcript hugs the bottom (`margin-top:
  auto`) so newest messages land right above the prompt the user
  is typing into.
- **API key moved to admin dropdown.** Was sitting at the top of
  the chat column; now lives in the admin section alongside reset.
  Pre-fills from sessionStorage + OPFS when admin opens. `run_send`
  reads via a new `read_api_key` fallback chain so a closed admin
  doesn't block sending.
- **Enter sends** in the prompt textarea (Shift+Enter for newline).
  Cmd/Ctrl+Enter still works as before.

### Added (browser app)

- **`templates::financial_card(tba, lh_balance, price_wei, is_owner)`**
- **`templates::terminal_input()`** — the prompt + status surface
  hosted in the footer.
- **`templates::pricing_readonly_line(price_wei)`** — visitor's
  read-only price line inside the financial card.
- **`Action::ToggleFinancial`** — mirrors `ToggleFiles`; flips
  `financial-collapsed` on `#layout`.

### Removed

- **`Action::Feedback`** (and the feedback button it was wired to).
- Old separate `#pricing-slot` in the left column — pricing now
  belongs to the financial column.

### Note on the bigger vision

User flagged the AI-OS direction: agents owning agents (TBA-of-TBA),
subdomain composability without iframes (recursion-limit constraint),
in-app IDE for differentiating subdomains, marketplace subdomain,
$LH token gating with per-user daily allowance, headless agent
API routes. None of that landed in 0.10.10 — it's noted in memory
for the next planning conversation.

## [0.10.9] - 2026-05-24

### Changed (browser app)

- **File panel moved to left side, collapsible via toggle rail.**
  Tenant chrome now lays out as: a narrow vertical `files` rail
  (left, always visible below the header) | the file panel itself
  (left of chat, default expanded) | chat column (right, takes
  remaining space). Clicking the rail toggles a
  `files-collapsed` class on `#layout`; CSS hides the panel
  without re-rendering its DOM, so any open file viewer or
  breadcrumb position survives collapse + expand.
- **Mobile chrome stacks vertically.** Under 900px viewport the
  rail becomes a horizontal strip at the top with the label
  un-rotated, and the file panel sits below it (above chat)
  instead of beside.
- **`Action::ToggleFiles`** — wired to the rail button. Pure DOM
  class flip; no Rust state involved.
- Also re-shifts apex `main.apex-main` padding so it doesn't
  fight the new layout-class rule.

## [0.10.8] - 2026-05-24

Two bugs found by tailing the actual console output during a
verify-failed reproduction.

### Fixed

- **Signer's `source.dyn_into::<Window>()` failed for cross-origin
  parents.** A cross-origin parent shows up in `MessageEvent.source`
  as a `WindowProxy` (opaque proxy), which fails wasm-bindgen's
  strict `instanceof Window` check even though it has a working
  `postMessage`. The signer was erroring out at this dyn-into and
  silently dropping the response — the parent then timed out
  waiting for it. Fix: hold `event.source()` as a generic `JsValue`
  and post the reply via `Reflect.get(source, "postMessage").call(...)`.
- **Noise from incidental message events.** Pages run lots of
  unrelated postMessage chatter (Vercel's lockdown script,
  browser extensions, dev tooling). The signer was extracting
  `source` for every message before checking the type, so each
  third-party message logged a spurious "source is not a Window"
  warning. Fix: early-return for unrecognized `msg_type` BEFORE
  any source/origin work.

Together these mean the verify roundtrip should now actually
complete instead of timing out twice and falling back to "verify
failed".

## [0.10.7] - 2026-05-24

Chrome alignment + a real fix for the verify timeout that 0.10.6
only mitigated. Both surfaced from live testing.

### Fixed

- **Verify timeout** — the apex signer iframe's wasm bundle takes
  longer to compile + install its postMessage listener than the
  previous fixed 500ms sleep allowed for, so the subdomain's
  challenge was posted into a void and timed out. The cold-load
  case hit this consistently. Real fix: `paint_signer` now sends a
  `lh-signer-ready` postMessage to its parent once the listener is
  installed and the wallet is loaded-or-known-absent;
  `signer_iframe_request` gates challenge posting on receiving
  that ping (with a 15s ceiling falling back to post-anyway).
  Eliminates the race entirely instead of guessing at sleep
  durations.

### Changed (browser app)

- **Header + footer content aligns with body content.** Both wrap
  in `.header-inner` / `.footer-inner` boxes with the same
  `max-width: 1180px; padding: 0 24px` as `main`, so the columns
  line up at the same edges. Before, the header's outer padding
  was *additive* and content extended 48px past where body content
  starts.
- **Footer feedback button centered** instead of right-aligned.
  Same height as the header admin button (`padding: 4px 14px`,
  same font-size). Header and footer are now the same physical
  height.
- **Mobile-friendly chrome.** `.header-inner` / `.footer-inner`
  get `flex-wrap: wrap`; the admin button uses `margin-left: auto`
  so it stays right-aligned regardless of how many pills landed
  on the left side, and wraps gracefully when they don't fit on
  one line.

## [0.10.6] - 2026-05-24

UX cleanup pass driven by real-use feedback. SSOT sticky chrome
across every page, verify-fail diagnostics so the next failure
mode is actually inspectable, and a heavy declutter of the
create-agent + pricing surface.

### Changed (browser app)

- **SSOT sticky header + footer.** `site_header` and a new
  `site_footer` template are now used by every chrome variant
  (apex, tenant, unclaimed, signer). Header sticks to the top of
  the viewport at `position: sticky; top: 0`; footer to the
  bottom. Header on tenant pages still carries the verify + TBA
  pills; footer carries a (dummy for now) `feedback` button —
  real channel lands later.
- **Apex no longer shows the wallet address inline.** It moved
  into the header admin dropdown's new "wallet" section so the
  main flow stays focused on the create-agent input.
- **Create-agent form decluttered.** Input is full-width on its
  own row, button under it (`justify-self: start` so it doesn't
  stretch), hint text *under* the button reads "3–32 chars, a–z
  0–9 dash." Placeholder shifted from `name` to `my-agent`.
- **Pricing card hidden for non-owners.** Was always-rendered
  before — now only injected by `kick_verification` when the
  visitor is the verified owner. Visitors see the price in chat
  status messages during send instead of a permanent card.
- **Unclaimed-subdomain page simplified.** Was a wall of explainer
  copy + legacy local-UUID claim option. Now just shows
  `<name>.localharness.xyz` + a single `[claim on apex]` button
  that pre-fills the apex form via `?prefill=`.

### Fixed

- **Verify-fail race condition.** The apex signer's `paint_signer`
  is async; if the subdomain posted its sign challenge before the
  apex wallet had loaded, the signer responded with "no identity"
  and verify failed permanently. Bumped the pre-post sleep from
  200ms → 500ms and added a 1500ms-backoff retry at the
  `verify_owner` level. Race-condition failures should drop to
  near zero.
- **Verify-fail diagnostic visibility.** The failure reason was
  only in the pill's `title` tooltip — invisible to most users.
  Now also written to `dom::set_status` (visible in the status
  area below the input) and `console.warn` for cross-reload
  inspection.

### Added (browser app)

- **`templates::site_footer`** — global sticky footer.
- **`templates::pricing_card`** — full-card variant injected into
  `#pricing-slot` when the visitor is the owner (replaces the
  always-rendered placeholder pattern).
- **`Action::Feedback`** — wired to a no-op + console log for now,
  ready for a real channel later.

## [0.10.5] - 2026-05-24

**$localharness ERC-20 ships.** Replaces 0.10.4's
native-ETH-based BootstrapFaucet (dormant — Tempo Moderato
forbids EOA↔contract native value transfers, so neither the
faucet nor the 0.10.3 payment loop could actually move value).
Everything flows through `LocalharnessToken.transfer` /
`.faucet` from here on. Verified end-to-end on-chain.

### Added (contracts)

- **`contracts/src/LocalharnessToken.sol`** — hand-rolled ERC-20
  (name = symbol = "localharness", 18 decimals). Adds a public
  `faucet(recipient)` that mints `faucetAmount` (default 1000 LH)
  out of thin air, one claim per recipient ever. Owner-only
  `mint(to, amount)` for arbitrary distribution; owner-only
  `setFaucetAmount` + `transferOwnership`. No pre-funding needed —
  the contract mints, doesn't redistribute.
- **`contracts/script/DeployLocalharnessToken.s.sol`** — single
  no-arg deploy.
- **Live deploy on Tempo Moderato:**
  `0xcC8A300658dC8d0648D984A5066Af3F8E75e0936`, owner
  `0x81E9c327…`, faucetAmount 1000 LH. Smoke-tested with a fresh
  address — `faucet()` mints, `balanceOf` reflects.

### Added (Rust SDK)

- **`registry::LOCALHARNESS_TOKEN_ADDRESS`** const (live address).
- **`registry::token_balance_of(holder)`** — ERC-20 `balanceOf` view.
- **`registry::token_faucet_self(signer)`** — calls
  `faucet(signer.address)` on the token. Caller pays gas.
- **`registry::token_transfer(signer, to, amount)`** — calls
  `transfer(to, amount)` on the token. The payment loop's
  substrate now.
- **`registry::rlp_call_unsigned(...)`** + **`registry::rlp_call_signed(...)`**
  — general EIP-155 RLP builders for any legacy tx (with or
  without calldata). The previously-shipped `rlp_native_transfer_*`
  pair are still exported as the no-data convenience case.

### Changed (browser app)

- **Identity creation now mints starter $localharness.** Sequence:
  `tempo_fundAddress` (gas) → poll balance → `token.faucet(self)`
  → done. New wallet ends up with 1000 LH ready to spend on a
  paid agent.
- **Payment loop switched to ERC-20.** `chat::collect_payment_if_required`
  now builds `transfer(tba, price_wei)` calldata, sends it through
  the (extended) iframe signer, and submits. No more
  `rlp_native_transfer` to the TBA — that was a dead path on Tempo.
- **Iframe signer extended to handle contract calls.** `lh-sign-tx`
  payload accepts an optional `data` hex field; empty for native,
  populated for ERC-20-style calls. Same `purpose` logging,
  same auto-approve (consent collected at the subdomain).
- **Pricing UI copy:** "test ETH/turn" → "$localharness/turn".
  Default placeholder shifted from `0.001` to `1.0` (LH tokens
  are denominated in much smaller units than ETH).

### Deprecated

- **`registry::bootstrap_fund_self`** — removed (was unreachable
  anyway; `BOOTSTRAP_FAUCET_ADDRESS` stays at zero for safety).
- **`BootstrapFaucet` contract** at `0xA439…` remains deployed
  but unreferenced. Holds 0 balance. Owner can self-destruct it
  via a future cleanup if desired.

### Tempo Moderato findings (carried into memory)

- The chain rejects EOA→contract and contract→EOA native ETH
  value transfers ("value transfer not allowed"). All economic
  activity must go through ERC-20-style contract calls.
- Every account reads as having a sentinel `4242424242…` wei
  balance via `cast balance` / `eth_getBalance` regardless of
  actual on-chain reality. Don't trust this number for spending
  capacity; only `transfer` reverts ("balance" / "drained") tell
  you what's real.

## [0.10.4] - 2026-05-24

Ultra-minimal apex onboarding pass plus a `BootstrapFaucet` contract
that decouples first-wallet funding from the public testnet faucet.
Also kills every remaining `window.confirm()` in the bundle —
confirmation flows are now HTML-template + inline `data-action`
buttons end to end.

### Changed (browser app)

- **Stepped apex.** The apex page now renders exactly one of two
  screens at a time: no-identity → just `[create identity]` and
  `[import seed]` buttons; identity-exists → owned-agents list +
  `[name].localharness.xyz [create]` form, with a small wallet
  footer at the bottom. No more tagline, no more "Open source · …"
  footer, no more identity+claim panels stacked together.
- **Header strip.** Header shows `localharness 0.10.4`. No "web demo"
  prefix, no `apex` / `tenant · name` tag chip. Admin button moved
  to top-right and opens a dropdown panel.
- **Admin dropdown.** Single home for seed reveal + seed import +
  reset-local-state. Replaces the old footer admin link and the
  identity-sidecar disclosures.
- **`create →` → `create`.** Button label is just the word; no
  arrow glyph.
- **Tenant chrome trim.** No "Streaming Gemini chat…" preamble.
  Inputs use minimal placeholders. "send" / "new" actions only.
  OPFS panel title is just `files`.
- **Wipe-button consent moves inline.** Click `wipe` in the OPFS
  panel → button swaps to `wipe? / no`. Confirm runs the wipe.

### Added (browser app + SDK)

- **`BootstrapFaucet.sol`** — admin-pre-funded distribution contract
  at `contracts/src/BootstrapFaucet.sol`. `fund(address)` callable
  by anyone, one drip per recipient, owner controls drip size +
  withdraw. `contracts/script/DeployBootstrapFaucet.s.sol` deploys
  with `forge script ... --rpc-url tempo_moderato --private-key
  $EVM_PRIVATE_KEY --broadcast`.
- **Auto-funding on identity creation.** `Action::CreateIdentity`
  now: generate wallet → `tempo_fundAddress` (gas drip) → poll
  `eth_getBalance` until non-zero → call `BootstrapFaucet.fund(self)`
  if `BOOTSTRAP_FAUCET_ADDRESS` is set → re-paint. Fixes the
  prior "have 0 want N" error visitors hit when claiming a name
  immediately after creating an identity.
- **`pub fn registry::balance_of(address_hex)`** — `eth_getBalance`
  wrapper.
- **`pub fn registry::wait_for_min_balance(...)`** — poll until
  the address has at least N wei, with 1s cadence + timeout.
- **`pub fn registry::bootstrap_fund_self(signer)`** — sign + send
  + confirm a `BootstrapFaucet.fund(self_address)` call.
- **`pub const registry::BOOTSTRAP_FAUCET_ADDRESS`** — initially
  zero (contract not deployed yet). Update this constant after
  running `DeployBootstrapFaucet.s.sol`; the bundle then activates
  the on-chain top-up automatically.

### Fixed

- **No more JS dialogs.** Every `window.confirm()` is gone:
  - OPFS wipe → inline arm-then-confirm in the panel header.
  - Admin reset → inline `[reset…] → [yes, wipe] / [cancel]` in the
    header admin dropdown.
  - Tx-signing consent → moved to the subdomain side as a
    user-facing pay-card click (the iframe signer auto-signs once
    the subdomain has collected consent; same model as challenges).
- The `agents-list` border-top no longer renders at the top of an
  otherwise-empty section — empty list collapses to display: none.

### Deploy step (manual)

The new `BootstrapFaucet.sol` is **written and compiled but not yet
deployed** — the deploy needs the admin key in env, which only the
operator has. To activate:

```sh
EVM_PRIVATE_KEY=<admin-key> \
forge script script/DeployBootstrapFaucet.s.sol \
  --rpc-url tempo_moderato \
  --root contracts \
  --broadcast \
  --sig "run(uint256,uint256)" \
  10000000000000000 \  # 0.01 ETH per drip
  1000000000000000000  # 1 ETH prefund
```

Take the printed address and update
`src/registry.rs::BOOTSTRAP_FAUCET_ADDRESS`. Rebuild wasm + redeploy
to vercel. Until then, identity creation funds via `tempo_fundAddress`
only.

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
