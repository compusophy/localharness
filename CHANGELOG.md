# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- **OpenAI backend now retries a transient stream-open failure (#29 drift).** The gemini and
  anthropic turn loops retried a transient transport/5xx/timeout when OPENING the model stream;
  the openai loop shipped without that retry, so a single 503 aborted the whole turn. The
  retry-wrapped open is hoisted into the shared `backends::retry::open_stream_with_retry` and used
  by all three loops, so the policy can't drift per-backend again. (Auth/credits/rate-limit still
  fail fast; a mid-stream failure is never retried.)
- **CLI: `whoami --as <name>` no longer looks up the literal string `--as` on-chain.** `--as <name>`
  is accepted as an alias for the positional name, and any other unrecognized `--flag` errs with
  usage instead of being sent to the registry as a name.
- **CLI: the `call` x402 line said "paying … to the platform meter" while the debit actually came
  from the WALLET** (`X402Facet.settle` pulls via `transferFrom`; the meter is untouched). The
  message now states the wallet as the source. Message-only — billing behavior unchanged.
- **CLI: `discover` no longer folds flags into the search query.** `discover "writing help" --as
  claude` used to search for the literal string `writing help --as claude`; `--as` is now accepted
  and ignored (discover is identity-free) and any other `--flag` errs with usage.

### Added

- **CLI: `whoami` now shows the owner's `$LH` balances** (wallet + per-call meter — the same reads
  `credits` prints) in both the text and `--json` output.

### Changed

- Turn-loop phase A dedupe (roadmap R3): the byte-identical per-backend copies of
  `extract_canonical_path`, `resolve_tool_args`, and the `emit_error` free fns collapsed into the
  shared `backends::loop_util` / `LoopState::emit_error`. No behavior change outside the openai
  retry above.

## [0.60.20] - 2026-07-02

### Fixed

- **Notifications could be lost when the pending-push file was corrupted.** On reload
  `load_inbox` deleted `PENDING_FILE` even when its JSON failed to parse — permanently losing the
  closed-tab pushes it held; it now deletes only after a successful parse (keeping the file for a
  retry). And `stash_to_inbox` did `unwrap_or_default()` on a parse failure, then overwrote the
  file — silently discarding all prior stashed entries; it now surfaces the corruption instead of
  swallowing it.
- **OpenAI backend silently dropped a tool-call fragment with args but no name.** A truncated or
  malformed stream (args accumulated, name empty) was dropped via a bare `continue`, against the
  "never silently drop a part" policy. It now logs before skipping.

### Docs

- Corrected stale `COST_PER_REQUEST_WEI` / `MAX_COST_PER_REQUEST_WEI` defaults in `proxy/README.md`
  (were 0.01 LH / 1 LH; the code is 1 LH / 100 LH). (The 0.01 in `session.rs` is the separate x402
  ask price and is correct.)

## [0.60.19] - 2026-07-02

### Fixed

- **CLI `create` claimed the name mint was free when it charges ~1 LH.** The success line said
  "(free — the name mint is sponsored, you paid nothing)" while `register` pulls
  `registrationCost()` (~1 LH) from the wallet via transferFrom — contradicting the "claiming
  costs 1.00 LH" line right above it, and the seed of the recurring "told free, charged 1 LH"
  onboarding confusion. Verified live (a fresh create went 5.00 → 4.00 LH). It now says the
  mint's *gas* is sponsored but the name fee comes from your `$LH`.
- **SSE frames with invalid UTF-8 were silently dropped.** `extract_data_payload` did
  `from_utf8(frame).unwrap_or("")`, so a frame carrying invalid UTF-8 (a multi-byte char split
  across a network chunk, or backend corruption) was emptied — losing that frame's `data:`
  payload and truncating the stream. It now uses `from_utf8_lossy` (keeps the frame, replaces
  only the bad bytes).

### Docs

- Fixed a stale `COST_PER_REQUEST_WEI` comment in the proxy (said "Default 0.01 $LH" while the
  code + adjacent lines are 1 $LH).

## [0.60.18] - 2026-07-02

### Added

- **`onramp --settlement-tx <hash>` resumes a paid-but-unclaimed mint.** If the mint claim
  timed out waiting for a slow settlement confirmation, the user had already paid USDC.e but
  had no way to claim without paying again — and the error even promised an idempotent retry
  that didn't exist. Now `onramp --settlement-tx <hash> --pay <same usdce>` skips the
  challenge+payment and claims against the already-settled tx (the mint is idempotent, bound
  to that tx — no double-pay). The failure message prints the exact resume command.

### Fixed

- **Registration funding-deficit message floor-divided to a confusing "need 0 more LH".** A
  fractional shortfall (e.g. 0.5 LH short of a 1 LH cost) showed "need 0 more LH". It now
  rounds up.

### Docs

- Corrected a `display.rs` comment still citing the old `JOINER_RE` `{1,32}` (now `{8}`).

## [0.60.17] - 2026-07-01

### Fixed

- **Visitor could get a free turn on a priced agent during the load window.** The
  per-turn payment gate read `pricing_wei.unwrap_or(0)`, collapsing `None` ("pricing not
  checked yet — verification still running") into `0` ("free"). A visitor who sent within
  the few-second window before the price loaded hit the free-return path *before* the
  verify/TBA fail-closed checks, bypassing a priced agent's gate. It now fails closed until
  pricing is known (retry-in-a-moment), matching the function's other not-ready guards.

## [0.60.16] - 2026-07-01

### Fixed

- **Defense-in-depth confirmation on every value/authority agent tool.** `send_lh`,
  `batch_send_lh`, `spend_treasury`, `set_role`, and `attest` are all `CONFIRM_GATED` but
  lacked the belt-and-suspenders in-body confirmation check that `release_subdomain` /
  `publish_app_to` / `found_company` already carry. The dispatch-layer `confirm_guard` hook
  is the primary enforcement, but if a dispatch path ever skipped hooks, an *unconfirmed*
  `$LH` transfer / treasury spend / privilege grant / reputation write could have executed.
  Each now also rejects a call with no confirmation code in its own body.
- **Platform (proxy, deployed separately): mesh signaling roster could be hogged.** The
  WebRTC `JOINER_RE` accepted 1–32 chars, but a joiner id is always the first 4 bytes of the
  peer address (exactly 8 hex). One peer could register many different-length prefixes of its
  own address to fill the 8-slot mesh roster. Pinned to exactly 8.

### Docs

- Corrected the `tempo_tx` wire-format docstring: `sender_signature` is flat 65 bytes
  (`r‖s‖v`), not `rlp([v,r,s])` — matching the implementation, CLAUDE.md, and the golden vectors.

## [0.60.15] - 2026-07-01

### Fixed

- **A cartridge could OOM-crash the browser tab.** The display framebuffer copied the
  worker's transferred buffer into a `Vec` sized by the buffer length with no cap, so a
  worker bug or a compromised cartridge sending an oversized buffer would panic the whole
  tab on allocation. Bounded to the worker's 4 MiB max (1024×1024×4); oversized frames are
  dropped.
- **P2P shared-folder sync could silently lose data.** Two `apex_write` results were
  discarded — if preserving your *losing* edit as a conflict copy failed (OPFS full / I/O),
  the sync still pulled the peer's winner and overwrote your file with no backup. It now
  aborts the sync round on that failure (preventing the overwrite) and surfaces a
  received-file write failure instead of reporting a phantom successful pull.
- **`validation reclaim`/`draw` burned sponsor gas on the wrong state.** Neither checked
  the validation's on-chain state first, so a wrong-state poke wasted sponsored gas on an
  opaque revert. They now preflight (`reclaim` needs OPEN, `draw` needs CHALLENGED) with a
  clear message; a transient RPC read falls through so it never blocks a valid poke.
- **`party` accepted member id 0.** The numeric-id path took `0` (never a valid tokenId)
  while the name path rejected it — `formParty` would then revert cryptically on-chain. It's
  now rejected up front.

## [0.60.14] - 2026-07-01

### Fixed

- **Compaction drop-oldest fallback could fold in turns it never planned for.** The
  summarize-install path bails if history changed while `summarize()` awaited, but the
  drop-oldest fallback (taken on a summarization failure) lacked the same guard — so it
  applied a stale split position to a possibly-grown history. It now threads the expected
  length through and aborts identically on a race (regression tested).
- **bashlite: backslash-newline inside double quotes is now a line continuation.** An
  unquoted `\<newline>` was spliced, but inside double quotes both the backslash and the
  newline were preserved as literals. POSIX splices it in quotes too.

### Docs

- Clarified `AskUserHandler` / `Policy::ask` — the handler returns the decision
  (`true` = approve, `false` = deny), it doesn't itself prompt — with a compiling doctest.

## [0.60.13] - 2026-07-01

### Fixed

- **Headless `call`/`abtest` agents hallucinated their own chain.** The persona-only
  system prompt carried no runtime grounding (unlike the in-browser session), so asked
  what chain it runs on an agent answered "Arbitrum". A one-line grounding derived from
  the active `ChainConfig` (Tempo mainnet, chain 4217) now precedes the persona —
  correct on both mainnet and testnet. Found by live dogfooding.
- **A denied confirmation could be erased by a later approval in the same turn.** With
  multiple confirm-gated tools in one turn, an approved tool overwrote the
  "awaiting-confirmation" flag that an earlier *denied* one had set, so the loop
  auto-continued past the blocked call (re-issuing it, burning credits). The flag is now
  set-only within a turn and cleared on the next.
- **A mixed-case `--worker`/caller could judge its own colony work.** `select_judge_panel`
  and the judge==caller check compared identity names case-sensitively, so `--worker Claude`
  did not exclude the `claude` key — letting the worker onto its own neutral panel
  (self-inflated rating). Now case-insensitive, matching the sibling guards (regression
  tested).
- **Platform (proxy, deployed separately): concurrent metered calls 502'd on nonce
  collision.** Concurrent debits for the same meter key each auto-fetched the same
  pending nonce and collided — one landed, the rest were rejected "nonce too low". The
  proxy now passes an explicit pending nonce and retries only on a nonce-too-low
  rejection (which never lands, so it can't double-debit).

### Docs

- Fixed broken `classify` intra-doc links in `error_codes` (`cargo doc` warning-free).

## [0.60.12] - 2026-07-01

### Fixed

- **`notify --to` reported a delivered cross-agent note as a failure.** The proxy
  always records a `--to` note in the recipient's on-chain inbox (real delivery —
  it surfaces in their bell) and meters it; `enrolled` only flags whether a live
  Web-Push *device* exists. But the CLI printed `enrolled:false` as "the note did not
  reach them" and its docstring claimed "the sender is not charged" — both wrong (the
  note landed and the sender was billed). It now reports the inbox delivery accurately.
  Found dogfooding cross-agent notify.

## [0.60.11] - 2026-07-01

### Fixed

- **`call` silently dropped a `--pay` placed after the target.** `--pay` / `--model` /
  `--verify` / `--fresh` are leading flags (before the target, so they aren't parsed
  out of the message), but `--as` is position-independent — so a user who naturally
  wrote `call agent "msg" --pay auto` had the flag *joined into the message text and
  ignored*, paying the agent **nothing** with no warning. It now errors clearly
  ("`--pay` must come BEFORE the target"). Found dogfooding the x402 pay-per-call path
  (which otherwise works: a leading `--pay auto` settles to the agent's TBA on-chain).

## [0.60.10] - 2026-07-01

### Fixed

- **`<command> --help` no longer tries to claim "--help" as a subdomain.** `publish
  --help` (and every name-first command — create/persona/price/whoami/…) treated the
  flag as a positional name and errored "invalid name '--help'". A generic guard now
  routes `<single-word-command> --help`/`-h` to the command list; two-word commands
  (`colony run --help`) keep their own per-command help. Found dogfooding the
  publish→live-URL flow.

## [0.60.9] - 2026-07-01

SDK ergonomics wins from a dedicated `cargo add localharness` consumer-lens review
(all additive / non-breaking).

### Fixed

- **`#[must_use]` on `ChatResponse` + the config builders.** Dropping a `ChatResponse`
  silently discarded the turn's streamed output, and a bare `cfg.with_model("x");`
  statement vanished (the `*AgentConfig` builders return `Self`, not `&mut self`) — both
  now warn.
- **The safety-guard error suggested a function that doesn't exist.** It said
  `policy::allow("tool_name")`, but that's the associated fn `policy::Policy::allow` — so
  copy-pasting the *unblocking* error message was itself a compile error (`E0425`). Fixed.
- **Doc drift.** `GeminiAgentConfig::with_filesystem` said "6 fs built-ins"; it's 8
  everywhere else.

## [0.60.8] - 2026-07-01

### Fixed

- **Clarify an agent's ask-price vs the inference meter (top dogfood confusion).** A
  persona-cohort dogfood repeatedly flagged "told 0.01 LH but charged 1.00 LH": `whoami`
  showed an agent's advertised x402 price bare, and callers conflated it with the ~1 LH
  per-message inference meter. The price line (whoami + `call --pay auto`) now labels it
  as the agent's `--pay` ask price, explicitly *separate from the ~1 LH model meter*.
  Messaging only — no economics change.

## [0.60.7] - 2026-07-01

### Fixed

- **CLI create funding-wall message showed a double unit.** `fmt_lh` already appends
  "LH", so the claim-funding error rendered `costs 1.00 LH $LH` — the first thing a
  fresh, unfunded identity sees. Dropped the redundant " $LH" to match the CLI-wide
  `fmt_lh` convention. Found dogfooding onboarding on mainnet.

## [0.60.6] - 2026-07-01

### Fixed

- **`set_persona` / `set_lessons` on-chain publish is best-effort.** `set_persona`
  published on-chain *before* saving the edit locally, so a relay/network failure lost
  the persona self-edit entirely (the same #34 data-loss class fixed for `record_lesson`
  in 0.60.3, but the sibling self-edit tools were missed). Both now save locally first
  and degrade to a deferred-publish success instead of hard-erroring.
- **Colony default `--min-accept-rating` raised 2 → 3.** Running the colony, below-bar
  work (e.g. non-compiling code a judge sympathy-grades at 2/5) still cleared the
  default payment gate and got paid from escrow. The default now rejects medians 1–2;
  operators can still opt into a lenient bar explicitly.

## [0.60.5] - 2026-07-01

Three skeptic-verified correctness fixes in the less-trafficked cores.

### Fixed

- **`parse_hex_quantity` accepts an uppercase `0X` prefix.** It was the one hex codec
  stripping only `0x` (siblings strip both cases), so an RPC or dapp-supplied EIP-1559
  field with a `0X` prefix errored the whole parse — contradicting its documented
  "optional `0x` prefix" contract.
- **`native_balance` errors on a malformed `eth_getBalance` result** instead of
  coercing a non-string RPC response to a real-looking zero balance — matching the
  sibling `eth_call` read and the reject-not-truncate convention.
- **soliditylite CODECOPY wraps its source offset.** The copy loop used unchecked
  `src + i`; hostile bytecode could set `src` near `usize::MAX` and overflow-panic in
  debug builds (how `cargo test` and the diff-harness run), breaking the "untrusted
  bytecode never panics the process" invariant. It now wraps like its CALLDATACOPY
  sibling (with a regression test).

## [0.60.4] - 2026-07-01

### Fixed

- **A user pre-tool-call hook now satisfies the safety-policy guard.** The startup
  safety check enforces "write/custom tools require a policy **or** a user-installed
  pre-tool-call hook", but it inspected the still-empty `HookRunner` (hooks are
  registered just *after* the check), so the hook branch was dead — an SDK consumer
  who wired `with_pre_tool_hook(...)` and no explicit policy was wrongly rejected with
  "no safety policies are configured". It now inspects the config's `pre_tool_hooks`,
  restoring the documented contract (with a `start_mock` regression test).

## [0.60.3] - 2026-07-01

Three skeptic-verified fixes from an autonomous growth tick.

### Fixed

- **`record_lesson` on-chain publish is best-effort (telemetry #34).** The lesson is
  saved locally first, but a sponsored `setMetadata` failure (e.g. an unfunded wallet
  can't pay gas) then hard-errored the whole tool call and lost it. It now degrades to
  a local-only success (`recorded: true`, null `tx_hash`, deferred note) so a chain
  hiccup never drops an already-saved lesson.
- **Colony `--worker` self-deal guard.** The auto-pick path excludes the caller as its
  own worker, but the explicit `--worker` override bypassed that — a caller could force
  itself as its own worker and collect its own escrowed reward. Rejected up front,
  mirroring the existing `--judge == worker` guard.
- **Fail-closed `receiptUsed` read.** The fiat on-ramp mint gate coerced any u256
  decode failure to `false` ("receipt unused") via `unwrap_or(false)` — a fail-open
  default; it now propagates the decode error with `?` like its sibling reads.

## [0.60.2] - 2026-07-01

Three small, individually-verified fixes from an autonomous growth tick —
real-money onboarding recovery and two identity-at-rest safety hardenings.

### Fixed

- **Onramp mint-claim retries transient 5xx.** After the buyer has already
  self-paid USDC.e on-chain, the `$LH` claim loop only retried a 402 — a transient
  proxy 5xx made it give up and strand the paid-for credit. The claim is idempotent
  per settlement tx, so 5xx are now retried too (2xx/4xx stay terminal).
- **`list_directory` hides protected seed files case-insensitively.** It was the one
  fs builtin still filtering via a raw case-sensitive check; it now routes through the
  shared `is_protected_basename` guard, so `.lh_wallet` and friends stay hidden on
  Windows/macOS (with trailing dot/space folding) like every other fs builtin.
- **`EncryptedFilesystem::is_exempt` folds case on Windows/macOS.** `.LH_WALLET` and
  `.lh_wallet` are the same on-disk file there, so a differently-cased write path
  could have sealed the seed and bricked identity; the exempt check now matches
  case-insensitively on those platforms (Linux stays byte-exact).

## [0.60.1] - 2026-07-01

Two follow-up fixes on top of 0.60.0's hardening batch.

### Fixed

- **Pricing messaging.** The onboarding tips, CLI help / hints, doc comments and
  test-fleet scripts quoted "~0.01 `$LH`" per call, but the meter has charged
  1 `$LH` per message since 0.47.0 — users were shown a price 100× below what
  they were billed (found dogfooding). Corrected to ~1 `$LH` (0.01 remains only
  the x402 agent-advertised default, a separate mechanism); the regression guard
  now scans the CLI tips as well as `skill.md`.
- **x402 zero-address settlement.** `settle_x402_sponsored` now rejects a `0x0`
  recipient before building the tx, so a caller can't irrecoverably burn the
  payer's `$LH` — mirroring the guild-treasury guard and the already-shipped
  x402 nonce-state zero-address guard.

## [0.60.0] - 2026-07-01

Security + real-money hardening across the untrusted-input, on-chain, and metering
surfaces, with the autonomous-business / colony / marketing layer maturing. A large
batch of small, individually-verified fixes — most carrying a regression test.

### Security / hardening

- **Untrusted-input bounds.** Cap hex parsing (`MAX_HEX_LEN`), `create_file` /
  `edit_file` content + read + post-replace output, bashlite stdout / stderr / loaded
  script size, and the soliditylite interpreter memory + stack (EVM 1024-item parity)
  — so attacker-supplied input can't OOM the process or tab.
- **At-rest identity safety.** `EncryptedFilesystem` no longer seals `.lh_wallet` when
  a rename crosses the EXEMPT boundary, and `is_exempt` handles trailing separators —
  closing two paths that could brick an identity by encrypting its own seed.
- **Fail-closed on-chain reads.** The x402 nonce-replay check and
  `has_validated` / `has_attested` propagate an RPC/decode error instead of silently
  reading a malformed word as `false` (which defeated the replay + dedup guards).
- **Guild treasury** rejects a zero-address payout (was able to burn pooled `$LH`).

### Fixed

- **Metering.** The credit proxy keeps `max_tokens > thinking.budget_tokens` when the
  spend cap caps output (telemetry #38 — a live Anthropic HTTP 400 that crashed
  turns); `onramp claim_mint` now backs off + retries on the 402 settlement window.
- **Agent loop.** `dispatch` lifts non-string tool `{error}` values (were silently
  dropped); `error_codes::classify` no longer mislabels a transport `truncated` error
  as empty; `turn_flow` treats a content-block stop as terminal before max-tokens;
  `conversation` won't let whitespace-only text clobber the last answer; compaction
  dedups its drop-oldest fallback note.
- **Onboarding.** `onboard` distinguishes a balance-read failure from a real zero
  balance; empty terminal cards surface the exit code and label a missing argv.
- **Colony.** Judges score against ground-truth `rustlite::compile` evidence,
  worker/judge selection is gated to active-chain-registered agents, and repro
  extraction is broadened (bare lines, attrs, crash axis).
- **Registry.** Bridge the `$LH` allowance in `create_guild_sponsored`; the relay
  sponsors the bounty/attest lifecycle and small `setMetadata` self-edits for funded
  callers; `--pay` self-paid founding routes around the relay funded-gate.

### Added

- **Marketing / self-sovereign reach.** An autonomous Nostr broadcaster + SETI, an
  ERC-8004 agent-card, and a from-scratch SMTP client for agent email, plus a mainnet
  founding runbook.
- **Repo practices.** `rust-code-reviewer` + `rust-api-ergonomics-reviewer` subagents
  (adapted from anthropics/buffa) and a substantive, sectioned README.

## [0.59.0] - 2026-06-30

The **autonomous-business** layer: compose an on-chain "company" of role-agents and
inspect its entire work cycle locally — read-only — before anything touches the chain.
Built by composing existing economy primitives; no new on-chain surface.

### Added

- **`found_company` agent tool + `company found` CLI.** Stand up a business as a
  `GuildFacet` guild (org identity + pooled `$LH` treasury TBA) with N role-agent
  subdomains (coder, reviewer, PM, executive, accounting, HR, marketing), each with an
  on-chain persona — one call composing only existing sponsored helpers. Value-moving, so
  it is typed-confirmation **and** allowlist gated (like `set_persona`); the CLI mirrors
  it and previews a broadcast-free plan unless `--confirm` is passed.
- **`company` CLI — a read-only window onto a company.** `status`, `plan` (dry-run the
  next work cycle off live chain reads), `payroll` (suggested split, no transfers),
  `books` (net position / runway / breakeven), `day` (one-shot daily report), and
  `forecast` (multi-cycle projection). None of these sign, broadcast, or move `$LH`.
- **`set_role` + `attest` agent tools** — typed-confirmation-gated wrappers over the
  guild-role and reputation facets.
- **Five pure decision cores** (native + wasm, dependency-free, ~80 unit tests):
  `work_cycle` (assign→judge→pay→attest modeled as data), `work_cycle_runtime` (previews
  a cycle's actions, never executes), `accounting` (honest seed-vs-earned economics —
  net position / runway / break-even), `hiring` (role-fit ranking; the single
  implementation behind `work_cycle::assign_next_task`), and `simulation` (multi-cycle
  forward forecast).
- **`examples/autonomous_company.rs`** — a runnable, pure demo of the full loop (hiring →
  work-cycle preview → honest books), no keys or chain access required.

### Fixed

- A `simulation` test used an always-true comparison (`x <= u128::MAX`) that tripped
  `clippy::absurd_extreme_comparisons` under `-D warnings`; replaced with a meaningful
  saturation assertion.

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
