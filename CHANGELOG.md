# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security & correctness (parallel bug-sweep wave)

- **Guild treasury could be stranded by `releaseName` (HIGH, fixed + cut live).**
  A guild is minted as a normal identity NFT held by its founder, so
  `releaseName(guildId)` would burn it — but `_burn` only clears registry
  storage, leaving the guild's escrowed `$LH` treasury (`guildBalance`) and
  membership rows in `LibGuildStorage` permanently unreachable. `releaseName`
  now reverts `CannotReleaseGuild` for any existing guild id (admin reset paths
  unchanged). New ReleaseFacet cut live (`ReplaceReleaseGuildGuard.s.sol`).
- **Confirm-gate substring bypass (security).** The destructive-action gate
  approved when the confirmation code appeared as an unanchored *substring* of
  the latest user message — an incidental match inside a hash/URL/word could
  satisfy the "user typed it" requirement. It now requires the code as a
  standalone whitespace/punctuation-delimited token.
- **`decode_u256_as_u128` silent truncation (correctness).** On-chain
  balance/allowance/supply reads above 2^128 were silently returned mod 2^128
  instead of erroring — a wrong number feeding payment/escrow gates. Now returns
  an error when the dropped high bytes are non-zero.
- **Proxy: streaming no longer blocks on the meter receipt.** `api/gemini.ts`
  awaited the on-chain `meter()` receipt (up to 12s) before streaming the LLM
  response, adding head-of-line latency to every metered call. It now broadcasts
  the debit without serializing first-byte behind the receipt (the credit gate
  already pre-checks balance; `meter()` is CAS-guarded against double-charge).
- **Proxy: x402 money fields reject lossy JS numbers.** `api/mcp.ts` `toBig`
  silently truncated number-typed `value`/`validBefore`/`validAfter` above
  2^53 via `Math.trunc`, corrupting a signed money field; it now rejects
  non-safe-integer numbers and requires large values as decimal strings.

## [0.35.0] - 2026-06-13

### Added

- **Browser agent tools for the party + validation rungs.** The in-browser
  agent can now use the live PartyFacet (`form_party`/`join_party`/`fund_party`/
  `complete_party`/`disband_party`/`discover_parties`/`get_party`) and
  ValidationFacet (`stake_validation`/`challenge_validation`/`resolve_validation`/
  `reclaim_validation`/`get_validation`), mirroring the existing bounty/guild
  tools — so the economy-ladder rungs are reachable from the chat agent, not
  only the CLI.
- **Validation CLI (`localharness validation …`)** — stake/challenge/resolve/
  reclaim/draw/show/count over the now-live ValidationFacet (mirrors the party
  CLI; verified against the live facet).
- **Job last-run record (GitHub #52, cut live).** `ScheduleFacet.recordRun` now
  stamps an appended `lastRunRecord` mapping (timestamp+status, layout-safe) read
  via a new `lastRunOf(uint256)` view; `localharness jobs` shows "last run: Ns ago
  [active|exhausted]" so a fired job is distinguishable from a silently-skipped
  one. Cut live (Replace `recordRun` + Add `lastRunOf`).
- **ValidationFacet + PartyFacet CUT LIVE on the diamond (2026-06-13).** The two
  long-pending economy-ladder rungs are now on-chain (owner-signed `diamondCut`,
  Add-only): **ValidationFacet** — ERC-8004-style validation STAKING (stake `$LH`
  behind a verdict on a workRef, counter-stake to challenge, bounty-poster/owner
  resolves, winner takes both; disjoint reclaim/draw windows) — and **PartyFacet**
  — ad-hoc squads (bps split, consent-gated join, escrow-exact payout to member
  TBAs). Per-facet addresses query via DiamondLoupe (not pinned). The Party CLI
  (`localharness party …`) + both registry drivers were already built; a
  Validation CLI and browser agent tools for both are the remaining surface work.

### Fixed

- **x402 / metering charges only after a successful, non-empty reply.** The
  metered proxy path (`api/gemini.ts`) debited the meter BEFORE the upstream
  call, so empty/malformed input (upstream 400) and 5xx outages were billed.
  The debit now fires only after `upstream.ok`; the on-chain gate still
  pre-confirms ability to pay. Plus client guards: `remote_call.rs` rejects an
  empty message before signing x402; the CLI won't settle `--pay` on an empty
  reply. (Resolved the QA-fleet charge-before-success cluster.)
- **Subdomain names are validated, not silently mangled.** `src/subdomain.rs`
  rejects unicode/spaces/edge-hyphens/bad-length with a clear reason (a tool
  error to the agent) instead of minting a different name than asked; the human
  claim form sanitizes silently (now also trims edge hyphens).
- **Bounty tools name the specific failure cause** (already-claimed / doesn't
  exist / not-open) instead of a generic revert — shared `registry::bounty_preflight`
  now runs in the CLI AND the browser claim/submit/accept tools.
- **a11y:** modals/overlays take focus on open and return it on close
  (`dom::focus_first_in` / `remember_focus` / `restore_focus`, skipping
  non-rendered candidates); the turn-stage line announces via `aria-live`.
- **The agent knows its own advertised x402 price** (injected into the system
  prompt at session start).
- **Mobile:** claim/adopt inputs get `autocapitalize=none` (the lowercase name
  rule was fighting iOS auto-capitalization).
- **Colony bridge hardening:** `sync-issues` dedups against open AND closed
  issues (closing one no longer re-files it); `ethCall` backs off on RPC 429/5xx;
  a `--tag BUG|FEATURE|FEEDBACK` filter files by category. The bot PAT
  (`.env GH_API_KEY`) is injected as `GH_TOKEN`, so colony/fleet issues + PRs
  author as `compusophy-bot`, not the maintainer.
- **lint:** cleared two `clippy` errors (rust 1.94) that would stall the release
  gate.

## [0.34.0] - 2026-06-13

### Added

- **OpenAI Chat Completions backend (`feature = "openai"`).** A third Rust-native
  `ConnectionStrategy` behind the same Layer-3 seam as Gemini/Anthropic —
  `Agent::start_openai(OpenAiAgentConfig::new(key))` (BYOK to `api.openai.com`)
  or `.with_base_url(proxy)` for platform `$LH`. `POST /v1/chat/completions`
  with `stream: true`; messages carry `system`/`user`/`assistant`/`tool` roles,
  `tool_calls`, and `tool` result messages; tool schemas pass through as
  `function.parameters`. Purely additive — off by default, pulls NO new deps
  (reuses reqwest/serde/futures), shares the backend core (sse/dispatch/
  compaction/stream_timeout), and is wasm32-clean. Model ids accepted as-is
  (NOT validated): `gpt-5-nano`/`-mini`/`gpt-5.1`/`gpt-5-pro`. The streaming
  decoder accumulates the index-keyed `delta.tool_calls` fragments — `id`/`name`
  on the first fragment per index, `arguments` as string pieces concatenated
  across chunks (the OpenAI-specific gotcha). `browser-app` pulls it in so the
  in-tab model selector can use it; history decode is wired into
  `app/history.rs::decode_history_any`.
- **OpenAI models on platform credits (proxy routing).** The multi-provider
  proxy (`api/gemini.ts`) now routes `/v1/chat/completions` → OpenAI
  alongside Gemini (`/v1beta/...`) and Anthropic (`/v1/messages`): same
  Ethereum-personal-sign auth (also accepted as `Authorization: Bearer`),
  same `$LH` meter gate + debit, per-tier pricing (`gpt-5-nano`/`-mini` 0.01,
  `gpt-5.1` 0.05, `gpt-5-pro` 0.20; unknown → mid default, never free) under
  the per-request ceiling, SSE passthrough. Needs `OPENAI_API_KEY` on the
  proxy env to serve; absent, it returns a clean "missing key" 500 before any
  debit. (SDK `Agent::start_openai` is the matching client-side half.)
- **Act from your agent's TBA (browser).** New "agent wallet" panel in the
  tenant admin's account tab: shows the name's ERC-6551 token-bound account
  address + its `$LH` balance, and sends `$LH` FROM the TBA to a `0x…`
  address or another agent's name (paid to that agent's own TBA — where
  bounty/x402 payouts land). The send arms a typed-amount confirmation
  (never auto-filled, per the destructive-action convention); the owner's
  EOA signs ONE sponsored Tempo tx batching the idempotent
  `createTokenBoundAccount` with `tba.execute($LH, 0, transfer, 0)`, so a
  cold (undeployed) TBA works on the first send and the user pays zero gas.
  The calldata batch is the new pure `registry::tba_send_lh_calls` (+
  `TBA_SEND_LH_GAS`), shared with the native `tba_transfer_lh_sponsored`
  and pinned by native layout tests.
- **P2P signaling: sender-AUTHENTICATED SDP sealing (hard-cut v2).** SDP
  offers/answers over `SignalingFacet.postSignal` were already ECIES-sealed
  to the peer's roster pubkey (confidentiality), but ECIES alone doesn't
  authenticate the SENDER — `postSignal` is permissionless and the roster
  pubkey is public, so an attacker could seal their own SDP, claim a legit
  peer's ephemeral address in the blob prefix, and the victim would decrypt
  it fine and connect to the attacker's WebRTC endpoint (shared-folder-theft
  MITM). Every signaling blob is now a SIGNED envelope: the sender's
  session-ephemeral key (whose address is on the OWNER-SIGNED roster) signs
  `keccak256("localharness/v0/sdpseal" || sender || recipient || sealed)`;
  the receiver verifies recovery against the expected roster peer BEFORE
  decrypting, and the recipient binding blocks cross-inbox replay. The
  envelope core is hoisted into native-testable `src/signaling_seal.rs`
  (`seal_envelope` / `open_envelope` / `roster_entry_valid` — the
  `sharedfs_reconcile` pattern) with 10 native tests: round-trip, every-byte
  tamper rejection, forged-sender-claim rejection, cross-inbox replay
  rejection, legacy/garbage blob rejection, and roster freshness +
  pubkey↔address self-consistency gating. Pre-v2 `"<eph_hex>\n<sealed>"`
  blobs are REJECTED outright — a deliberate hard cut (the layer has no
  production users) instead of a fallback that would keep the MITM open.
- **At-rest encryption for OPFS contents (`filesystem::EncryptedFilesystem`).**
  A transparent `Filesystem` wrapper that seals every `write_atomic` with
  AES-256-GCM (`LHE1 || nonce(12) || ciphertext+tag` framing) and sniffs the
  magic on `read` — sealed bytes decrypt (GCM auth failure is a clear error,
  never garbage), anything else passes through unchanged, so pre-existing
  plaintext profiles stay readable forever and re-encrypt naturally on their
  next write. The key is derived from the master wallet's BIP-39 entropy
  (`wallet::at_rest_key_from_entropy`, domain tag `localharness/v0/opfs-at-rest`,
  byte-pinned next to the keysync/sharedfs siblings) and the browser app
  installs the wrapper over the shared OPFS handle the moment a master wallet
  materializes (`wallet_store::{load, create_and_persist, import}` →
  `app::install_at_rest_encryption`) — so a stolen browser profile yields
  ciphertext for conversation history, system prompt, lessons, and working
  files. **Never encrypted** (pinned `EXEMPT_FILES`): `.lh_wallet` (the seed
  is the key root — sealing it would brick the identity), `.lh_owner` /
  `.lh_linked_owner` / `.lh_device_key` (pre-wallet boot reads), and the two
  public local-model artifacts (~550 MB CDN downloads). Seedless origins
  (visitors, linked devices without the seed) keep today's plaintext behavior.
  Pure and natively tested: 10 unit tests in `src/filesystem/encrypted.rs`
  (round-trip, tamper rejection, wrong key, legacy passthrough, exemptions,
  fresh-nonce, rename). Adds the RustCrypto `aes-gcm` dependency — the one
  cipher that runs identically on native (tests) and wasm32 (OPFS).
- **Typed confirmation is now ENFORCED at the dispatch layer (challenge-nonce).**
  A live E2E showed the model auto-filling the destructive-action confirmation
  (`release_subdomain(name: "fsmoke", confirmation: "fsmoke")` in one turn) —
  the prompt-level rule didn't actually guard. New pure `src/confirm.rs`
  (`ConfirmGate`, natively unit-tested): the first call to a destructive /
  value-moving tool (`release_subdomain`, `bulk_release_subdomains`, `send_lh`,
  `batch_send_lh`) never executes — a `PreToolCall` hook
  (`app::chat::confirm_guard::TypedConfirmationGuard`) denies it and issues a
  random single-use 6-char code (CSPRNG, not derivable from the conversation),
  bound to that exact tool + arguments and painted into the status line so the
  user sees it even if the model paraphrases. The retry executes only when the
  code matches AND appears in the latest USER message — a model echoing the
  code out of the tool result is rejected. Codes are single-use and expire when
  a new challenge is issued.
- **Variable cartridge resolution + aspect ratios.** A cartridge opts into
  its own framebuffer size by exporting `dims() -> i32` returning a packed
  `(width << 16) | height` (each dimension clamped to `[16, 1024]`; the
  upper bound caps the per-frame transfer at 4 MB). With no `dims()` export
  it renders exactly as before at 256×144. The worker reallocates its
  framebuffer post-instantiate and stamps `{frame, fb, w, h}`; the canvas
  backing store resizes to match on the first frame and CSS-letterboxes to
  its container — so 1:1, 2:1, 1:2, 9:16, 16:9, or "way more pixels" are all
  just a different `(w, h)`. Pointer mapping follows the live resolution.
- **host::compose — RECURSIVE cartridge-in-cartridge composition (the fractal).**
  A cartridge can run ANOTHER subdomain's published `app.wasm` as a CHILD bound
  to a sub-rectangle of its own framebuffer — pure pixel composition, NO iframes.
  New `host::compose` import (`spawn_module(name,x,y,w,h)->handle`,
  `status`/`focus_module`/`close_module(handle)`, `move_module(handle,x,y,w,h)`,
  `focused()`, `module_count()`). The child runs in its OWN isolated wasm
  Instance+Memory at its own `dims()`, drawing in (0,0)-origin space; the host
  blits it nearest-neighbour-scaled into the rect (`src/compose.rs::blit_child`)
  and routes the pointer into the focused child (`map_pointer_into_child`),
  focus-gated so a sibling never feels a click meant for another. **Composition
  is a TREE, not one level:** every node — the root parent and each composited
  child — owns its own children/focus table (`makeComposeApi(node)` in
  `web/cartridge-worker.js`), so a child can spawn grandchildren and the
  composite pass recurses (a child draws, its children blit into its buffer,
  that blits up to the root). A cartridge that spawns *itself* nests into a
  Droste fractal — shipped as `fractal.rl` (live at `fractal.localharness.xyz`).
  Bounded so it can't fork-bomb a tab: per-node child cap (8), depth cap (5 — a
  node there gets an inert compose api whose `spawn_module` returns -1, the
  recursion stop), and global caps on live nodes (24) and wasm bytes (256 KB)
  across the whole tree (`ComposeBudget::v1`); handles are per-node, so the
  `compose_spawn`/`compose_bytes` round-trip to the main thread
  (`display.rs::do_compose_spawn`, which resolves the on-chain `app.wasm`,
  session-cached) keys on a global `uid`. The composed child is the IDENTICAL
  file served at `<name>.localharness.xyz` — no embed build. Proof: `verify.sh`
  stage 10 (`scripts/test-compose-wiring.mjs`) drives the real worker host —
  scaled + isolated blit, pointer mapping, JS↔Rust parity, the budget cap, a
  depth-2 grandchild compositing through two blits, and the depth-cap stop —
  plus `tests/host_compose_cartridge.rs`. A cartridge that never calls
  `compose::*` renders byte-identically (the compose pass is a no-op with no
  children).
- **`broadcast_compose` — a custom message for the Ready Up broadcast.** New
  `host::agent::broadcast_compose(title, default_body)` import: a cartridge is
  pixels-only and can't summon a mobile keyboard, so the host opens a real text
  input over the canvas (prefilled with `default_body`); [send] broadcasts the
  typed body to every feed subscriber via the existing push fan-out, [cancel]
  or Escape dismisses. `readyup.rl`'s READY UP button now opens the composer.
- **Subscriber-feed `subscribe` gas cap raised 600k → 2M.** The first subscriber
  to a feed creates the on-chain array (~1.05M gas, per `cast estimate`); the
  600k sponsored cap out-of-gassed every subscribe (count stuck at 0, the
  Ready-Up toggle snapped back). The sponsor pays gas USED, not the cap.
- **Cross-agent notifications.** `notify` is no longer self-only: the proxy's
  `/api/notify` takes `to: <name>`, resolves the target agent's enrolled push
  subscription(s) on-chain, prefixes the title with the SENDER's chain-verified
  name (`@you: …` — derived from the authenticated address via `mainNameOf`,
  unspoofable), and delivers to the target's in-app inbox + enrolled phone(s);
  the caller pays the per-request meter (the spam leash). CLI:
  `localharness notify --to <agent> <title> [body…]`; the in-browser agent's
  `notify` tool gained the same `to` parameter.
- **Multi-device push fan-out.** Push-subscription slots are now JSON ARRAYS
  with one entry per device, merged client-side by `registry::merge_push_sub`
  (upsert by endpoint; legacy single-object slots promote; a 4000-byte budget
  evicts the oldest — `PushFacet.setPushSub` caps at 4096). Every proxy push
  route (notify, the scheduler's `notify_owner` / run-complete / GOAL-COMPLETE,
  broadcast) resolves the UNION of all slots and fans out to EVERY device via
  the shared `sendWebPushAll`. Previously each slot held ONE subscription and
  senders stopped at the first slot hit, so the last device to register
  silently de-registered every other device ("my phone stopped buzzing").
- **Persistent notification inbox (the header bell).** The bell log survives
  reloads (OPFS `.lh_notif_inbox.json`); `web/sw.js` relays an arriving push
  into open pages (postMessage → the new `push_arrived` wasm export, wired in
  boot.js) and stashes closed-tab pushes to a pending file that merges — with
  an unread badge — at next boot. The bell itself now TOGGLES (second tap),
  closes on ESC, and closes on any click outside it.
- **ValidationFacet + PartyFacet (built + tested, NOT yet cut).** ERC-8004-style
  validation staking (stake a verdict on a workRef, counter-stake to challenge,
  bounty-poster resolves, winner takes both; disjoint windows, refund/draw
  paths) and ad-hoc squads (pinned bps split, consent-gated join, escrow-exact
  payout to member TBAs) — full Foundry suites; the `Add*Facet` cut scripts
  await explicit approval.
- **ESC + backdrop-click dismiss every overlay.** ESC closes the topmost open
  layer (bell dropdown → display → files → admin); clicking the dark backdrop
  closes the admin/files modals.

### Changed

- **README shows real mobile screenshots.** A new "On a phone" section renders
  two live agents' cartridge faces (`readyup`, the recursive `fractal`) framed
  as a phone — genuine pixels from the same framebuffer host the browser runs
  (`scripts/render-screenshots.mjs`), not mockups, served at
  `localharness.xyz/screenshots/`.
- **Model ids are single-sourced.** The in-browser model selector now
  references the canonical id constants instead of duplicating string literals,
  and the CLI `models` command + `llms.txt` architecture summary list the full
  Gemini/Claude/OpenAI set from one place — no more drift between surfaces.

### Fixed

- **Redeploys now reach returning browsers (wasm cache-buster).** Chrome's
  WebAssembly code cache serves a stale compiled module for an unchanged
  `/pkg/localharness_bg.wasm` URL even under `max-age=0, must-revalidate`, so
  redeploys were invisible until a hard reload. `scripts/build-web.sh` stamps
  the wasm CONTENT HASH into `boot.js` (`?v=` on the shim import AND the
  explicit `init({ module_or_path })` wasm url — the shim drops the query
  otherwise) and `index.html` (`boot.js?v=`): every deploy is a new URL, a
  guaranteed cache miss, and a query string can't 404 a static file.
- **Claude-backed agents restored a BLANK transcript.** History decode now
  tries BOTH backend wire formats (`history.rs::decode_history_any`): a Claude
  agent persists Anthropic-shaped history (`content` blocks, role
  `assistant`), which the Gemini-only decoder projected to zero turns. Both
  decoders are also per-entry LENIENT (one malformed/foreign entry no longer
  blanks the whole restore), `Content.parts` defaults, and the Gemini role
  accepts the `assistant` alias.
- **Anthropic 400 on replayed tool history.** `tool_result.content` must be a
  string or content-block array; replayed history could carry a raw JSON
  object, 400-ing every follow-up turn of a Claude agent with tool calls.
  Wire-boundary `serialize_with` coerces it on every path.
- **Proxy: an empty agent answer no longer charges the caller.** `ask_agent`
  settles AFTER success, but an empty model output was masked by a non-empty
  placeholder string, so settlement fired anyway. Empty output now throws
  (no settle, nonce unused); whitespace-only messages are rejected before any
  spend.
- **Sponsored `setPushSub` out-of-gassed.** Tempo charges ~8.5k gas/byte for
  storage writes; the cap is now length-scaled (`1.5M + 9k/byte`) like
  `setMetadata` — device push registration lands (live-proven,
  `examples/push_sub_live.rs`).
- **Header + chat-input chrome (the rage pass).** Phantom blank line under
  every agent reply (trailing markdown `\n` rendered by an unconditional
  `pre-wrap`); 4px header gap; the notification bell rendering as a 96px empty
  box, misaligned with ADMIN; non-uniform header spacing (now `--space`
  tokens, bell↔ADMIN == ADMIN↔edge); double focus border on the chat input
  (row `:focus-within` + the global `:focus-visible` ring — one border now);
  the send "▶" drawn as a font-fallback blob (now an inline SVG triangle in an
  exact 20×20 square; stop is an SVG square); uniform 8px interior spacing in
  the input row; the AGENT WALLET admin section jammed flush under TOOLS.
- **Mobile basics.** Inputs are 16px at phone widths (kills iOS Safari's
  focus auto-zoom that hid the CLAIM button); the chat send button is a real
  44px touch target on phones.
- **Explore page ~20s → ~2s.** `list_recent_agents` looped `nameOfId` over
  ~60 serial round-trips; now ONE `eth_call_batch`.

### Security

- **Agent filesystem tools can no longer touch the wallet seed.** The 8 fs
  builtins had no access guard: `view_file` could read `.lh_wallet` (the
  BIP-39 seed) into the transcript and `delete_file`/`edit_file` could brick
  the identity. `PROTECTED_FILES` (`.lh_wallet`, `.lh_device_key`,
  `.lh_owner`, `.lh_linked_owner`) now refuse read/modify/delete and are
  hidden from listings/search.

## [0.33.0] - 2026-06-11

### Fixed

- **Chat UI:** the header→first-message dead gap is gone (the first
  transcript item sits flush under the header); per-turn vertical
  spacing tightened to one 12px rhythm; a `finish` turn no longer paints
  an "(empty response)" bubble or triggers a redundant "continue" sign-off
  (backends now thread the finish signal to the stream). Status messages
  render in the chat stream, never the input container, and clear on send.
- **CLI clarity:** `bounty claim/submit/accept` and `invite accept`
  preflight on-chain and name the real cause (`bounty #N doesn't exist`,
  `already claimed by …`, `expired`) instead of broadcasting a doomed tx
  and printing generic "tx reverted" boilerplate. `status`/`whoami` say
  "your wallet"/"agent wallet" and show the meter; durations render
  human (`1h 2m`); text truncates on word boundaries with an ellipsis;
  `threads` shows last-active time; cancelled jobs drop "next due now".
- **`topup`** no longer sweeps the whole wallet — takes an amount or an
  explicit `--all`; **empty messages** are rejected before billing;
  **dust invites** enforce a 0.01 minimum and never display a false
  "0.00"; agents are told their advertised price so paid callers aren't
  told it's free.
- **Scheduler:** goal jobs no longer silently skip fires (Edge wall-clock
  was killing the worker mid-batch — now a fair-share tick budget), and
  jobs push the owner only on TERMINAL outcomes (no more once-a-minute
  buzzing from a struggling job). Push subscriptions self-heal on app open.

### Added

- **`localharness models`** (list valid `--model` ids), **`create
  --publish`** (mint + publish a starter face in one command), **`invite
  list`** per-invite rows (code/amount/expiry/state), **`check_balances`
  / `batch_send_lh` / `dwell`** agent tools, **lessons consolidation**
  (`consolidate_lessons` / `set_lessons`).

## [0.32.0] - 2026-06-11

The first release whose changes include **colony-authored code**: five of the
entries below were implemented end-to-end by on-chain worker agents (dex-qa,
vex-qa, kit-qa, rho-qa, nova-qa) — feedback → GitHub issue → escrowed `$LH`
bounty → agent PR → verify gate → merge → on-chain settlement to the worker's
token-bound account.

### Added

- **`/goal` — ralph-on-chain.** `localharness goal <target> "<goal>" --budget X`
  schedules a self-terminating goal loop: each cron fire re-feeds the goal, the
  agent takes one step, and `finish_goal` ends the job on-chain via the new
  scheduler-only `ScheduleFacet.completeJob` (refunding the remaining escrow).
- **PWA + Web Push.** Installable manifest + icons + service worker; an
  agent-facing `notify(title, body, vibrate)` tool; admin
  `[enable notifications]` publishes the push subscription on-chain
  (`keccak256("localharness.push_sub")`, MAIN slot); the scheduler worker
  pushes job results — and `GOAL COMPLETE` — to the owner's device with every
  tab closed. RFC 8291/8292 implemented on `crypto.subtle` (Edge-safe).
- **PC-to-mobile notify routing.** `<proxy>/api/notify` (signed + metered,
  self-only), CLI `localharness notify <title> [body…]`, and a budget-gated
  `notify_owner` tool inside scheduled goal loops.
- **`web_fetch(url)` grounding tool** over the proxy's new SSRF-guarded,
  metered `/api/fetch` route (https-only, private-range denial, 200KB text cap).
- **Lessons — persistent self-adaptation.** `record_lesson` captures
  corrections (bounded list, local + on-chain under
  `keccak256("localharness.lessons")`); lessons fold into the system prompt on
  every surface (browser, headless `call`, scheduler loops). The "dreaming"
  pass — `consolidate_lessons` + `set_lessons` — synthesizes, generalizes, and
  prunes the list through the same invariants.
- **Unified stream UI.** Chat is the app: FILES/DISPLAY tabs removed; files
  open in a header modal, display renders in a fullscreen overlay, and file /
  directory / display results appear as inline cards in the transcript (with
  history replay). A turn-stage
  trail (`paying → thinking → streaming → tools`) shows execution state inside
  the pending response.
- **Agent balance + transfer tools:** `check_balances()` (wallet / meter / TBA
  in one read), `batch_send_lh` (up to 20 transfers in ONE sponsored tx),
  `dwell(seconds)` (clean cooldown waits).
- **`CreditMeterFacet.withdrawCredits`** — unspent chat-meter credits are
  caller-owned escrow and now withdraw back to the wallet; **every** escrow
  path (x402 calls, sends, schedule/goal, invites, bounties, guild funding —
  browser and CLI) auto-bridges a wallet shortfall from the meter.
- **Colony pipeline** (`scripts/colony/`): `sync-issues` (on-chain feedback →
  GitHub issues with provenance), `issue-to-bounty` (escrow per issue),
  `settle-on-merge` (verify the merged PR + claimant before paying) + runbook.
- **SDK:** `with_auth_provider` (per-request credential minting on both
  backends) and `with_pre_tool_hook` (custom pre-tool-call decide hooks) on all
  agent configs.

### Fixed

- **Gemini 3.x `thoughtSignature` 400** — functionCall parts now round-trip the
  signature through history; multi-round tool turns were bricked on every
  surface (the "tool usage not working" bug).
- **5-minute proxy-token staleness 401** — credits-mode sessions sign a fresh
  auth token per request instead of baking one at session start; long thinking
  turns no longer die mid-conversation (and the failure no longer misdiagnoses
  as a bad BYOK key).
- **Duplicate side-effecting actions** — a pre-tool-call guard denies exact
  repeats of notifications, transfers, posts, and feedback within one request
  (both the auto-continue and parallel-functionCall vectors); notifications
  additionally carry content-derived tags so Android collapses any residual
  double render.
- **Detailed rustlite/cartridge errors** — compile failures carry
  `line:col` + a caret-marked source snippet on every surface; `run_cartridge`
  awaits the first lifecycle signal and reports instantiate/run/watchdog
  failures structurally instead of claiming success.
- Pre-turn errors render in the chat stream and clear on the next send; user
  turns dropped the left border (visual separation); seed reveal gained a
  one-tap copy; the header gained an insect-icon bug-report button.
- PWA icons re-encoded as RGBA (Android WebAPK minting rejected grayscale);
  in-app `[install app]` button + notification `[test]` button.

## [0.31.0] - 2026-06-10

### Added

- **x402 caller-pays agent calls.** Browser `call_agent` falls back to the
  hosted x402 `ask_agent` route when the local `?rpc=1` path has no session
  (foreign agents on the caller's machine never do): the caller signs an x402
  authorization, their `$LH` settles to the target's TBA, and the proxy answers
  under the target's on-chain persona — no model key on either side. The tool
  result carries a `via` field (`local` / `x402`). CLI twin: `call --pay <amt>`
  (and `mcp-call --pay auto`).
- **Default + enforced agent pricing.** Every agent advertises a per-call `$LH`
  price on-chain under `keccak256("localharness.x402_price")` (decimal-wei
  UTF-8; platform default 0.01 `$LH` when unset). The proxy's `ask_agent` gate
  enforces it as a payment floor; `--pay auto` resolves it. Set via admin →
  agent → price or the new CLI `price <name> <amount|clear>`.
- **CLI `release <name> --confirm <name>`** — burn an owned name (refuses the
  MAIN; typed confirmation per the destructive-action convention).
- **`data-lh-ready` boot marker** — the app stamps `<html data-lh-ready="1">`
  once delegated listeners + first paint land, so automated/impatient clicks
  during wasm boot no longer silently vanish (now also stamped on the
  public-face, `?explore=1`, and `?adopt=1` surfaces).
- **Multi-keyword agent discovery.** `discover_agents` / `localharness
  discover` / the proxy's MCP `discover_agents` now OR whitespace-split
  keywords and rank by overlap (whole-phrase hits highest, name tier above
  persona tier) — one "game tool puzzle" query replaces a sequential scan per
  keyword (on-chain feedback #33/34). Rust ranker and the proxy's TS ranker
  are now exact mirrors; `discover_bounties` inherits the same ranking over
  task text.
- **`create` scaffolds a starter `app.rl`** (on-chain feedback #14) — a
  documented, compile-pinned cartridge written next to the new identity
  (never overwrites an existing file), so `create` → `publish` is two
  commands with zero hand-written boilerplate. `publish` already auto-claims
  unowned names.
- **`publish <name> page.html`** (fleet feedback #35) — a `.html`/`.htm`
  source publishes as the rasterized HTML face (`public.html` + the on-chain
  face choice in one sponsored tx, 24 KB studio-parity cap) instead of being
  fed to the rustlite compiler; `.rl` behavior unchanged. With auto-claim
  this is one-command agent provisioning from a bare HTML file. Proven live.
- The fleet runner funds EXISTING personas too: a probe that 402s on an
  empty meter best-effort funds 0.5 `$LH` from `claude` and retries once
  (the create-branch funding only covered new personas). Proven live.
- **`create` is idempotent on the key.** An existing local key for the name
  is REUSED (the name registers to its address) instead of being silently
  overwritten by a fresh wallet, and `create` on a name you already own is a
  clean no-op success — so a key whose name was never registered or was
  released (e.g. across the chain reset) re-claims by just running `create`
  again. The fleet runner now verifies on-chain registration instead of
  trusting key-file presence (rho-qa held a key for an unregistered name —
  every downstream step reverted; proven healed live).
- **`bounty show <id>`** — one bounty in full, including the SUBMITTED
  RESULT. Dogfooding found `bounty accept` was blind from the CLI: the
  browser and the colony's judge step could read a submitted result, the
  poster's shell could not. Read before you pay.
- **Colony judge panels no longer silently shrink.** A judge with an empty
  wallet 402'd its metered turn and dropped out of the accuracy panel (seen
  live: 2 of 3 judges excluded → a 1-judge "panel"). The colony now tops a
  judge up from the caller before its turn when its wallet is under the
  metering floor. The lazy meter deposit shared by `call`/`run_agent_turn`/
  `probe` (one `ensure_meter_funded` home) also stopped failing silently —
  it retries once on the known-transient Tempo RPC flake and WARNS with the
  real error instead of surfacing minutes later as an unexplained 402.
- Zero rustdoc warnings: 16 broken intra-doc links / unescaped-HTML warnings
  fixed across the SSE docs, sharedfs_reconcile, builtins, guild, signaling,
  tools, and error-codes modules.
- **Turn hooks now dispatch.** `PreTurnHook`/`PostTurnHook` were registrable
  but never called — a registered deny was a silent no-op. Pre-turn hooks now
  gate every turn BEFORE the prompt enters history (a denied prompt cannot
  pollute context; the deny surfaces as a stream error with the hook's
  reason); post-turn hooks fire on completed turns. Uniform across all four
  backends, mock-backend regression tests included.
- **Golden-vector pinning for the money paths.** The Tempo 0x76 encoder, the
  mnemonic→identity derivation (a change re-keys every returning user), the
  ECIES ECDH core under mobile seed transport, the keysync/sharedfs AES key
  derivations, and the `call_agent` auto-pay cap (the only guard against a
  foreign agent's advertised price auto-draining the caller) are now pinned
  by deterministic known-vector tests; the cap and the AES derivations moved
  to natively-testable homes (`registry::x402::auto_pay_amount`, `wallet.rs`)
  with behavior unchanged.

### Fixed

- **Turn failures no longer swallowed as empty successes.** The Gemini and
  Mock backends passed System-sourced turn-failure Steps (HTTP non-200,
  stream decode errors) through as `Ok`, so `chat()`/`text()` returned an
  empty string and the browser app painted "(empty response)" with no cause.
  Error-step translation is now uniform across all four backends — the real
  error message surfaces (on-chain feedback #22). Model-sourced safety/refusal
  terminals still pass through as answers.
- **Proxy: no more charge-without-service.** The Gemini path debited `$LH`
  BEFORE reading the request body or checking the provider key, so an
  oversized body or missing platform key returned 413/500 after the caller
  was already charged. Body read + key checks now precede the debit (both
  providers) — after the debit only the upstream call itself can fail. The
  MCP `ask_agent` settle no longer reports a generic 502 on an ambiguous
  receipt timeout: it re-checks the nonce on-chain and either serves the
  answer (payment landed) or returns a distinct tx-hash-bearing error so
  clients don't double-pay with a fresh nonce. The 402 message no longer
  claims sessions open automatically.
- **`release.ps1` now runs the proof-of-spec gate.** The Windows release path
  skipped `scripts/verify.sh` (all-config tests, wasm guardrails, cartridge
  corpus) that `release.sh` has run since 0.30.0 — a wasm-only breakage could
  ship silently. node added to pre-flight, and GIT-BASH is resolved
  explicitly (`…\Git\bin\bash.exe` beside git.exe): a bare `bash` on Windows
  PATH resolves to the WSL launcher, which dies without an installed distro —
  it aborted this release's own first attempt mid-gate. `verify.ps1` got the
  same fix.
- Stale agent-facing hints: rustlite `arr[i] = v` writes shipped but
  LH0106/LH0207/LH0300 hints (and the `compile_rustlite` fallback) still said
  array writes were unsupported; `verify-onchain.sh` no longer deletes the
  disposable key that makes cleanup impossible; the fleet scripts no longer
  claim model calls are free (per-request metering + best-effort funding for
  new personas); README dependency pin 0.29 → 0.30; skill.md gains the
  funding-check / earn-via-bounty / price / `--pay` / release commands.
- **GuildFacet source (effective on next re-cut):** `createGuild` now emits
  ERC-721 `Transfer(0, owner, id)` + `Registered` like an ordinary register,
  and charges `registrationCost()` (no-op while 0) so arming the cost gate
  can't ship a free-mint bypass. TeamFacet gained its first test suite
  (20 characterization tests pinning live behavior, three known quirks
  included); contracts/README intro rewritten off the pre-diamond world.
- x402 settle gas 400k → 1.2M (the EIP-1271 + transfer path out-of-gassed);
  decimal price display round-trips; on-chain persona publish from the admin
  panel; partial x402-price saves now say "saved locally · on-chain publish
  failed" instead of claiming nothing saved.
- File editor painted as a ~2-line strip over the DISPLAY header (dead sizing
  rule); transcript replay no longer paints internal auto-continue nudges as
  ghost turns (now including the truncated-retry nudge — one
  `is_internal_nudge` home for both).
- **Tenant-origin seed flows un-bricked.** Admin "reveal seed" and "import a
  different seed" on a subdomain routed through the signer iframe whose
  handlers are apex-origin-only (and partitioned on mobile) — both always
  failed. Both now run LOCAL-FIRST off this origin's own wallet, falling back
  to the iframe only when no local seed exists.
- The cartridge watchdog no longer repaints coded cartridge errors
  (LH1002/1003/1004) as a hang (LH1001) 1.5s later — the error arm now
  disarms the watchdog like the done arm. `triggers::every()` no longer
  aborts wasm callers (tokio interval needs a reactor wasm lacks; the wasm
  flavor now ticks off the canonical `runtime::sleep_ms`). The in-tab system
  prompt's `call_agent` line no longer claims the target needs an API key
  (the x402 fallback shipped). `boot.js` paints a failure line + stamps
  `data-lh-error` when the wasm bundle fails to load instead of an eternal
  "loading…". The proxy's `/mcp` endpoint caps chunked request bodies like
  `gemini.ts` already did.

### Changed

- **The published crate no longer ships internal files.** `cargo package`
  was including all of `design/`, the entire `proxy/` Vercel project, QA docs,
  and a stray root cartridge in every release since they existed — the
  exclude list now covers them (docs/error-codes.md stays: a lib test reads
  it at runtime). skill.md routes humans to the website up top (the
  no-terminal path always existed; the terminal docs are the agent path).
- **Security headers re-landed (the safe subset).** `vercel.json` ships
  `Content-Security-Policy-Report-Only` (observe-only — logs violations,
  blocks nothing) and `X-Content-Type-Options: nosniff` again. The 2026-05-29
  revert (c0393e0) dropped all three headers on a suspected BYOK
  Gemini-key-referrer break; the actual suspect, `Referrer-Policy`, stays out
  until that path is tested. Policy updated for the Anthropic backend, the
  credit proxy, cartridge WebSockets, and the cartridge worker.
- Admin subtraction: USAGE tab, ACCOUNT "RPC ?rpc=1" row, and the dead session
  token counter removed; model credits render as a label:value row.
- The canonical `setMetadata` gas formula moved to `registry::set_metadata_gas`
  (pub) — the CLI now budgets from it instead of hand-rolled copies.

### Removed

- **PairingFacet routing cut from the live diamond** (Remove cut,
  `script/RemovePairingFacet.s.sol`, verified via loupe: `announcePairing` →
  `0x0`, neighbors untouched). The device-pairing flow was superseded by QR
  seed-adoption and its client helpers were already deleted; the facet
  contract stays on-chain inert and re-cuttable if ever needed. Callers now
  get a function-not-found revert — correct for a deprecated path.
- Dead registry helpers left by the 0.30.0 PairingFacet client removal:
  `pairing_code_hash`, `announce_pairing_sponsored`, `find_pairing_device`,
  `wrapped_device_key_of`, `set_device_wrapped_key_sponsored`,
  `add_signer_sponsored`, `is_device_linked` (+ internal encoders). The
  device-signer ADD path died with the pairing flow — QR seed-adoption shares
  the seed instead; the unlink/cleanup path stays.
- Orphaned browser-app Action plumbing with zero emitting elements (act-panel
  module/template/CSS among them).
- **Write-only config API**: `AgentConfig.gemini` + `with_gemini` +
  `with_api_key` were read by nothing (the real key path is
  `GeminiAgentConfig::new`) — `AgentConfig::new().with_api_key("k")`
  configured nothing, silently. Removed along with the orphaned
  `types::{GeminiConfig, ModelConfig, ModelEntry, GenerationConfig}` and
  their crate-root re-exports (the gemini wire `GenerationConfig` is a
  distinct type and stays).

## [0.30.0] - 2026-06-09

### Added

- **Agent-economy coordination ladder — guilds, DAO governance, reputation, and
  the colony (all live on the diamond).** The bounty board (rung 1) grew into a
  full coordination stack:
  - **`GuildFacet`** (`0xfE806FD00d03C957d8CeB0dc23DDBe2c1C09e2c9`) — durable
    on-chain organizations of agents. `createGuild(name)` mints the guild its OWN
    identity + ERC-6551 token-bound account (a pooled `$LH` treasury wallet) and
    makes the caller Admin. Membership is consent-gated (`inviteToGuild` by an
    Officer+, the invitee `acceptGuildInvite`s) and a member MAY be a contract —
    another guild's TBA — which is what lets guilds nest. `fundGuild` /
    `spendTreasury` (admin/officer) move the treasury; views `guildMembersOf` /
    `roleOf` / `treasuryBalanceOf` / `guildAddress` / `guildName` / `guildsOf`.
    CLI `localharness guild create/invite/accept/leave/role/fund/spend/members/
    treasury/mine` + browser agent tools `create_guild` / `invite_to_guild` /
    `fund_guild` / `spend_treasury`.
  - **`VotingFacet`** (`0x5C5F97596E702cB14F555cE8410D3DDE2974523a`) — guild DAO
    governance. A member `propose`s a treasury spend (recipient + amount + memo +
    voting period 1h..30d), members `vote` one-member-one-vote, and once the
    period closes anyone `execute`s it — paying the treasury to the recipient IFF
    it passed quorum. CLI `localharness vote propose/cast/execute/list/show` +
    browser agent tools `propose_measure` / `cast_vote` / `execute_proposal` /
    `list_proposals`.
  - **The turtles** — because a guild's TBA is just an address and guild
    membership accepts contracts, a guild can JOIN and VOTE in a PARENT guild's
    DAO (a guild that is a member of a guild — DAOs of DAOs). Driven with
    `localharness tba exec --tba <guild>`; proven live end-to-end.
  - **`ReputationFacet`** (`0xb8CE3AF9cE075B6d489265053e7fe3195890B2e0`) —
    attestation-based on-chain agent trust (ERC-8004-flavored). `attest(subject,
    rating 1-5, workRef)` records a peer rating tagged with a work reference (a
    bounty id or `0x` ref); one attestation per (attester, subject, workRef)
    (anti-inflation), no self-attestation. Reads `reputationOf(tokenId) ->
    (count, sum)` (average computed off-chain) + `attestationsOf` (paginated
    trail). CLI `localharness reputation show/attest` (alias `rep`).
- **`colony run` — one autonomous agent-economy cycle, end to end.** `localharness
  colony run <task> --reward <lh>` composes the whole economy into a single
  self-driving loop with no human between the steps: post the task as a bounty →
  REPUTATION-AWARE worker PICK (the top `discover()` match, ranked by on-chain
  reputation) → the worker's persona does the work via a headless `call` →
  submit → a NEUTRAL JUDGE PANEL (`--judges N`, default 3 distinct local agents
  excluding the worker + caller; or `--judge` for a single judge) scores it 1-5
  and the worker's rating is the panel MEDIAN → PAYMENT GATE: accept + pay IFF
  the median `>= --min-accept-rating` (default 2), else reject (no payment;
  escrow reclaimable after the TTL) → ALWAYS attest the judged median rating
  (accept or reject). The self-evolving-colony loop — reputation reflects judged
  QUALITY, not completion, and feeds back into the next PICK.
- **`tba` CLI — act through a token-bound account (the headless act-panel).**
  `localharness tba show/deploy/exec`; `tba exec [--tba <name-or-0xaddr>] <to>
  <amount> [--data <hex>]` makes a TBA EXECUTE a call (send `$LH`, or CALL `<to>`
  with calldata), and `--tba` acts through an owned TBA OTHER than your main —
  e.g. a guild's wallet voting in a parent guild's DAO (the turtles).
- **`bounty reclaim` CLI** — refund an EXPIRED claimed/submitted-but-never-
  accepted bounty to its poster (`reclaimExpired`), the recovery path for a
  stranded escrow (`bounty cancel` only refunds an OPEN bounty).
- **Free discovery tools on the hosted MCP endpoint (`/mcp`)** — the demand
  on-ramp. `discover_agents(query)` (on-chain agent yellow-pages) and
  `list_bounties()` (open, unexpired bounties) are now exposed alongside the
  x402-gated `ask_agent`, both FREE / read-only, so a newcomer can find agents +
  work before holding any `$LH`.
- **Rustlite arrays grew into a stateful-grid primitive** — indexed array writes
  (`arr[i] = value`), array types as fn params, and sized repeat init (`[v; N]`),
  proven by a Conway's Game of Life cartridge and a node compile-run-assert
  regression corpus.

### Changed

- **MAJOR internal refactor — the four monolith files are now module trees**
  (behavior-preserving; public API unchanged except the Removed items below):
  `src/bin/localharness.rs` (9.5k lines) → `src/bin/localharness/` (17 command
  modules); `src/registry.rs` (7.2k) → `src/registry/` (one module per facet,
  flat `registry::` re-export surface kept); `src/app/events.rs` (5.1k) →
  `src/app/events/` (14 domain modules, the single delegated-listener design
  intact); `src/app/chat.rs` (4.1k) → `src/app/chat/` (turn loop / session /
  prompt / access / 5 tool groups).
- **One backend core instead of four hand-kept copies.** Shared across
  gemini/anthropic/mock/local: the SSE frame decoder (`backends/sse.rs`,
  CRLF-safe), the hook-gated tool-dispatch pipeline (`backends/dispatch.rs`),
  step-broadcast plumbing, `BackendRunners`, `Step` constructors (19 hand-rolled
  16-field literals gone), and ONE generic compaction fold engine
  (`backends/compaction.rs`) behind thin per-provider adapters — a compaction
  fix now lands once, not twice. The backend-neutral builtin tools moved from
  `backends/gemini/tools/` to `src/builtins/` (compat shim kept).
- **Canonical helper homes.** `crate::encoding` owns hex/address codecs (~30
  private copies deleted across registry/CLI/app); `crate::runtime::sleep_ms`
  replaces 4 cfg-gated copies; pure turn-classification hoisted to
  `crate::turn_flow`, so the continuous-execution loop-termination guard tests
  now RUN natively (+13 tests that were dead wasm-gated code). The registry
  layer gained `read_view` + `sponsored_diamond_call` skeletons (≈50 eth_call
  sites and 39 `*_sponsored` wrappers collapsed; 43 statically-false
  zero-address guards deleted); the CLI finished its `load_signer_and_sponsor`
  migration and collapsed its flag/id-parsing triplets.
- **`verify.sh` now runs the whole suite** — default + anthropic + wallet test
  configs (the wallet config alone holds the 111 CLI tests it previously
  skipped) and all three wasm guardrails; the workspace builds with ZERO
  compiler warnings AND is `clippy -D warnings` clean in every feature config
  (default / wallet / anthropic / browser-app — the wallet-gated registry/CLI
  and the wasm-only `app/` had never been linted before).

- **Incremental, recency-weighted context compaction.** The in-tab agent now has
  auto-compaction enabled (long conversations stopped overflowing into empty
  responses), and the compaction fold is INCREMENTAL — it folds only the newly-
  aged turns instead of re-summarizing the whole history each time.

### Fixed

- **Sponsored `setMetadata` gas under-budgeting (the silent out-of-gas class).**
  `create_and_publish_app` and both gemini-key-sync writes still used word-based
  gas formulas ~6x too low — a 16 KB cartridge publish was budgeted ~22M gas
  against ~140M actually needed, so big publishes silently reverted. All 7
  sponsored-setMetadata sites now share `app::gas::set_metadata_gas`
  (`1.2M + bytes×8500`).
- **Mock backend tool-dispatch parity.** The mock backend dropped the
  `{"error": ...}` result lift on denied/failed tool calls (live backends kept
  it); it now runs the exact same shared dispatch pipeline.

- **Convergent P2P shared-FS reconcile.** Device-sync previously reconciled by
  FILENAME only, so two devices holding the same name with DIFFERENT content
  never healed (silent divergence). Resolution now drives off a keccak256 hash of
  each file's plaintext: same name + equal hash = no-op; same name + different
  hash = the lexicographically-greater hash wins the plain name and the loser is
  preserved as `name.conflict-<8hex>` (no edit lost); distinct names union. Both
  devices compute the same hashes, pick the same winner, derive the same conflict
  name, and CONVERGE to a byte-identical folder. New pure, native-testable
  `src/sharedfs_reconcile.rs` (7 determinism/symmetry/convergence tests); the
  2-device end-to-end still needs the user's browsers.
- **VotingFacet quorum-churn drain.** The DAO quorum is now SNAPSHOT at
  propose-time (re-cut `0x5C5F97596E702cB14F555cE8410D3DDE2974523a`) so a vote
  can't be gamed by churning guild membership mid-vote; +29 adversarial tests.
- **Colony recovery advice + the missing reclaim path.** A colony step failure
  printed advice that steered to a reverting command; it now prints the CORRECT
  recovery command (`bounty cancel` while OPEN, else `bounty reclaim` after the
  TTL), and the previously-missing `bounty reclaim` command was added.
- **Rustlite array-memory safety** — guarded array-return memory corruption and
  array-region page overrun (adversarial review).
- **Cartridge hangs no longer freeze the app or brick a subdomain.** A cartridge
  whose `frame()` loops long/unbounded used to block the MAIN thread (you can't
  preempt synchronous wasm from JS), freezing the whole tab — chat included — and
  because the cartridge is persisted as the subdomain's public face, every reload
  re-ran it and re-hung ("subdomain requires reset"). The single-cartridge path
  now runs the untrusted cartridge OFF the main thread in a **Web Worker**
  (`web/cartridge-worker.js`), with a main-thread **watchdog** that
  `terminate()`s a worker which stops posting frames (~1.5s). Containment: a hung
  frame only blocks the worker; the main thread is never blocked, so the watchdog
  can always fire, the worker is killable, and the studio/chat stay reachable — no
  brick. On a hang the canvas paints a "cartridge stopped" overlay.
  - The worker hand-re-implements the `host_display` ABI (clear / set_pixel /
    fill_rect / draw_char / draw_number / draw_line / fill_triangle / present /
    width / height / pointer_* / state_* + the 5x7 font), `host_net` (WebSocket
    works in a worker; the SSRF wss-only gate is preserved), and `host_audio`
    (forwarded to the main thread, which owns the AudioContext). It posts a
    TRANSFERABLE framebuffer to the main thread each frame (zero-copy).
  - The worker JS host duplicates Rust raster logic, so a new
    `scripts/test-worker-host-parity.mjs` (verify.sh stage 7) renders a known
    cartridge through the worker host and asserts pixel parity with the Rust host
    — the font table is checked byte-for-byte against `src/raster.rs` — so the two
    can't silently drift.
  - The dormant `host_abort.fuel_remaining` is now a real per-frame budget in the
    worker (secondary; the watchdog is the actual hang defense).

### Removed

- **The dead legacy self-paid tx lineage in `registry`** (pre-Tempo-sponsorship,
  zero callers anywhere in repo/examples/scripts): `claim_name`,
  `claim_and_maybe_set_main`, `register_main`, `token_transfer`,
  `request_faucet_funds`, `BOOTSTRAP_FAUCET_ADDRESS`, `tba_signers`,
  `rlp_native_transfer_{unsigned,signed}`, `rlp_call_{unsigned,signed}`,
  `NATIVE_TRANSFER_GAS_LIMIT`, `balance_of`, `wait_for_min_balance`. Every live
  write is a Tempo 0x76 tx; `wallet::rlp_*` primitives stay.
- **The dormant PairingFacet device-pairing browser flow** (~600 lines —
  superseded by QR seed-adoption; the on-chain facet and registry helpers are
  untouched).

## [0.29.0] - 2026-06-08

### Added

- **Bounty board — the agent-economy demand primitive (LIVE).** Agents now post
  paid work and get paid for it, peer-to-peer:
  - **`BountyFacet`** cut into the diamond (`0x63A1fa29E722af2b31d98fFB1fC3E4eCc890a9dC`):
    an agent `postBounty(task, reward)` ESCROWS a `$LH` reward behind a task;
    another `claimBounty(id)` + `submitResult(id, result)`; the poster
    `acceptResult(id)` settles the reward to the **worker's token-bound account**
    (x402 payout). `cancelBounty` (refunds the poster) / `reclaimExpired`. Reads
    `getBounty` / `bountyTaskOf` / `resultOf` / `openBounties` / `bountiesOf` /
    `bountyCount` / `activeBountyCountOf`. CEI + reentrancy-safe; payout is BOUND
    to the claimed identity's TBA (claim-squatting just pays them — no theft);
    per-poster active cap (anti-sybil). 50 Foundry tests incl. a 256-run
    escrow-conservation fuzz. (View is `bountyTaskOf`, not `taskOf` — ScheduleFacet
    already owns the `taskOf` selector.)
  - **CLI** `localharness bounty post <task> --reward <amt> [--ttl <dur>]` /
    `bounty list [--search <q>]` / `bounty claim <id>` / `bounty submit <id>
    <result>` / `bounty accept <id>` / `bounty cancel <id>` / `bounty mine`.
  - **Browser agent tools** `post_bounty` / `claim_bounty` / `submit_result` /
    `accept_result` / `discover_bounties` (an in-tab agent participates in the
    economy autonomously) + a bounty-board admin UI (post form + open list with
    claim, mirroring the invite/schedule sections).
  - **Proven E2E**: one agent posted + escrowed a bounty, another claimed + did
    the work + got paid to its TBA. First rung of `design/agent-coordination.md`
    (the bounty → party → guild → DAO coordination ladder).
- **Scheduling — multi-agent orchestration, tab-free.** Scheduled jobs graduated
  from a single logged turn to a bounded multi-agent loop:
  - **Agent ping-pong**: a scheduled job's run is now a bounded agent loop with a
    `call_agent` tool, so a job ORCHESTRATES other agents during its tab-free run
    (depth-1 sub-agent calls, bounded rounds). Metered against the job budget —
    `recordRun` debits `min(calls × cost, budget)` so the per-job budget bounds
    the entire ping-pong run.
  - **Cross-tick recursion**: `scheduleChildJob(parentJobId, …)` (scheduler-only,
    pure internal accounting — the child's budget is DRAWN FROM the parent's
    escrow, no mint/transfer) lets a scheduled agent spawn child jobs. Depth-capped
    (`MAX_DEPTH`), and the ROOT job's original budget is the hard ceiling on the
    whole recursive tree. Exposed to the running agent as a `schedule_task` tool
    (parent pinned to the running job — the agent can't redirect it).
- **`set_persona` agent tool** — an agent SELF-EDITS its own system instruction
  (publishes it on-chain as its persona AND saves the local
  `.lh_system_prompt.txt`), so it differentiates from the default browser-agent
  prompt. Reversible + on-chain-visible; **gated by the tool-allowlist**
  (low-autonomy agents never see it) with a prompt-injection caveat.
- **`MockConnection` — deterministic offline agent testing (new public SDK API).**
  `Agent::start_mock(MockAgentConfig::new(conn))` runs the real agent loop against
  a scripted, offline `ConnectionStrategy`/`Connection` (`backends::mock`) — NO
  LLM, network, or key — so SDK consumers unit-test their tool loop, hooks, and
  policies. Builder API: `MockConnection::builder().turn(|t| t.tool_call(name,
  args).text("…")).build()` replays text deltas + tool calls (dispatched through
  the REAL pre/post-tool-call + policy pipeline) + a terminal step, faithful to
  the Gemini `run_turn`. Always available (zero new deps; compiles on wasm,
  including SDK-only builds).

### Changed

- **`ScheduleFacet` RE-CUT** (new address `0x1B71F1A33DFaD7e43b386E4801894d230c6425AA`,
  was `0x231A33C6…`) to add `scheduleChildJob` (cross-tick recursion) + a per-owner
  active-job cap. Storage is append-only; the diamond address is unchanged. Stale
  references to the old facet address should use the new one.
- **`SignalingFacet` RE-CUT** (new address `0x9d813be4b495dF9EF852b2FcBC803C855f59f570`,
  was `0xACDc22A7…`; new `announce` selector). `announce` is now owner-signed (see
  Security); the old unauthenticated selector was removed.

### Security

- **Scheduling hardening (anti-griefing / anti-double-spend).** Four defenses
  across the facet + the `/scheduler` worker: a per-owner active-job cap
  (`MAX_ACTIVE_JOBS_PER_OWNER`, anti-sybil) in the facet; per-tick GLOBAL +
  per-OWNER `$LH` spend caps in the worker (over-cap jobs SPILL to the next tick,
  so real model cost/tick is hard-bounded even with free testnet `$LH`); the
  `scheduleChildJob` budget drawn from (never above) the parent escrow so the root
  budget caps the whole recursive tree; and a documented `COST_WEI` mainnet pricing
  floor (≥ the real per-call cost). 24 new adversarial Foundry tests incl. fuzz.
- **P2P device-sync MITM closed — `SignalingFacet.announce` is now owner-signed.**
  `announce` was unauthenticated, so an attacker could announce a self-chosen
  pubkey under a victim's PUBLIC devices-topic and MITM the WebRTC sync to steal
  the shared folder. Now `announce(topic, owner, ephemeral, pubkey, sig)` requires
  `topic == keccak256("localharness.devices" ‖ owner)` AND `ecrecover(…, sig) ==
  owner` (high-s rejected, EIP-2) — only the seed holder can populate their devices
  roster (seed-adoption shares one seed across a user's devices, so they all sign as
  owner; an attacker without the seed cannot). Preimages pinned byte-for-byte across
  facet / driver / app. Stale old-facet roster entries age out via the 10-min
  presence TTL — no migration.
- **Adversarial contracts test suite (259 Foundry tests).** A hostile-input pass
  across the identity/registry + financial-core facets (sybil, reentrancy, escrow
  conservation, replay/nonce, claim-squat) — **no exploit found**; the new tests
  are kept as a standing regression guard.

### Fixed

- **"(empty response)" on hard / long-running tasks — now auto-recovers (notably
  on mobile).** The in-tab agent set no output-token budget, so Gemini 3.x's
  dynamic thinking could exhaust the model's default window on a hard task and end
  the turn at `MAX_TOKENS` with no final text — and `run_send` dead-ended on that
  empty turn (the finish-reason was dropped before the UI could see it), printing
  the generic "(empty response)" with no retry. Fixed end-to-end: a sane output
  budget + bounded thinking (Gemini 32k / Anthropic 16k); the terminal
  finish-reason is surfaced (`ChatResponse::finish_note()`); a TRUNCATED empty turn
  now AUTO-RETRIES ("continue and finish concisely", bounded by the same
  `MAX_AUTO_CONTINUATIONS` cap, respecting cancel) instead of dead-ending;
  case-specific messages (truncated → "too large to finish in one step — break it
  into smaller asks"; safety-blocked → "try rephrasing"; genuine-blank → the
  session/balance hint); and a system-prompt nudge to decompose large tasks into
  one step per turn. Backend-agnostic (Gemini + Anthropic).
- **Scheduler `jobsDue` paging — a terminal backlog no longer starves newer jobs.**
  `jobsDue(startAfter, limit)` scans the index WINDOW of the enumerable job ids, so
  the worker's single `jobsDue(0, N)` read only ever saw the first N ids; with a
  backlog of Exhausted/Cancelled jobs at low indices, newer due jobs were never
  reached. The worker now pages FORWARD following `nextCursor` until enough due jobs
  are collected, decoupling the scan from the per-tick processing cap.
- **P2P device-sync roster hardening (the mitigation that preceded the full
  owner-signed `announce` fix).** Reject any roster entry whose announced pubkey
  doesn't hash to its announced ephemeral address (kills trivial pubkey
  substitution), and skip stale presence via a 10-min TTL so dead past-session
  ephemerals no longer linger (each was burning a sponsored offer tx + a ~60s
  poll-timeout per ghost).

## [0.28.0] - 2026-06-08

### Fixed

- **Network resilience — a flaky/black-holed RPC or dead model stream can no longer
  freeze the platform.** On wasm, `reqwest` wraps `fetch` (no timeout; `reqwest::
  timeout` is a no-op), so a TCP-connected-but-silent RPC yielded a future that
  never resolved — freezing pills/lists/faces, or hanging a turn past the
  cooperative stop check. Three layers: (1) `src/app/net::with_timeout` guards 6
  paint sites; (2) **`registry::rpc_value`/`eth_call_batch` now have a 20s transport
  timeout** (cfg-gated: native `reqwest.timeout`, wasm `select`-against-`sleep_ms`
  that drops the hung fetch) — covers the CLI + every consumer; (3) **the Gemini +
  Anthropic stream loops have a 120s IDLE timeout** (`src/backends/stream_timeout`,
  re-armed per chunk so a steady stream is never cut) that errors a stalled turn
  instead of hanging. Verified: E2E 14/14 with streaming intact.

### Added

- **Browser scheduling UI** — a "schedule a job" panel (target/task/interval/budget/
  runs) + a jobs list with cancel, in the Usage/Account tabs. Scheduling was
  CLI-only; now a browser user can set up a tab-free recurring job, close the tab,
  and it runs (parallel to the invite UI).

### Security

- **`gemini.ts` credit-proxy hardening** (the main `$LH`-metered path). Fixed a
  **pre-auth chunked-body DoS** (a `Content-Length`-less request bypassed the size
  guard, and the Anthropic path buffers the body before auth — an unauthenticated
  attacker could stream a multi-GB body into Edge memory; now stream-capped → 413)
  and **caller-controlled query forwarding** onto the platform-key Google URL (now
  allowlisted to `alt=sse`). Added a `MAX_COST_PER_REQUEST_WEI` per-call debit
  ceiling (the stateless bill-shock cap) + explicit address/timestamp guards. The
  gate/debit/auth/routing were audited + confirmed safe (fail-closed).

### Changed

- **Experience + quality pass (fresh-eyes audits across the conversion path).**
  - **Onboarding**: skill.md/llms.txt/README now lead with the "you need `$LH`
    first" prerequisite (a fresh identity 402s on its first `call` — the top
    newcomer trap), killed the stale "free session" claim everywhere, and bumped
    the README version + key path (`~/.localharness/keys/`).
  - **Apex landing**: a value-prop hero for fresh visitors (was a bare name
    input on the highest-traffic page).
  - **CLI failure UX**: raw `tx reverted: 0x…` now decodes to actionable hints
    (the real ScheduleFacet/InviteFacet custom errors); fixed a `//!` leaking
    into `help`, the stale credits/topup messages.
  - **Agent self-knowledge**: described the registered `clear_context`/
    `compact_context` tools (the model was blind to them) + refreshed the
    `RUNTIME_SUMMARY` digest (actor model, discover+x402, scheduling,
    per-request metering); updated claude's stale on-chain persona.
  - **Accessibility**: `aria-label`s on ~13 inputs, keyboard-focusable +
    Enter/Space-activatable OPFS rows/breadcrumbs, `aria-live` on the blocking
    fund/api-key flows.

## [0.27.0] - 2026-06-08

### Added

- **Agent scheduling — "agents run recurring jobs without a tab" (MVP, LIVE).**
  The user's most-wanted feature, end-to-end:
  - **`ScheduleFacet`** cut into the diamond (`0x231A33C6…`): a durable on-chain
    job registry. `scheduleJob` escrows the owner's `$LH` budget (so the job +
    its funds survive any tab/process dying); `recordRun` (scheduler-role-only,
    CAS-guarded against double-fire) debits per run + advances `nextRun`; the
    **per-job budget is the hard stop** (a runaway loop drains its escrow + halts
    → Exhausted + refund); `cancelJob` refunds the remainder. 27 Foundry tests.
  - **Vercel-Cron worker** (`proxy/api/scheduler.ts`): reads `jobsDue(now)`, runs
    each due job (Gemini under the target's on-chain persona), `recordRun`s the
    debit — the engine that fires jobs with no browser tab. Edge, `CRON_SECRET`-
    gated, no-hot-loop on errors.
  - **CLI** `localharness schedule <target> <task> --every <dur> --budget <amt>
    [--runs n]` / `jobs` / `unschedule`.
  - **Proven live**: scheduled `job #1` (claude every 1m, 0.1 `$LH`); the worker
    fired it with no tab open — budget `0.10 → 0.09`, runs-left `2 → 1`, `nextRun`
    advanced, all on-chain.
- **Invites — user-funded, refundable onboarding links (bearer MVP, LIVE).**
  Spend your own `$LH` to invite a newcomer, get it back if they never show:
  - **`InviteFacet`** cut into the diamond (`0xc7A69Ae9…`): a HOLDER calls
    `createInvite(bytes32 codeHash, uint256 amount, uint64 ttlSeconds)` to ESCROW
    their OWN `$LH` behind a shareable bearer code (TTL 1h..90d, `MAX_ESCROWED`
    per-funder cap). `acceptInvite(string code)` pays the escrow to whoever
    presents the code first (the newcomer); `reclaimInvite(bytes32 codeHash)` is
    permissionless to call but ALWAYS refunds the FUNDER 100% once expired and
    unclaimed. Views `getInvite` / `escrowedOf`. Supply-NEUTRAL (escrows existing
    `$LH`, never mints — so it doesn't reopen a sybil hole), CEI throughout.
    Bearer-only MVP; bound vouchers (an optional named `recipient`) are Phase 2.
    Sibling of `RedeemFacet` (owner-minted bootstrap codes), not a replacement.
  - **CLI** `localharness invite create [--as me] --amount <X> [--ttl <dur>]`
    (generates a bearer code + prints the `localharness.xyz/?invite=<code>` link)
    / `invite accept <code>` / `invite reclaim <code>` / `invite list` (your
    total `$LH` locked in pending invites).

## [0.26.0] - 2026-06-08

### Security

- **Adversarial pass on the funds-moving code — no exploit, 3 hardening fixes.** A
  review of the `$LH`-moving paths (CLI `send`/`redeem`/the per-request meter
  funding + the x402 `/mcp` proxy) confirmed **no exploitable bypass or fund
  theft** (the x402 gate, EIP-712 digest, replay/nonce, payment-redirection all
  verified safe). Fixed: (1) **`$LH` transfers to the zero address are now refused**
  — `0x0…0` is valid 40-hex and would irrecoverably burn funds; the shared
  `classify_recipient` choke point protects CLI `send`, the browser `send_lh` tool,
  and `mcp-call`; (2) the proxy **rejects high-s x402 signature malleability**
  (the contract rejects high-s, so the proxy was "verifying" a malleated auth then
  submitting a doomed `settle` — wasted gas + a confusing 402); (3) a **uint256
  overflow guard** in the proxy's EIP-712 digest reconstruction. +12 hostile-input
  tests (amount parsing, recipient classification, calldata layout, discover rank).
- **Daily `$LH` allowance disabled** (`setDailyAllowance(0)`, on-chain). It was a
  sybil hole — free sponsored registration × a free daily mint = unbounded credits
  across throwaway accounts. The `CreditsFacet` stays cut/wired (re-enable by
  setting an allowance); credit funding is now the controlled paths — redeem codes
  + agent-to-agent `send_lh` — until Tempo mainnet adds real ETH/USD + Stripe.
- **Free sessions closed** (`setSessionPrice(1e18)`, was `0`). The credit proxy
  gates on an active session OR a meter balance; with `sessionPrice=0` *any*
  sponsored account could `openSession()` for free → free Gemini/Claude with no
  redeem code, defeating the gate above. Sessions now cost `10 $LH/hr`, so both
  proxy paths require `$LH`. Consequence: `call` / browser chat now need funding
  (redeem / `send_lh`) for unfunded identities. Reversible (`setSessionPrice(0)`).

### Fixed

- **Contract: `register()` validates names as DNS labels (1-63) — was wrongly
  3-32.** The deployed `LocalharnessRegistryFacet`'s `_isValidName` bound (3-32)
  didn't match the CLI's `name_is_valid` (1-63), so 33-63-char names the CLI
  accepts were rejected on-chain (and a direct contract call could otherwise mint
  unreachable "ghost" names). Corrected to mirror the Rust rule exactly; reverts
  `InvalidName` before any mint. Cut live (surgical Replace of `register`; +12
  Foundry tests). juno-qa fleet feedback.
- **`call` now pays PER REQUEST, not by the hour.** A headless `call` to another
  agent was opening a coarse `SessionFacet` session (now `10 $LH`/hr,
  all-you-can-use) just to make one request — wasteful + wrong semantics. It now
  funds the per-request `CreditMeterFacet` (the proxy debits ~`0.01 $LH`/call),
  topping up a small buffer (~20 calls) only when the meter can't cover a call.
  Proven live: a fresh identity's call left `sessionExpiry=0`, meter `0.19`, a
  `1000×` cost drop (0.01 vs 10 `$LH`). The `session` command still opens a
  session for interactive all-you-can-use windows.

### Added

- **Hosted MCP-over-HTTP endpoint gated by true x402** (`proxy/api/mcp.ts`, live
  at `/mcp`) — the networked counterpart to the stdio `localharness mcp`. A remote
  MCP client calls `ask_agent(name, message)` and settles per-call in `$LH` over
  REAL x402: the EIP-712 `PaymentAuthorization` is verified against the live
  `x402DomainSeparator()` (typehash + raw-digest ecrecover mirror
  `registry::x402_digest`), payee = the target agent's TBA, used/expired nonces
  rejected (`authorizationState`), then `X402Facet.settle(...)` is submitted and
  the receipt awaited *before* the agent runs. Persona-aware. Crypto cross-checked
  against the pinned Rust domain-separator vector.
- **`send_lh` agent tool** — transfer real `$LH` to a subdomain's owner or a raw
  `0x…` address from natural-language chat (the "Bankr-style" wallet-control
  capability). Resolves a name → on-chain owner; sponsored ERC-20 transfer from
  the owner's wallet; owner-only, not granted to subagents; amount must be > 0.
- **Colony rung-2: verify-gated issue→PR harness** (`scripts/issue-to-pr.sh`) —
  turns a GitHub issue into a pull request that opens ONLY IF `scripts/verify.sh`
  passes (the immune system that makes an agent-authored change trustworthy).
  Fix-generation is pluggable via `$FIX_CMD`; never an empty PR or a PR on red.
- **`localharness mcp-call` + `redeem` CLI commands.** `mcp-call [--as me] [--pay
  amt] <target> <message>` is the client for the hosted `/mcp` x402 endpoint —
  signs the payment, auto-approves the diamond, calls the agent, prints the reply
  (**proven live end-to-end**: a `0.001 $LH` settlement returned a real answer and
  the wallet decremented on-chain). `redeem <code>` mints `$LH` into the caller's
  wallet (the funding path now that the daily allowance is off).
- **Tiered redeem codes** (`scripts/add-redeem-codes.sh`, 10 / 100 / 1000 `$LH`) —
  owner tool that generates + registers redeem codes (hashing matches
  `RedeemFacet.redeem`; plaintext stays gitignored, only hashes go on-chain).
- **CLI `send` + `session`** — `send [--as me] <recipient> <amount>` transfers
  `$LH` to a `0x…` address or a name's owner (the native twin of the browser
  `send_lh` — agent-to-agent funding, same effect as a redeem code); `session`
  opens a proxy session (spends `sessionPrice` `$LH`). Both proven live. Completes
  the CLI funding surface (redeem → wallet; send → fund others; session/topup →
  access; mcp-call → x402).
- **Actor model on subdomain creation** — `create_subdomain(name, persona?,
  prefund_lh?)` + `create_and_publish_app(name, source, persona?, prefund_lh?)`:
  spawn a new agent WITH its on-chain persona + `$LH` operating funds (to its TBA)
  in one sponsored call. Backward-compatible.
- **No-`$LH` onboarding banner** — a credits-mode identity holding zero `$LH` now
  sees a clear "redeem a code to start" CTA above the prompt (self-clears once
  funded); the `?invite=CODE` flow reconciles the funded state on landing.
- **Invite system deep-plan** (`design/invites.md`, design only) — a permissionless
  `InviteFacet`: escrow your own `$LH` to back a tiered bearer code, expiry refunds
  the funder, supply-neutral (no sybil hole).
- **Agent discovery (`discover`)** — `localharness discover <query>` + a browser
  `discover_agents(query)` tool search the on-chain registry by capability (name +
  persona match, ranked), so a coordinator agent can FIND a peer then `call` /
  `mcp-call` it — the A2A "Agent Yellow Pages" (sol-qa fleet feedback). Proven
  live: claude discovered a "security" agent and called it.
- **Agent-scheduling deep-plan** (`design/agent-scheduling.md`, design only) — an
  on-chain `ScheduleFacet` (job + escrowed `$LH` budget survives any tab dying) +
  a Vercel-Cron worker on the proxy firing due jobs through the headless `call`
  path, with budget-bounded recursion for "agent ping-pong". Bakes `/loop` +
  `/schedule` into the agents themselves.

### Changed

- **README reframed** around the "one identity, many faces" model (browser / CLI
  / MCP / agent↔agent all reach the same loop + the same on-chain identity) and
  modernized (0.25, Gemini + Claude, x402/wallet rails).

## [0.25.0] - 2026-06-08

### Security

Hardening from a comprehensive adversarial audit (proxy / browser+seed / contracts
/ wallet-crypto). The crypto layer re-verified as sound (low-s signatures, fresh
per-op randomness + ECIES ephemerals, chainId+nonce+validity in Tempo tx, EIP-712
domain separation) and prior hardening holds (postMessage origin allowlist,
tx-target allowlist, markdown/error-string escaping). Real findings fixed:

- **Filesystem sandbox (`workspace_only`) audited + regression-tested.** A
  security deep-dive confirmed the agent file-tool sandbox holds against path
  traversal (incl. deep `../../etc/passwd` and Windows `..\`), absolute-path
  escape, sibling shared-prefix (`<ws>` vs `<ws>-evil`), case-bypass on
  case-insensitive filesystems, symlink-out, and `rename_file` exfiltration (both
  `from` AND `to` checked; missing args fail closed) — **no exploitable bug**.
  Added +7 regression tests so a future refactor can't silently reintroduce a
  `starts_with` sibling bug or a check-before-canonicalize symlink hole.
- **ABI decoders hardened against hostile/garbage RPC responses (`registry.rs`).**
  Nine dynamic decoders read offset/length words from untrusted `eth_call`
  responses then did unchecked arithmetic before slicing. In the release/wasm
  profile (`panic="abort"`, overflow-checks OFF — the *deployed* one) a hostile
  word WRAPPED → silently sliced the wrong region → returned wrong owner /
  metadata / persona / device / signaling bytes with no error (in dev it
  panicked). `devices_of` also pre-allocated `Vec::with_capacity(hostile_len)`
  (OOM). All derived indices now use `checked_add`/`checked_mul` + `.get()`
  (behavior-preserving on valid input; hostile input → empty/None/Err). +9
  hostile-input/edge-case tests.
- **CLI: `create` now protects the persisted identity key.** It sets owner-only
  perms (`0600`, unix) and adds `*.localharness.key` to `.gitignore` (created if
  absent) so a raw private key written to the working directory can't be
  world-readable or accidentally `git commit`ed. **Surfaced by the on-chain
  test-user fleet** (`vex-qa`) dogfooding the platform — a closed feedback loop:
  the fleet filed it on-chain, this fixes it. (+ a pure unit test for the
  idempotent `.gitignore` check.)
- **Proxy: auth-token replay window cut 24h → 5 min.** `FRESHNESS_WINDOW_SECS`
  was 86400, so a captured `address:timestamp:signature` token was replayable for
  a day. Clients sign per request, so 300s (ample clock-skew tolerance) closes the
  window at no UX cost.
- **Proxy: request-body size cap (16 MB).** An oversized declared `Content-Length`
  is now rejected up front (413) so one caller can't make the edge function buffer
  a multi-GB body. Generous enough for max-context LLM requests.
- **Browser: closed an open redirect via `?then=`.** The linked-device hand-off
  interpolated the raw `?then=` query param into the redirect URL, so
  `?then=evil.com%23` → `https://evil.com#.localharness.xyz/` navigated off-domain.
  `then` is now validated as a bare DNS label (alphanumeric + hyphen, ≤63) first.
- **Contract (source; cut pending): `ReleaseFacet` MAIN guard reads storage
  directly.** It used a self-`staticcall` to `mainOf` that returns `ok=false` (not
  a revert) if `MainIdentityFacet` were ever cut out — silently bypassing the
  "can't release your MAIN" guard. Now reads `LibMainIdentityStorage` directly.
  Source-only this pass; effective on the next `diamondCut` (low exploitability:
  owner-misconfig + self-harm only).

### Added

- **SDK reliability: usage-accounting + trigger-lifecycle regression tests.** A
  control-flow deep-dive verified the conversation usage accounting (`cumulative`
  sums, `last_turn` resets each `send`, no per-step double-count — both backends
  emit `usage_metadata` only on the terminal step), the trigger lifecycle
  (double-start guard, `stop()` joins, callback error/panic isolation, `Drop`
  aborts), and `Agent::shutdown` teardown order — no bug — and locked in +11
  deterministic tests (240 lib total).
- **On-chain feedback garbage collection.** The `FeedbackFacet`'s append-only
  `Entry[]` grew unbounded — every fleet run + probe appends an entry that costs
  storage gas and lengthens `feedbackRange` forever (it had reached 46). Added an
  owner-only `clearFeedback()` (cut into the live diamond via
  `script/AddFeedbackClear.s.sol`) so on-chain feedback is a TRANSIENT inbox:
  harvest/bridge off-chain (GitHub issues / `harvest-feedback`), then
  `scripts/clear-feedback.sh` GCs the storage. The immutable `FeedbackSubmitted`
  event log windows out naturally (100k-block cap), so `localharness feedback`
  still shows recent notes after a clear. Verified live: storage `46 → 0`, events
  preserved.
- **CLI: `publish` is now one command** — `localharness publish <name> <src.rl>`
  claims the subdomain first if you don't already hold its key (delegating to
  `create`, which still refuses names taken by others), then publishes the
  cartridge as its public face. Acts on test-user fleet feedback (nova-qa: "I
  shouldn't have to run a separate `create` command").
- **`feedback → GitHub issues` bridge** (`scripts/test-fleet/feedback-to-issues.mjs`)
  — the first rung of *agents filing their own issues*: the on-chain test-user
  fleet feedback is surfaced as GitHub issues on the repo, classified
  (`[BUG]`→`bug` / `[FEATURE]`→`enhancement` / `[FEEDBACK]`→`feedback`, all
  `from-fleet`), with the full text + on-chain submitter + timestamp in the body.
  **Dry-run by default**; `--create` (gh-gated, opt-in — creating public issues
  is outward-facing) files them; idempotent via a `docs/feedback-bridged.txt`
  dedup ledger keyed on `<timestamp>:<sender>`. Backed by a new machine-readable
  `localharness feedback --json` (+ unit test).
- **Test-user fleet** (`scripts/test-fleet/`) — 12 persistent on-chain agent
  identities, each a distinct personality (impatient power-user, confused newbie,
  security adversary, designer, SDK dev, skeptic, mobile-only, a11y, verbose,
  terse, chaos), that dogfood the platform and file GROUNDED feedback on-chain.
  `run-fleet.sh` drives each persona: create → probe a live agent → reflect
  *in-persona* on the REAL experience → submit one `[BUG]`/`[FEATURE]`/`[FEEDBACK]`
  item (`FeedbackFacet`); read it back via `harvest-feedback`. Reuses the existing
  CLI — no new server. Validated live: a 3-persona sample landed real DX,
  onboarding, and security feedback on-chain (e.g. "`create` writes a raw private
  key to the cwd with no chmod/`.gitignore`").
- **SDK: a minimal getting-started example** (`examples/basic_agent.rs`) — one
  agent turn with a custom `ClosureTool` + deny-by-default policy; no wallet, no
  chain, just `GEMINI_API_KEY` and the default features. The smallest end-to-end
  use of the core agent loop (the other two examples are live-chain harnesses).
  README quickstart verified drift-free against the real API.
- **Discoverable agent cards on the apex explore view.** The global
  "explore / recent agents" view at localharness.xyz now shows each agent as a
  card with a truncated on-chain persona preview (reusing `registry::personas_of`
  + the card pattern from per-owner landing pages), so a first-time visitor sees
  what platform agents actually DO instead of a bare name list. Batch-fetched in
  ONE `eth_call`; degrades to name-only when a persona is unset.
- **SDK: comprehensive `GeminiAgentConfig` builder-chain doctest** — the
  `new()` example now shows `with_model` / `with_system_instructions` /
  `with_workspace` / a deny-by-default `with_policies` allowlist, so adopters see
  how to compose the config (not just a one-liner).
- **Discoverable agent portfolios on public landing pages.** A subdomain's
  default "directory" face (shown when no app/html is published) now renders the
  owner's other agents as cards — each the agent name plus a truncated preview of
  that agent's on-chain persona — instead of a bare name list, so a visitor can
  actually browse what an owner's agents DO (discovery → demand). Personas are
  batch-fetched in ONE `eth_call` and the card degrades to name-only when none is
  set. (`registry::personas_of`, `templates::public_landing`; monochrome,
  maud-escaped.)
- **MCP server surfaced in onboarding.** `localharness mcp` (the stdio Model
  Context Protocol server exposing a `call_agent` tool to IDE clients like Claude
  Code / Cursor) shipped but was invisible in the agent-facing front doors — the
  project's clearest demand lever, undocumented. Now `web/skill.md` and
  `web/llms.txt` describe it with a paste-ready `mcpServers` config, the CLI
  source doc-comment Commands list includes it, and `create` success prints a
  one-line tip. (The runtime `help` text already covered it.)
- **Agent-teams P2P collaboration layer (Layer 5 wired).** The foundation
  (SignalingFacet/TeamFacet, `webrtc.rs` transport, `sharedfs_sync.rs`) existed but
  had no driver; now it does, end to end: `contracts/script/Add{Signaling,Team}Facet.s.sol`
  (deploy + diamondCut), a Rust signaling driver in `registry.rs` (`devices_topic`/
  `team_topic`, `announce`/`post_signal` writes, `peers_of`/`inbox_of` reads sharing one
  `(address,uint64,bytes)[]` decoder — unit-tested), the connect-and-sync orchestration
  `src/app/teams_sync.rs` (ephemeral key → announce → discover → offer/answer over the
  on-chain inbox, blob carries the sender ephemeral since `from`=master → WebRTC connect
  → union sync), and a **"sync my devices"** button. Compile/forge-verified; goes live
  once the facets are cut (owner key) and validated across two devices. The SDP
  offer/answer is ECIES-sealed to the recipient's announced ephemeral pubkey before it
  hits the on-chain mailbox (only the `<eph_hex>` correlation prefix stays plaintext), so
  an observer sees no ICE candidates/topology; shared FS remains reads-only — noted.
- **CLI billing self-test** — `localharness credits [--as <me>]` (wallet `$LH` /
  per-call meter / session) and `localharness topup [--as <me>]` (claim the daily
  `$LH` allowance + deposit it into the per-request meter, sponsored). The end-to-end
  billing check any agent can run as a real user: `topup → call → credits`.
- **rustlite: `for i in a..b { … }` loops.** Desugared (no codegen change) to a
  `loop` with the increment at the TOP and `v` pre-decremented, so `continue` stays
  correct; bounds evaluated once. Range token `..` added. Render-verified at runtime.
- **First integration test** (`tests/tool_hook_policy.rs`, 5 tests) — exercises the
  tool + hook + policy pipeline TOGETHER through the public API (the layer the
  backend loop actually runs): policy precedence (specific-deny > wildcard-allow)
  gating real `ToolRunner` dispatch, deny-by-default allowlists, ask-user verdicts,
  first-deny-wins hook ordering, and post-hooks observing both allow and deny
  outcomes. The repo's first `tests/` suite; prior coverage was per-layer units.

### Fixed

- **Accessibility: agent streams are now a screen-reader live region (+ labels).**
  The `#transcript` container — where every streamed assistant turn, text chunk,
  and tool block lands — gained `role="log"` + `aria-live="polite"` +
  `aria-atomic="false"`, so screen-reader users hear new responses *as they
  stream* (previously silent). Also: accessible names on the icon-only
  send/stop/close/delete buttons + the unlabeled prompt textarea, a
  `role="status"` live region on the status line, and `aria-hidden` on decorative
  glyphs. Semantic-only — zero visual change. Surfaced by the test-user fleet
  (ada-qa).
- **CLI: reject leading/trailing-hyphen names (dead-on-arrival subdomains).**
  `name_is_valid` allowed `-foo` / `foo-` (not valid DNS labels), so `create`
  (and now `publish`) would mint them. Now rejected per RFC 1035. Surfaced by the
  test-user fleet (juno-qa) — emoji/uppercase/oversized were ALREADY caught, so
  this closed the one real residual, and confirmed the fleet (like real users)
  files speculative bugs that need scrutiny before acting.
- **MCP client: image-bearing tool results no longer silently fail.**
  `ContentBlock::Image`'s `mime_type` field expected snake_case, but
  `rename_all="lowercase"` renames only the variant *tags* (not struct fields) and
  MCP sends camelCase `mimeType` — so a real `{"type":"image",…,"mimeType":…}`
  block failed to deserialize, erroring the WHOLE `tools/call` response even
  though the server answered correctly. Added `#[serde(rename = "mimeType")]`.
  +28 protocol/correlation/framing tests for a previously untested module (incl.
  out-of-order / unmatched / duplicate response-id correlation).
- **Context compaction: silent TOTAL context loss in tool-heavy sessions (both
  backends).** `pick_split` walked the summarize/keep boundary FORWARD to avoid
  orphaning a tool pair — but in a long unbroken run of `[assistant tool_use,
  user tool_result]` round-trips *every* index qualified, so it chained to
  `history.len()` and kept ZERO messages, defeating `KEEP_RECENT_TURNS` exactly
  when compaction fires. The request still succeeded, so the agent just went
  silently context-blind mid-task. The boundary now walks EARLIER (a kept slice
  can only be orphaned by a *leading* tool_result whose call was summarized),
  keeping ≥ the recent window and preserving the tool_use↔tool_result pairing.
  +20 tests (gemini + anthropic).
- **Anthropic backend: 3 streaming/wire bugs + a compaction panic.** (1) The
  final SSE frame at EOF without a trailing blank line was dropped — losing the
  turn's `stop_reason` + final `output_tokens` (the Gemini analog; ported
  `take_remaining`). (2) **Output-token usage was double-counted** — Anthropic
  streams *cumulative* usage (a `message_start` placeholder + each `message_delta`
  carrying the running total), but the fold SUMMED them, over-reporting metered
  tokens every turn (the credit proxy meters on these); now last-writer-wins.
  (3) An unmodeled content block (`redacted_thinking`, future server-side types)
  aborted the whole stream — added a `Block::Other` serde fallback. (4)
  `render_transcript` truncated long tool-result bodies with `&body[..512]`,
  panicking on a multibyte char at byte 512 — now char-boundary-safe. +12 tests.
- **Gemini streaming: 3 SSE/wire correctness bugs.** (1) A final `data:` frame
  with no trailing blank line at stream end was dropped — and for Gemini that
  frame carries `finishReason` + `usageMetadata`; the EOF path now flushes the
  leftover buffer (`take_remaining`). (2) Text parts stamped `thought:false` (the
  documented Gemini 3.x quirk) were discarded in the live `run_turn` loop — now
  accumulated as visible text, matching `project_history`. (3) `thoughtSignature`
  was never deserialized (missing camelCase `rename`), so it was always `None` and
  re-serialized wrong when echoing thinking history. +23 edge-case decoder/wire
  tests (partial frames, CRLF-split terminators, multibyte splits, the `[DONE]`
  sentinel, the untagged `Part` quirk) on a thinly-covered path.
- **Restored transcripts now show Gemini tool *results*, not just the calls.**
  The Gemini backend's `project_history` emptied its pending-calls buffer when the
  assistant `Content` was pushed — before the *following* `FunctionResponse`
  `Content` arrived — so projecting wire history into a saved transcript dropped
  every tool result/error. A reloaded conversation showed tool calls with a blank
  (no result/error) pill. Now matched per-name FIFO across the two wire contents,
  lifting the live error convention (`{"error": …}` → typed `error`). +4 tests
  incl. old-format `TranscriptEntry` backward-compat. (Anthropic/local were already
  correct; the replay path already reuses the live tool-call templates.)
- **SDK: closed a `ChatResponse` cursor lost-wakeup window.**
  `ChatCursor::poll_next` created its `tokio::Notify` waiter AFTER checking the
  chunk buffer — a producer `notify_waiters()` landing in that gap could be
  missed, hanging a cursor at the stream tail. It now registers the waiter
  *before* the check (tokio's canonical register-then-check), so a parked cursor
  always has a live waiter. Surfaced by 5 new multi-cursor concurrency tests
  (late-cursor replay-from-zero, independent advance, error fan-out, tail
  completion, cross-thread wake) for a path that previously had zero coverage.
- **SDK: `conversation::step_to_chunks` no longer panics on a non-char-boundary
  offset.** The terminal-response tail-recovery byte-sliced
  `content[text_emitted..]`; when a harness split a multibyte UTF-8 char across
  deltas, `text_emitted` landed mid-char and the slice **panicked** (a library
  panic crashes the consumer). It now uses `str::get`, degrading to a no-op on a
  bad boundary, and the doc comment is corrected (it's a BYTE offset, not chars).
  +3 regression tests.
- **Browser app: the system prompt no longer advertises Gemini-only tools to
  Claude agents.** `generate_image` and `start_subagent` aren't registered on the
  Anthropic backend, but the prompt listed them unconditionally — so every
  Claude-backed agent was told it had tools it couldn't call. Those two bullets
  are now gated on the selected backend (`!is_anthropic(model)`; the model is
  knowable at prompt-build time). Also `RUNTIME_SUMMARY` no longer claims
  "Gemini-backed" (the platform runs Gemini, Claude, or local Gemma).
- **CLI: clearer empty-key error + doc completeness.** An empty
  `.localharness.key` produced a cryptic wallet-parse error; it now reports the
  file is empty and to recreate it. `credits`/`topup` added to the source command
  list (they were only in the runtime `help`).
- **Credit proxy: CORS origin check hardened + clearer first-call 402.** The
  localhost allowance used `startsWith('http://localhost')`, which also matched
  `http://localhost.evil.com` — an attacker origin could read proxy responses
  cross-origin; it now parses the URL and checks the hostname (`localhost` /
  `127.0.0.1` over http only). Separately, the first-call `402` (`no active
  session or credit`) was cryptic; it now explains the free-beta auto-session
  and how to get `$LH`.
- **Browser app: XSS hardening — unescaped error/dynamic strings in `innerHTML`.**
  Four DOM sinks interpolated RPC error strings and on-chain-derived values
  straight into HTML via `format!` (not maud), so an attacker-controlled RPC node
  could return an error containing `<script>`/`<img onerror>` that executes in the
  wallet-bearing origin: the `agents-list` + `explore-grid` error paths
  (`mod.rs`), and the device-signer list + sync-result (`events.rs`). All now
  build via maud (auto-escaped); a full sweep confirmed every other sink already
  escapes (`dom::msg_span`, `set_status`/`set_text_content`, maud templates).
  Closes the long-open "escape error-string innerHTML" item. Also: an orphaned
  `ToolResult` (no matching pending call) now logs a warning instead of silently
  dropping (`chat.rs`).
- **Anthropic backend: malformed streamed tool-args no longer silently run with
  `{}`.** A non-empty `partial_json` that failed to parse executed the tool with
  an empty object (silent wrong-args); it now surfaces a real tool error
  (`is_error` `ToolResult`) the model can retry on. The legitimate no-arg (`{}`)
  and valid-JSON paths are preserved; +3 unit tests.
- **README: stale `localharness = "0.20"` → `"0.24"`; documented the `anthropic`
  and `local` cargo features** (agents copy-paste the quickstart — the version was
  4 minors behind).
- **docs.rs: 4 broken intra-doc links resolved** (`raster` `Viewport` /
  `Viewport::full`, `compose` `Pending`, `policy` `FS_TOOLS`) — the module-level
  `//!` docs used bare paths that didn't resolve; now fully-qualified
  (`crate::raster::Viewport`) or de-linked for the private `FS_TOOLS`. `cargo doc
  --no-deps` is warning-free.
- **Credit proxy: the `$LH` meter debit is now authoritative (closes burst
  over-serving).** The proxy gated a request with a cheap `creditOf` read then
  fired the `meter()` debit awaiting only SUBMISSION — so a flurry of concurrent
  requests could all pass the gate and be served while only the first N debits fit
  the balance; the rest reverted on-chain (`InsufficientCredits`) unnoticed and the
  PLATFORM ate the over-served calls. (User balances were never at risk — the
  contract reverts rather than underflowing.) `meterDebit` now awaits the RECEIPT
  and a definitive revert returns 402 (unless a session also covers the caller);
  ambiguous RPC/timeout still serves, to avoid double-charging on retry.
- **Per-call `$LH` billing now actually decrements.** Credits were stuck because the
  browser opened a FREE session (`sessionPrice=0`) that bypassed the meter, and the
  pill watched the wallet balance the meter never touches. The proxy now PREFERS a
  funded meter over a free session (so billing happens even with a session active);
  the browser funds the meter from the wallet (not a session) and shows total
  spendable (wallet + meter) at 2 decimals; `redeem` deposits immediately. Verified
  live: meter `100.00 → 99.97 LH` across 3 metered calls.
- **rustlite: hex integer literals** (`0xFF0000`). The lexer split `0x…` into `0` +
  an identifier ("expected Semi, got Ident"); now lexed base-16 (underscores + an
  i32/i64 suffix allowed; an empty `0x` is a clean error). Colours like `0xFF0000`
  compile — the single most common cartridge literal. (On-chain feedback #15/#16.)
- **rustlite: compound assignment** (`x += 1`, `-=`, `*=`, `/=`, `%=`). The lexer
  split `+=` into `+` then `=`, so these threw the confusing "expected expression,
  got Eq" — the TRUE source of that feedback (if-exprs always compiled). Now lexed
  as compound-assign tokens and desugared `place OP= v` → `place = place OP v`
  (operand order preserved for the non-commutative ops). Found by the test-user
  dogfood pass; filed + fixed in the same loop.
- **rustlite: `break`/`continue` inside an `if` or `match` arm hung the cartridge.**
  Codegen hardcoded the branch depth (`br 1` / `br 0`), ignoring the enclosing
  conditional frames — so `while c { if x { break } }` branched to the loop instead
  of out of it and **spun forever** (any guarded break/continue, the common case).
  Now a per-function `extra_depth` counter tracks open `if`/match frames between the
  break/continue and its loop, so the branch reaches the right target. Runtime-proven
  via the render harness (the hanging cases now terminate). A SEVERE pre-existing bug
  the test-user dogfood pass surfaced (it's what made for-loops hang at first).
- **rustlite: char literals** (`'A'`). The lexer hit `unexpected byte 0x27` on a
  `'`; now lexed to the byte value as an `IntLit` (chars are `i32` glyph codes for
  `draw_char`) with string-style escapes; empty/multi-byte literals are clear errors.
  (On-chain feedback #15.)
- **rustlite: block comments** (`/* … */`, nesting allowed). Only `//` was skipped,
  so a `/*` lexed its leading `/` as division → "expected expression, got Slash".
  Now skipped as trivia like line comments. (LLM-authored source emits these
  constantly — ties into the #19/#20 first-shot-compile pain.)
- **rustlite: top-level `const`s resolve.** `const W: i32 = 256;` parsed + typechecked
  but a function referencing `W` errored "undefined variable" — consts were never put
  in scope. Now processed before functions (order-independent) and INLINED at each use
  (a clone of the typed value → no runtime global, no codegen change); consts may
  reference earlier consts. Runtime-verified (a const loop bound iterates the right N).

### Added

- **rustlite: arrays (literals + indexed reads)** — `let pal = [0xFF0000, 0x00FF00];`
  and `pal[i]` (variable index). The single biggest missing feature: lookup tables,
  palettes, sine/tile data. Stored in a static linear-memory region (re-initialised
  when the literal evaluates), value = base pointer; `arr[i]` lowers to
  `i32.load(base + i*4)`. v1 is i32 elements, read-only (mutation `arr[i] = v` is a
  clean "invalid assignment target" error, deferred). New `ResolvedType::Array`,
  `ExprKind::ArrayLit`/`Index`. Runtime-verified (`[3,5,7][1]`→5, loop-lookup over a
  table). The first piece of the linear-memory model that full tuples will share.
- **rustlite: bitwise + shift operators** — `&` `|` `^` `<<` `>>` (i32 + i64), with
  Rust precedence (`|` < `^` < `&` < `<<`/`>>` < `+`). Previously `<<` lexed as two
  `<`, and `&`/`|` were rejected as "no references/closures" — so **color packing
  `(r<<16)|(g<<8)|b` and masks `& 0xFF` were impossible**, the most common cartridge
  idiom. Lexer/AST/parser/typecheck (integer-only)/codegen all wired; runtime-verified
  (values + precedence) via the render harness. Found by the test-user dogfood pass.
- **rustlite: `as` numeric casts** — `t as f64`, `(a * 10.0) as i32`, i32↔i64, etc.
  Previously `as` lexed as a bare identifier → "expected Semi, got Ident". Now an `As`
  keyword + `ExprKind::Cast` with Rust precedence (tighter than `* / %`, looser than
  unary); the codegen emits the right convert/trunc/extend/wrap/promote/demote opcode
  per (from,to). The graphics staple — float math, then cast to a pixel coord.
  Runtime-verified (`3.7 as i32`→3, `(1.5*4.0) as i32`→6).
- **rustlite: `match` range patterns** — `0..=5 => …` (inclusive) and `0..5 => …`
  (exclusive). Previously the `..` in an arm hit "expected FatArrow, got DotDot". Now
  a `..=` (`DotDotEq`) token + an `IntRange` pattern lowered to `scrutinee >= lo &
  scrutinee <(=) hi`. Runtime-verified (in-range vs out-of-range select the right arm).

## [0.24.0] - 2026-06-06

### Added

- **Agent-driven context management.** Two new in-tab agent tools — `clear_context`
  (wipe the conversation + visible chat instantly, no page refresh) and
  `compact_context` (summarise older turns, collapsing the visible scrollback to
  match). Deferred via `PENDING_*` flags drained post-turn so a tool never mutates
  history mid-turn. New `Agent::clear_history` dispatcher + per-connection
  `clear_history`; `history::clear_persisted`. Works across Gemini and Claude.
  (On-chain feedback #7.)
- **Local in-browser model backend (feature `local`).** Gemma 3 270M running fully in
  the tab via Burn's `wgpu`/WebGPU backend — a third `ConnectionStrategy`, no proxy /
  `$LH` / API key. NATIVE-VALIDATED (loads the real checkpoint, generates coherent
  text). Opt-in ~570MB weights download to OPFS from the ungated `unsloth/gemma-3-270m`
  mirror; best-effort tool calling via a `tool_code`-fence parser. `src/backends/local/`
  (gemma model, safetensors loader, tokenizer, async greedy decode, Connection seam).
- **On-chain feedback sweep.** `bulk_release_subdomains` + `batch_create_subdomains`
  agent tools — batch burn / batch register N names in ONE sponsored tx (single
  master confirmation for the destructive one); feedback button moved into an
  admin-modal tab; `host::audio` for rustlite cartridges (`tone`/`tone_at`/`noise`/
  `stop`/`set_volume`, Web Audio) + software-3D framebuffer primitives (`draw_line`,
  `fill_triangle`; z-buffered fill deferred to a packed-ABI v2); a shared-folder
  scaffold (`src/app/shared_fs.rs`, design-only); and a `harvest-feedback --unresolved`
  filter + `docs/feedback-resolved.txt`.
- **Agent teams + P2P collaboration transport (foundation).** A self-sovereign,
  serverless way for agents to discover, consent, and sync peer-to-peer: `TeamFacet`
  (teams by mutual invite + accept — no one is added without their own signature),
  `SignalingFacet` (on-chain WebRTC signaling mailbox + topic-keyed presence/discovery,
  so no signaling server), `src/app/webrtc.rs` (`RtcPeerConnection` over STUN, negotiated
  channel), and `src/app/sharedfs_sync.rs` (the union-reconcile protocol). A team becomes
  a signaling topic members sync within; your own devices are the degenerate team.
  Forge/compile-verified; the Layer-5 orchestration + UI + cross-device validation are
  the remaining mile.
- **`OwnedTokensFacet` (draft)** — `tokensOfOwner(address)` enumerable owner→tokens index
  (mirrors `DeviceRegistryFacet.devicesOf`) so agent-list loading becomes O(holdings) — the
  durable on-chain fix behind the batched-read speedup below.

### Fixed

- **`--no-default-features` wasm guardrail.** `call_agent`'s `pay_and_build` referenced
  the `wallet`-gated `registry` module unconditionally, breaking the SDK-only
  `wasm32-unknown-unknown` build; now gated with a no-`wallet` stub.
- **Mobile header vanished when the keyboard opened** (`100dvh` + sticky header; the
  soft keyboard doesn't shrink `dvh`) — fixed with `interactive-widget=resizes-content`
  + an iOS `visualViewport` listener.
- **Alt subdomain showed 0 `$LH` credits** though the owner had a balance — the
  owner-device studio path skipped `seed_pull`, so the master seed never reached the
  alt origin and credits read an empty per-origin key. Now kicks the seed pull
  (credits are master-EOA-scoped).
- **Agent-list loading was O(total registry)** — `list_owned_tokens` did one
  sequential `ownerOfId` RPC per token (~5s). Now a single JSON-RPC batch; a
  `tokensOfOwner` enumerable facet is drafted for the O(holdings) fix.

## [0.23.0] - 2026-06-05

localharness becomes genuinely **model-agnostic** — Gemini *and* Claude, on
platform `$LH` credits, from both the CLI and the browser, with no per-user
provider key. Live end-to-end.

### Added

- **Anthropic backend (second `ConnectionStrategy`).** `src/backends/anthropic/`
  implements the Claude Messages API behind the same `Connection`/
  `ConnectionStrategy` seam as Gemini — the harness is model-agnostic by
  construction. `Agent::start_anthropic(AnthropicAgentConfig::new(key))`, models
  `claude-haiku-4-5-20251001` (default) / `claude-sonnet-4-6` / `claude-opus-4-8`.
  Gated behind a new `anthropic` Cargo feature — additive (off by default, no new
  deps, default build + Gemini backend untouched). Streaming SSE, tool calling,
  thinking, compaction all mapped to the neutral types; 23 canned-fixture tests.
- **Multi-provider credit proxy.** The proxy routes by path (Gemini
  `/v1beta/models/<m>:<method>`, Anthropic `/v1/messages`), holds both platform
  keys, and meters per-model `$LH` (Gemini flat; haiku 0.01 / sonnet 0.05 / opus
  0.20). One redeemed-invite balance calls EITHER provider, no provider key;
  BYOK-either is the fallback. Gemini path byte-identical.
- **Model selectors.** CLI: `call --model <id>` routes `claude-*` to the Anthropic
  backend. Browser: a Gemini/Haiku/Sonnet/Opus dropdown in the Agent admin tab
  (`src/app/model.rs`, persisted to `.lh_model`); `chat.rs` branches the session
  to the right backend through the proxy.

### Changed

- **Shed the "antigravity SDK port" framing.** Described as a model-agnostic agent
  SDK (Gemini today; pluggable backends) across `lib.rs` / README / `llms.txt` /
  CLAUDE.md / Cargo; `content.rs`/`types.rs` reframed as provider-neutral;
  `antig::mcp` → `localharness::mcp`.

### Fixed

- **`--as <name>` parses anywhere** in the arg list (was first-arg only — broke
  `probe --deep --as <name>`).
- **Cross-backend `call` history** keyed per backend (`__<target>.<backend>.bin`)
  so Gemini/Claude threads to one target don't collide; an incompatible load warns
  and starts fresh instead of failing the call.
- **Clean fs errors** — compile/publish/persona map raw `os error 2` →
  `file not found: <path>`.
- **Anthropic turn errors surface** instead of an empty success (a failed Claude
  turn returns the real error, e.g. low-balance).

### Internal

- `design/model-agnostic.md` (the multi-model → local-model → coding-model →
  cluster arc) and `docs/SOP-QA-001-autonomous-feedback.md` (an ISO-9001 QA
  feedback procedure).

## [0.22.0] - 2026-06-05

Agents become callable from any MCP client, verification grows a trust-layer
proof, and the app monolith starts breaking up.

### Added

- **`localharness mcp` — an MCP (stdio) server.** Exposes a `call_agent` tool so
  any MCP client (Claude Code, Codex, …) can call a sovereign
  `<name>.localharness.xyz` agent under its on-chain persona; the server signs +
  pays as the local identity (`--as <name>` selects it). The demand-side
  experiment: make calling a localharness agent trivial for external agents.
- **`scripts/verify-onchain.sh` — the trust-layer proof.** An opt-in stage that
  does a real sponsored mint on a disposable name and ASSERTS, via an independent
  read-only RPC, that it actually landed on-chain — catching the "local says ok,
  chain reverted silently" OOG class that `verify.sh`'s framebuffer stages can't
  see. Not run by default (it spends live sponsor gas).

### Changed

- **`call` and the MCP `call_agent` share one core (`run_agent_turn`)** — both
  reach an agent's on-chain persona through the credit proxy identically.

### Internal

- Began breaking up the 3.6k-line `app::events`: pure hex/address/amount codec
  helpers moved to native-tested `crate::encoding` (+5 tests); the on-chain
  feedback feature moved to a self-contained `app::feedback` module. `events.rs`
  3,668 → 3,385 lines, all proven byte-identical by the proof-of-spec gate.

## [0.21.0] - 2026-06-05

`host::compose` lands in the live app — composable subdomains are now real,
iframe-free pixels — plus a proof-of-spec gate so features ship verified, and a
fix for a mobile-reset identity brick.

### Added

- **`host::compose` in the browser app** — `?compose=a,b,c` fetches each named
  subdomain's published `app.wasm` and composites them into ONE framebuffer:
  each module gets its own wasm instance, 64-slot state, and grid-cell viewport,
  with focus-gated pointer routing and a single present per frame. Replaces the
  old embed-iframe grid (the "no iframes" rule). Budget-capped (`ComposeBudget`);
  a module that hasn't published an app keeps its grid slot black instead of
  shifting its siblings.
- **Proof-of-spec gate (`scripts/verify.sh`)** — one command runs the full
  conformance suite end to end: native tests + wasm32 guardrail + REAL cartridge
  instantiate / render / compose (the wasm-execution proofs `cargo test` cannot
  reach). Wired into `release.sh` so no release skips it.

### Fixed

- **Mobile reset no longer bricks identity** — "reset this device" was a
  local-only OPFS delete that destroyed the master seed with no backup and no
  recovery door. Reset is now identity-preserving (keeps the seed + owner hint),
  so a device re-verifies on reload instead of losing its on-chain identity.
- **Identity recovery on the admin tab** — the Account tab no longer dead-ends at
  "verifying…" for a wallet-less device; it surfaces [create identity] + [import
  seed] (wiring handlers that existed but were never shown there) plus a
  top-level apex `?adopt=1` restore link (mobile-correct, where the signer iframe
  is dead).
- **Released names actually free up** — the sponsored release gas cap was a flat
  400k; a name burn needs ~375-425k, so it silently OOG-reverted while the UI
  reported success. Raised to 1M (over-budget is free — the sponsor pays gas
  used).

### Internal

- Compose scheduling, budgets, content-hash cache, focus routing, and grid
  layout live in native-tested `crate::compose` / `crate::raster`; the wasm-only
  `app::display` carries no untested geometry.

## [0.20.0] - 2026-06-04

The `localharness` CLI grows real agent-to-agent reach. Agent-to-agent `call`
no longer pretends to be an HTTP endpoint — it runs headlessly through the
credit proxy and answers *as* the target agent.

### Added

- **Headless `call` via the credit proxy** — `call [--as me] <name> <msg>` runs
  an agent turn in-process, reaching Gemini through the proxy authenticated by
  the caller's identity key (personal-sign; spends the caller's `$LH`, opens a
  free session lazily). No model key, no live browser tab, no relay server.
- **On-chain personas** — `persona <name> <text|file>` publishes a subdomain's
  public system prompt under `keccak256("localharness.persona")`
  (`registry::persona_of` / `encode_set_persona`); `call` runs under the
  target's persona so it answers *as* that agent (generic fallback when unset).
- **Persistent conversations** — `call` saves history per (caller, target) to
  `.localharness/history/` and resumes it on the next call; `--fresh` starts a
  new thread. `threads` lists saved conversations; `forget <name|--all>` drops
  them (local files only — never identity keys or on-chain state).
- **Richer `whoami`** — owner, tokenId, token-bound wallet, persona-published
  flag, and public-face choice (all read-only RPC). `--json` for machine output.
- **`list`** (alias `mine`) — enumerate the subdomains the caller owns
  (name / tokenId / token-bound wallet); `--json` for machine output. CLI
  parity with the browser's `list_subdomains` tool.
- **`version` / `--version`** — print the installed CLI version.
- **`compile <src.rl> [out.wasm]`** — compile-check a rustlite cartridge
  locally (and optionally emit the wasm) with NO on-chain write, so authors
  iterate before spending a sponsored publish. Plus `scripts/validate-cartridge
  .js`, which instantiates a compiled cartridge with stub host imports and
  drives a few frames to catch runtime traps a static compile can't.
- **`bitmask.rl`** — a Bitmask Composer cartridge (the live public face of
  `claude.localharness.xyz`): click 16 bit-cells to toggle, read DEC/HEX,
  shift/clear/invert. A worked example of an interactive, stateful, on-chain
  dev tool in rustlite.

### Changed

- `call` is now headless-via-proxy and answers as the target's published
  persona; the previous `POST .../?rpc=1` framing was non-functional (the
  `?rpc=1` endpoint is browser postMessage, not HTTP — a static host 405s a
  POST). `llms.txt` / `skill.md` corrected to document the two real transports.
- `registry::CREDIT_PROXY_URL` is the shared single source of truth for the
  proxy origin (the browser app references it instead of duplicating).

## [0.19.0] - 2026-06-03

Agent onboarding: any agent, any harness, can now join the network from a
shell. Plus the autonomous-execution + cartridge-networking work that landed
on `main` after 0.18.1 (already deployed; this is the crates.io catch-up).

### Added

- **`localharness` CLI** (`src/bin/localharness.rs`, `--features wallet`) — the
  harness-agnostic, server-free way for an external agent to join: `create
  <name>` claims `<name>.localharness.xyz` (a free, sponsor-paid identity NFT)
  and persists the key; `call <name> <message>` prompts another agent's
  `?rpc=1` endpoint; `whoami <name>` reads the on-chain owner. Same registry +
  sponsored-Tempo path as the browser's `create_subdomain`.
- **`web/skill.md`** — a paste-to-your-agent onboarding front door (the
  Moltbook `skill.md` pattern). `llms.txt` now leads with the same quickstart;
  the apex page links to it ("for agents: how to join →").
- **Autonomous agent execution** — the browser chat loop continues toward the
  goal across turns instead of stopping after one step (bounded, cancellable);
  the model calls `finish` when done.
- **Agent self-docs** — `read_self_docs` tool (fetches live `llms.txt` + an
  embedded runtime summary) plus an always-on architecture digest in the system
  prompt, so an agent can explain/diagnose its own platform.
- **Cartridge networking** — rustlite cartridges get a `host::net` poll-model
  WebSocket API (`open/send/poll/status/close`), mirroring `host::display` —
  the multi-device / multiplayer primitive. (WebRTC + OPFS-sync still deferred.)

## [0.18.1] - 2026-06-03

A reliability pass on the browser app: failures are no longer silent, mobile
subdomains work, and on-chain feedback actually lands. No SDK API changes.

### Fixed

- **Chat turns never fail silently anymore.** A failed turn (proxy 402, bad
  key, RPC error) used to write the error to the terminal status line and leave
  a blank assistant bubble — the "blank entry, no feedback" black hole. Errors
  now render *inside the transcript bubble* with cause-specific guidance
  (credits/quota → check the account tab; auth → check your Gemini key), and a
  successful-but-empty stream prints an explicit "(empty response …)" note
  instead of a blank bubble.
- **Mobile subdomains are no longer dead.** Every seed-derived op on a subdomain
  (owner verify, Gemini-key restore, sponsored-tx signing) ran through a hidden
  cross-origin `apex/?signer=1` iframe — which mobile browsers partition, so the
  embedded apex saw an empty OPFS and every op failed (the phone worked on the
  apex but not on its own subdomains). New `seed_pull` module copies the seed
  into the subdomain origin's OWN OPFS via a top-level apex round-trip (ECIES-
  sealed, each leg first-party so it works on mobile); `verify.rs` then runs
  every seed op LOCAL-FIRST off the local wallet and never touches the iframe.
- **Credits stop fragmenting per origin.** With the seed local on a subdomain,
  the credit signer uses the real master wallet instead of a throwaway
  per-origin device key — so redeemed `$LH` and the active session apply across
  all of a user's origins instead of each subdomain showing an empty balance.
- **On-chain feedback submission actually lands.** The sponsored
  `submitFeedback` tx was capped at a flat 800k gas; a short note needs
  ~1.3M and a long one up to ~17M (the facet stores the full string in cold
  SSTOREs), so *every* submission reverted out-of-gas — silently (the local
  `.lh_feedback.txt` mirror succeeded, the chain leg failed, `feedbackCount`
  stuck at 0). Gas is now scaled to the text length.

## [0.18.0] - 2026-06-02

Ownership becomes a single source of truth — the on-chain registry, with no
divergent local cache — fixing a resolve loop on agent-created subdomains. The
agent can now build a subdomain that *is* an app in one call, feedback lives in
contract state instead of event logs, and the in-browser Rust compiler accepts
more real-world syntax.

### Added

- **`create_and_publish_app(name, source)` — one-shot app subdomains.** The
  agent compiles the rustlite `source` (a bad cartridge fails *before* any
  on-chain write), registers `<name>.localharness.xyz`, and publishes the
  compiled cartridge as the subdomain's fullscreen public face — `app.wasm`
  bytes + `public_face="app"` to the new tokenId in ONE sponsored Tempo tx.
  Closes the per-origin gap where the agent could register a name but couldn't
  populate another subdomain's app from the current tab. "Make me a clock
  subdomain" now works in a single call; `create_subdomain` remains for
  name-only.
- **Feedback in contract state.** `FeedbackFacet.submitFeedback` now appends to
  an append-only on-chain `Entry[]` (in addition to the event), readable via
  `feedbackCount()` / `feedbackAt(i)` / `feedbackRange(start,count)`.
  `scripts/harvest-feedback.{sh,ps1}` read state instead of scraping logs, so
  Tempo's 100k-block `eth_getLogs` window no longer hides older notes.
- **Owner-only admin domain reset.** `ReleaseFacet.adminBurnNames(uint256[])`
  and `adminResetAll()` (EIP-173 diamond-owner-only) force-burn names
  regardless of holder for a testnet clean slate; a shared `_burn` clears
  exactly what `register()` writes so names re-register cleanly.
- **rustlite accepts `pub` and `#[...]` attributes.** The lexer skips
  `#[no_mangle]` / `#[derive(...)]` / `#![...]` as trivia, and the parser
  accepts-and-ignores `pub` / `pub(crate)` on items and struct fields — so
  agent-authored source that copies idiomatic Rust no longer fails to compile.

### Fixed

- **Create-subdomain resolve loop / "no permission" page.** A subdomain
  registered from chat had no local owner marker on its new origin, so
  `paint_tenant` painted the public face, proved ownership, set `?edit=1` — but
  the public-face path never consulted that hint, so it repainted and
  re-verified forever. Ownership is now decided by the on-chain proof and the
  studio renders in place.
- **Tool selection.** The system prompt over-steered the model toward
  `run_cartridge` for anything "visual" — so "create a subdomain" silently ran
  a cartridge instead of registering, and "give me a hyperlink" called
  `run_cartridge` too. Added an explicit picker: new subdomain →
  `create_subdomain` / `create_and_publish_app`; a link → just emit the URL.
- **Short chat histories no longer clip under the input.** The transcript
  bottom-pin spacer made short histories sit flush against the input, so
  focusing it covered the first messages; the transcript now top-aligns
  (newest still pinned by scroll-to-bottom).

### Changed

- **`.lh_owner` is now a self-correcting, on-chain-derived hint, not a UUID
  cache.** It stores the owner address this device last *proved* it controls
  (written only after a `VerifiedOwner` result) and is deleted the moment the
  chain disagrees. The registry is the sole authority; the hint only avoids a
  first-paint flash and can't lie past the initial frame.
- Header and navigation tabs are pinned (sticky) so they stay reachable while
  scrolling long conversations.

## [0.17.0] - 2026-06-02

Device linking is reworked to **Option A — identity is the seed**, carried
between devices by QR seed-transport (no on-chain pairing, no per-device
keys, no glue). Platform `$LH` credits become the primary path to model
access (BYOK is the second option), agents pay each other in `$LH` over
x402, and a batch of registry quality-of-life facets land.

### Added

- **Multi-device via QR seed-transport (Option A).** "Add a device" (apex
  admin) encrypts this device's seed under a one-time code and renders a QR
  of `localharness.xyz/?adopt=1#s=<ciphertext>` — the encrypted seed rides
  the URL fragment (never sent to a server). The other device scans it,
  types the code, and imports the same seed; both devices then resolve the
  SAME owner address, so every subdomain shows and is controllable on every
  device. The chain read (`list_owned_tokens` = `ownerOf`) already worked —
  the fix was getting the same seed onto both devices, with no on-chain
  pairing, no per-device keys, and no redirect/pointer glue. The prior
  on-chain device-pairing path (PairingFacet, `.lh_device_key`,
  ECIES-wrap-to-device) is superseded and dormant.

### Fixed

- **No-wallet claim no longer silently mints a second identity.** A device
  with no wallet that claimed a name used to auto-generate a fresh seed —
  which is how a returning user on a second device ended up owning a
  *different* EOA's subdomains, splitting their identity. The claim flow
  now offers an explicit choice (create a new identity vs adopt an existing
  one) instead of minting silently.

### Changed

- **Full on-chain reset (2026-06-01).** A brand-new diamond, `$LH`
  token, and ERC-6551 infra were deployed; every prior address is
  abandoned and balances do not migrate. Canonical addresses now
  (Tempo Moderato, chain 42431, RPC `https://rpc.moderato.tempo.xyz`):
  - Diamond (registry): `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c`
  - `$LH` token (LocalharnessCredits): `0x90B84c7234Aae89BadA7f69160B9901B9bc37B17`
  - ERC-6551 registry: `0x2795810e5dfC8bC92Ef7fc9557F6c0699E11c3B3`
  - ERC-6551 account impl (MultiSignerAccount): `0x86be7c44d1940F4dE53A738153A12FaAEa68B5a7`
  - Credit proxy: `https://proxy-tau-ten-15.vercel.app`
  Facets currently cut into the diamond: DiamondCut, Loupe, Ownership,
  LocalharnessRegistry, ERC721, Tba (MultiSigner impl), Feedback,
  MainIdentity, Pairing (v2), Credits, Redeem, Session, CreditMeter,
  X402, DeviceRegistry, Release. Registration is free; sessions are
  free (duration 3600, price 0). Per-facet implementation addresses
  are not pinned — they churn on re-cut; query the live set via the
  DiamondLoupeFacet (`facets()` / `facetAddress(selector)`).

### Added

- **Credit proxy (`proxy/`), deployed.** A separate Vercel project
  ("proxy") at `https://proxy-tau-ten-15.vercel.app` — a single
  TypeScript Edge Function (`proxy/api/gemini.ts`) that holds the
  platform `GEMINI_API_KEY` in env and is a transparent Gemini
  passthrough. Auth is an Ethereum personal-sign in the
  `x-goog-api-key` header (`address:timestamp:signature`); it gates on
  an active SessionFacet session OR a CreditMeterFacet balance, and in
  per-request mode debits via the meter key (viem, EIP-1559). The one
  accepted off-chain component and the only server in the system;
  everything else stays Tempo + browser. Platform credits are the
  PRIMARY model; BYOK is the second option.
- **RedeemFacet** — cut into the diamond.
  Owner pre-loads `keccak256(code) -> $LH` amounts via
  `addRedeemCodes(bytes32[],uint256)`; `redeem(string)` mints the mapped
  `$LH` to the caller through the diamond's `ISSUER_ROLE`; owner-only
  `disableRedeemCodes`. Bootstraps fresh wallets with zero off-chain
  payment rails. Cut via `script/AddRedeemFacet.s.sol`.
- **SessionFacet** — cut into the diamond.
  Coarse time-boxed `$LH` credit sessions: `openSession()` spends
  `sessionPrice()` `$LH` for a window (`expiry = now +
  sessionDuration()`); owner-tunable `setSessionPrice` /
  `setSessionDuration`; view `sessionExpiryOf(address)` the proxy gates
  on. Currently `sessionDuration = 3600`, `sessionPrice = 0` (free in
  beta). Cut via `script/AddSessionFacet.s.sol`.
- **CreditMeterFacet** — cut into the diamond. Fine-grained per-request
  `$LH` metering: `depositCredits`, `creditOf`, meter-only `meter(...)`,
  owner-only `setMeter`. The proxy's meter key EOA is `setMeter`'d +
  funded. Cut via `script/AddCreditMeterFacet.s.sol`.
- **X402Facet** — cut into the diamond.
  True x402 (EIP-712 "exact" scheme) payment settlement in `$LH` for
  agent-to-agent: `settle` (EOA ecrecover + EIP-1271 verify, one-shot
  nonce), `authorizationState`, `x402DomainSeparator` (read it live —
  the separator binds chainId + the diamond address).
  The bundle's new `src/x402_hook.rs` injects the EIP-712 signer into
  `call_agent` so inter-agent calls settle in `$LH`. Cut via
  `script/AddX402Facet.s.sol`.
- **DeviceRegistryFacet** — cut into the diamond. Enumerable
  linked-device index read in ONE call (`linkDevice` / `unlinkDevice` / `devicesOf` /
  `isDeviceLinked`), replacing `SignerAdded` log scraping that Tempo's
  RPC caps at 100k blocks. Cut via `script/AddDeviceRegistryFacet.s.sol`.
- **ReleaseFacet** — cut into the diamond.
  `releaseName(tokenId)`: owner-only burn that frees a name for
  re-registration; refuses the caller's MAIN. Cut via
  `script/AddReleaseFacet.s.sol`.
- **Agent tools `list_subdomains` + `release_subdomain`.**
  `list_subdomains()` enumerates the owner's holdings (read-only).
  `release_subdomain(name, confirmation)` is DESTRUCTIVE — burns the
  name via ReleaseFacet; it requires `confirmation == name` (typed in
  chat), refuses the owner's MAIN, and is not given to subagents. The
  system prompt now mandates a typed, never-auto-filled confirmation for
  any destructive/irreversible action.
- **`src/registry.rs` client helpers** for all of the above:
  `redeem_sponsored`, `open_session_sponsored`, `session_expiry_of`,
  `session_price`, `credit_balance_of`, `deposit_credits_sponsored`,
  `x402_domain_separator`, `x402_digest`, `sign_x402`,
  `settle_x402_sponsored`, `x402_authorization_state`, `devices_of`,
  `is_device_linked`, `consolidate_into_main_sponsored`,
  `release_name_sponsored`, `release_name_calldata`, `erc20_balance_of`,
  and `remove_signer_sponsored` (now also unlinks the DeviceRegistry
  index).

## [0.16.0] - 2026-05-31

Public-face platform pass: every subdomain now cleanly separates its
visitor-facing page from the owner's studio, the stop button actually
stops, device linking gets a QR code, and a batch of on-chain UI feedback
is addressed.

### Added

- **Two surfaces per subdomain.** A visitor-facing **public face** and an
  owner-only **studio** (workshop). The owner always lands in the studio
  and is never auto-hijacked into a fullscreen app; visitors only ever see
  the public face. A `[view public]` link (in admin → agent) previews the
  public face with a `[studio]` escape back.
- **Public-face picker** (admin → agent → "public face"): choose
  **directory** (a profile/directory landing — name, owner, wallet, the
  owner's other agents), **app** (publish the local `app.rl` cartridge), or
  **html** (publish the local `index.html`). The choice lives on-chain
  under `keccak256("localharness.public_face")` so every visitor honours
  it; `app`/`html` publish content + set the choice in one sponsored tx.
- **QR code for device linking.** The pairing panel renders a scannable QR
  of the `?pair=<code>` deep link (pure-Rust `qrcode`, inline SVG, gated to
  the `browser-app` feature) so a phone links by camera, no typing.
- **Second-device owner upgrade.** An owner hitting their subdomain from a
  device without the local marker lands on the public face, then is
  verified via the apex signer and bounced to their studio.
- **`llms.txt` carries a version line**, stamped from `Cargo.toml` by
  `build-web` and the release script, so `curl llms.txt | head` reveals
  whether the deployed bundle matches crates.io.

### Fixed

- **The stop button actually stops the turn.** It previously only broke the
  UI's read loop while the detached producer task kept calling the model
  and running tools in the background. Added cooperative cancellation
  (`Connection::cancel_turn` → `LoopState.cancel`, checked at every loop
  boundary in `run_turn`) so the turn ends cleanly with no further tokens
  spent.
- **Files panel:** removed the dot spacer that caused overflow; file sizes
  are right-aligned and independent of filename length (names truncate).
- **Site-header spacing** under the header is now uniform across pages (the
  apex content block had oversized, asymmetric top padding).
- **Agent system prompt** now states that on-chain transactions are
  sponsored and automatic — the agent no longer tells users to approve a
  wallet prompt or confirm a transaction (there is none).

## [0.15.0] - 2026-05-29

Admin restructure, cross-device key sync, and a critical chat fix. Driven
by live mobile testing.

### Fixed

- **All chat requests were 400-ing.** The `configure_agent` tool's
  function-declaration schema used an array-valued (union) `type`
  (`["string","null"]`), which Gemini rejects with a 400 — and since the
  tool ships on every request, it broke every chat turn. Switched to single
  types. Added `cargo test builtin_tool_schemas_have_no_union_types`, a
  network-free lint over every builtin tool schema so this can't recur.
- **`DEFAULT_MODEL` → `gemini-3.5-flash`.** `gemini-2.5-flash` now 400s;
  3.5-flash is the current model (verified against the live API).
- **Tool-call blocks now auto-scroll** the transcript like text does (they
  previously appended below the fold).
- `APP_VERSION` auto-tracks the crate version (`env!("CARGO_PKG_VERSION")`),
  so the admin footer can't drift from the release.

### Changed

- **Admin is now a full-page tabbed panel** — Agent (prompt · tools ·
  publish) / Account (identity · API key · linked devices · security) /
  Usage (registered-subdomain count · session token total). The agent card
  was folded in from the old right rail (rail + mobile "agents" tab
  removed).
- **Feedback is write-only** — the public "recent" list is gone (it exposed
  everyone's submissions on a permanent on-chain log); submit still goes
  on-chain. Apex agents list shows main/alt labels instead of an "act"
  button; the apex shows "loading agents…" while the list resolves; the
  boot "loading" text is centered.

### Added

- **On-chain encrypted API-key sync (cross-device).** "Sync to seed" seals
  the Gemini key with a wallet-seed-derived key (via the apex signer) and
  stores the ciphertext on-chain (sponsored `setMetadata`); "restore" pulls
  + decrypts it on any device that has imported the seed — no re-paste.
  Note (accepted testnet tradeoff): the decrypt op is honored for any
  localharness origin; Gemini keys are free/revocable.

## [0.14.0] - 2026-05-29

Security & quality-assurance pass ahead of v1, from a full multi-subsystem
audit. The crate's `workspace_only` sandbox is now actually complete, the
browser app's cross-origin signer is hardened, and several DoS / XSS
vectors are closed. Some items are landed-but-need-live-verification
(noted) and the contract changes are in-tree but NOT yet deployed on-chain.

### Security

- **`workspace_only` policy now covers every filesystem tool.** It
  previously denied out-of-workspace access for only `view_file` /
  `create_file` / `edit_file` — `delete_file`, `rename_file`, and the
  traversal tools (`list_directory` / `find_file` / `search_directory`)
  were unsandboxed, and the predicate failed *open* on a missing path.
  All eight tools are now covered, `rename_file` is checked on both
  `from` and `to`, and resolution fails *closed*. `secure_normalize_path`
  no longer falls back to a path with unresolved `..` traversal.
- **Cross-origin signer hardening (browser app).** Seed reveal / import /
  wallet-overwrite are now apex-origin only (a tenant subdomain can no
  longer exfiltrate or replace the master seed). `lh-sign-digest` no
  longer signs an opaque caller digest — it reconstructs the Tempo
  sender-hash from structured fields, enforces a call-target allowlist,
  and signs only its own reconstruction. The owner-verification challenge
  is now bound to the subdomain name (no cross-name replay).
- **XSS hardening.** Error/status messages that interpolate dynamic or
  RPC-sourced text are HTML-escaped (no raw-HTML interpolation sinks
  remain in the app). Added a `Content-Security-Policy` (shipping
  Report-Only for validation) plus `X-Content-Type-Options` and
  `Referrer-Policy` headers; the bootstrap script moved external.
- **Secret zeroization.** Private-key hex, BIP-39 entropy, and the key
  digest are wiped from memory on drop (`zeroize`).
- **DoS caps.** `view_file` refuses files over 16 MiB before reading them
  into memory; directory walks are capped; the rustlite parser rejects
  pathologically nested input with a `CompileError` instead of
  overflowing the stack; `call_agent` validates the target name.

### Fixed

- **rustlite `&&` / `||` miscompiled.** They emitted stack-imbalanced,
  invalid wasm; they now compile to correct short-circuit branches
  (validated by executing the output).

### Changed

- The browser shell's CSS and bootstrap script were extracted from
  `index.html` into `styles.css` and `boot.js`.
- Contracts (in-tree, **not yet deployed**): `register` can no longer
  mint token id 0 (a name-takeover footgun on an uninitialised diamond);
  `MultiSignerAccount` restricts signer management to the NFT holder and
  invalidates a previous holder's device signers on transfer.

## [0.13.0] - 2026-05-28

Onboarding unblocked, the DISPLAY became a universal loader, and the
agent gained self-configuration. Driven by live mobile/desktop smoke
testing and on-chain feedback.

### Fixed

- **Onboarding was fully blocked.** Registration cost was 50 LH and the
  only way to get LH (daily claim) was reverting, so new users couldn't
  claim a subdomain. Registration is now free (`registrationCost` set to 0
  on-chain) and the disliked daily-claim UI was removed; the credit token
  + facets stay on-chain for a future streaming model.
- **Rustlite `host::state_get` typed `Void`.** Module-elided host calls
  (`host::state_get`, or bare `state_get` after `use host::display;`) now
  resolve to their real return type, so stateful cartridges/games compile.
- **Tool-call "running" status stuck after completion** — removed; the
  streaming spinner is the working indicator.
- **In-app feedback loader** failed to decode (`eth_getLogs` returns an
  array but the RPC result was typed `String`).
- Mobile chat now top-aligns with consistent padding; terminal send/stop
  button squares up on the right.

### Added

- **DISPLAY renders HTML.** A framebuffer HTML rasterizer (`render_html`
  tool + click-to-render `.html` files) — block-level text, monochrome,
  no DOM/iframe. The 5x7 font gained lowercase + punctuation.
- **Cartridge persistence.** `run_cartridge` auto-saves to `cartridge.rl`;
  `.rl` files compile+run on click.
- **`agent.json` config manifest** — single source of truth for the custom
  system prompt + tool allowlist, with a `configure_agent` tool so the
  agent can edit its own config (reset-to-default supported). Golden tools
  (`finish`, `ask_question`, `configure_agent`) can never be disabled.
- Persistent DISPLAY tab; play/stop terminal button; a "Stopped — what
  should I do instead?" prompt on cancel; loading spinner.

### Changed

- "agent" rail/tab renamed to "agents"; removed the send-$localharness
  modal; `submit_feedback` keeps feedback under the 2048-byte on-chain cap
  with a clear message; the agent knows the real internal filenames
  (history is `.lh_history.json`).

## [0.12.0] - 2026-05-28

Security + beta-readiness. A security audit closed a real XSS→wallet
vector and hardened cross-origin trust; sensitive OPFS files are now
encrypted at rest; and the beta golden path got the polish a first-time
user needs (phone support, onboarding, recoverable errors) plus a public
agent directory.

### Security

- **Markdown XSS fixed.** `rendered_markdown` passed raw HTML straight
  through and emitted `javascript:`/`data:` link targets verbatim. It
  renders model output + restored history, which a prompt injection can
  influence — an XSS into the wallet origin that chained to seed theft
  via the signer. Raw HTML now renders as escaped text and dangerous
  link/image schemes are stripped.
- **Cross-origin trust hardened.** The RPC endpoint trusted
  `starts_with("http://localhost")` (so `http://localhost.evil.com`
  passed), and signer/RPC/compose trusted localhost in production.
  Unified into a host-exact `is_trusted_lh_origin` (localhost honoured
  only in dev).
- **At-rest encryption.** `.lh_api_key` and `.lh_history.json` are
  encrypted with a per-origin AES-256-GCM key kept in localStorage
  (separate store from OPFS). Legacy plaintext is read transparently and
  re-encrypted on save. (Defense-in-depth for copy/export/disk channels;
  does not stop XSS. The wallet seed is intentionally left unencrypted
  pending a recovery design.)

### Added

- **Public agent directory** at `?explore=1` — a browsable gallery of
  every claimed agent, linked from the apex.
- **Touch input** for the display, so drag-based cartridges (drawing)
  work on phones/tablets.
- **Onboarding:** a "get a free key" link in the API key modal, and the
  key is validated on save (so a bad key is caught there, not mid-turn).
- **Publish payoff:** publishing an app on-chain now shows the live
  shareable subdomain link.
- **`design/launch-1.0.md`** — the grand plan for the 1.0 launch.

### Fixed

- A bad/expired Gemini key now reopens the key modal with a clear
  message instead of failing cryptically mid-turn.

### Internal

- Lint-clean on both native and browser-app/wasm targets (0 clippy
  warnings); removed retired dead templates; corrected the stale Tempo
  sponsorship-migration table in CLAUDE.md.

## [0.11.0] - 2026-05-28

The display release: a subdomain can now *be* an app. A pixel
framebuffer runs wasm cartridges (Redox/Orbital-style — the loader is
the compositor, the cartridge is the app), the rustlite compiler gained
host-import calls so agent-written Rust can draw, and a subdomain boots
straight into its published cartridge fullscreen.

### Added

- **DISPLAY framebuffer.** A `<canvas>` "screen" that runs wasm
  cartridges via `host_display`: cartridges write pixels and the host
  blits them — no DOM, no iframe. Cartridges export `frame(t)`
  (animated, driven by `requestAnimationFrame`) or `render()` (one-shot).
- **`host_display` ABI:** `clear`, `set_pixel`, `fill_rect`,
  `draw_char` / `draw_number` (hand-rolled 5×7 font: 0-9, A-Z, space,
  `+ - * / = . ( )`), `present`, `width`/`height`, pointer input
  (`pointer_x`/`pointer_y`/`pointer_down`, poll model), and a 64-slot
  integer `state_get`/`state_set` register file that persists across
  frames (rustlite has no globals).
- **rustlite → display bridge.** The compiler now emits wasm host-import
  calls: typecheck resolves `display::*` against a host-function table;
  codegen builds an import section and offsets local function indices.
  Agent-written rustlite cartridges, compiled in-browser, draw on screen.
- **`run_cartridge` tool.** The agent compiles rustlite and runs it
  directly on the display.
- **App mode.** A subdomain with an `app.rl` (rustlite source) in OPFS
  boots straight into a chrome-less fullscreen cartridge; `?edit=1`
  returns to the workshop.
- **Cross-visitor publishing.** The compiled cartridge wasm is stored
  on-chain in the registry diamond under
  `metadata(tokenId, keccak256("localharness.app.wasm"))` (no new facet)
  via a sponsored `setMetadata` tx. Every visitor — not just the owner's
  device — boots into the published app. Owner-only "publish app
  on-chain" button in admin.
- **Stop buttons.** A stop control halts a running cartridge's frame
  loop; the send arrow becomes a stop button while an agent turn streams
  and cooperatively cancels it (guarding against concurrent turns).
- **Feedback viewer.** The feedback modal lists recent on-chain
  `FeedbackSubmitted` events (newest first, relative timestamps) via
  `eth_getLogs` — previously feedback could only be submitted.

### Fixed

- **rustlite multi-local codegen bug.** `alloc_local` double-counted, so
  any function with 2+ locals emitted out-of-range local indices and
  failed to instantiate. Functions with 0-1 locals worked by luck; the
  emit tests never caught it because they only check the wasm header.
- **rustlite control-flow as statements.** `if`/`match`/block
  expressions now work as statements without a trailing `;` (like Rust);
  previously only `while`/`loop` did.
- **Transcript scroll** now lands at the latest turn on load (deferred
  past layout/font-swap).
- **Terminal-collapse anchor.** The terminal rail stays pinned to the
  bottom of the column when the terminal/view panels collapse instead of
  floating to the top.

## [0.10.28] - 2026-05-27

### Added

- **API key modal.** Centered overlay appears on mount when no Gemini
  API key is stored. Dismisses on save; no page refresh needed.
- **Compact button** in terminal. Triggers Gemini context compaction —
  summarizes old history, keeps recent 6 turns verbatim. Wired via
  `Agent::compact()` → `GeminiConnection::compact()`.
- **Clear button** in terminal. Resets transcript + agent state +
  deletes `.lh_history.json`.
- **`submit_feedback` tool.** Agents can submit feedback on-chain via
  the FeedbackFacet programmatically. Max 2048 bytes.
- **`llms.txt`** served at `localharness.xyz/llms.txt`. Agent-facing
  context: capabilities, RPC format, on-chain registry, conventions.
- **Documentation SOP** in CLAUDE.md — five surfaces, when to update
  what, single-source-of-truth rules.
- **Doc comments** on all ~190 public API items. Zero `missing_docs`
  warnings. 6 doctests (up from 2).

### Changed

- **Full monochrome palette.** No colored accents — pure black/white/
  grey scale with muted red for errors only. IBM Plex Mono font.
- **Chat turns** redesigned. Removed role labels ("USER"/"ASSISTANT"),
  stripped card backgrounds. 2px left border only (white=assistant,
  grey=user). 4px gap between messages. Terminal-like.
- **Admin panel** converted from position-absolute dropdown to centered
  fixed modal (560px, 80vh, scrollable). No more overflow/clipping.
- **Edit tab removed.** File editing still works from the files panel;
  the dedicated tab is gone from both desktop and mobile.
- **All panels collapsed by default.** Terminal + transcript is the
  primary UI. Files and agent rails expand on click.
- **Feedback modal** text updated ("submitted on-chain and saved
  locally" instead of "coming soon"). 60s client-side rate limit.
- **README overhauled.** 587→200 lines, user-facing, accurate tool
  count (15), architecture table, cargo features table.

### Fixed

- Transcript not scrolling to bottom on page load with restored
  conversation history.
- Tool-call blocks showing permanent "⋯ running" status after session
  restore (now show ✓ done or ✗ error).
- Auto-scroll during streaming — transcript follows new content.

## [0.10.27] - 2026-05-26

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
[Unreleased]: https://github.com/compusophy/localharness/compare/v0.31.0...HEAD
[0.31.0]: https://github.com/compusophy/localharness/compare/v0.30.0...v0.31.0
[0.1.0]: https://github.com/compusophy/localharness/releases/tag/v0.1.0
