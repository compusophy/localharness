# Unused / Legacy Code Debt Report - 2026-06-21

> ⚠️ HISTORICAL SNAPSHOT (note added 2026-06-27). Much of this has since been
> actioned — see the overnight tech-debt loop and `design/cleanup-backlog.md`. Treat
> the findings below as a point-in-time snapshot, not a live TODO: VERIFY each item
> against current source before acting on it.

## Executive summary

The repo is not failing from obvious Rust warnings: `cargo check` is clean, and
`cargo check --all-targets --all-features` reports only one unused function
(`src/landing.rs::ios_unavailable`). The real problem is quieter:

- obsolete systems remain tracked as source, docs, and deployment scripts;
- feature gates and public exports hide dead code from normal compiler warnings;
- broad `#[allow(unused_imports)]` suppressions in the CLI remove an important
  cleanup signal;
- docs repeatedly describe removed, shelved, historical, or parked systems;
- the existing cleanup backlog is real but too small and not wired into a
  removal process.

In short: the unused-code problem is mostly governance and surface-area debt,
not one giant compiler-warning pile. The codebase can accumulate trash because
there is no default rule that dead, parked, or historical code must either be
removed, moved to an archive, or justified with an owner and expiry.

## Checks performed

- `cargo check --message-format=short`
  - Result: clean.
- `cargo check --all-targets --all-features --message-format=short`
  - Result: one warning:
    - `src/landing.rs:122`: `function ios_unavailable is never used`.
- `rg` scan for `allow(...)`, `dead_code`, `unused`, `deprecated`, `legacy`,
  `shelved`, `removed`, `parked`, and related terms.
- Tracked-file inventory with `git ls-files`.
- Existing cleanup backlog review: `design/cleanup-backlog.md`.

## Main finding: warnings are being suppressed, not enforced

There are 83 `allow(...)`-style suppressions across tracked source/test/example
surfaces found by the scan. Many are reasonable local exceptions
(`clippy::too_many_arguments` on low-level raster/compositor primitives, ABI
non-snake-case names, wasm-specific `Arc` behavior), but the pattern is too
permissive in the CLI.

The biggest smell is the CLI module pattern:

- `src/bin/localharness/bounty.rs`
- `src/bin/localharness/abtest.rs`
- `src/bin/localharness/call.rs`
- `src/bin/localharness/credits.rs`
- `src/bin/localharness/colony.rs`
- `src/bin/localharness/buy.rs`
- `src/bin/localharness/facet.rs`
- `src/bin/localharness/guild.rs`
- `src/bin/localharness/identity.rs`
- `src/bin/localharness/invite.rs`
- `src/bin/localharness/mcp.rs`
- `src/bin/localharness/models.rs`
- `src/bin/localharness/notify.rs`
- `src/bin/localharness/party.rs`
- `src/bin/localharness/probe.rs`
- `src/bin/localharness/publish.rs`
- `src/bin/localharness/reputation.rs`
- `src/bin/localharness/schedule.rs`
- `src/bin/localharness/session.rs`
- `src/bin/localharness/status.rs`
- `src/bin/localharness/tba.rs`
- `src/bin/localharness/util.rs`
- `src/bin/localharness/validation.rs`
- `src/bin/localharness/vote.rs`

Most of these start with `#[allow(unused_imports)]` and then `use crate::*;`.
That pattern makes refactors cheaper in the moment but expensive over time:
removing a helper, command, or shared import no longer creates compiler pressure
to simplify the module.

Recommendation: replace `use crate::*` with explicit imports module by module,
then remove `#[allow(unused_imports)]`. Do this incrementally, one CLI command
family at a time.

## Expanded findings: SSOT and duplication debt

The initial dead-code scan understates the debt. The wider risk is drift between
parallel implementations and hand-maintained "single sources of truth" that are
only single within one layer.

### 1. Pricing has multiple live representations

Runtime and docs disagree about the default request price:

- `proxy/api/_prices.ts` defaults `COST_PER_REQUEST_WEI` to
  `1_000_000_000_000_000_000n` (1 `$LH`).
- `proxy/api/fetch.ts` defaults `COST_PER_REQUEST_WEI` to
  `10000000000000000` (0.01 `$LH`).
- `proxy/api/notify.ts` defaults `COST_PER_REQUEST_WEI` to
  `10000000000000000` (0.01 `$LH`).
- `proxy/api/scheduler.ts` defaults `COST_WEI` via `COST_PER_REQUEST_WEI ??
  10000000000000000` (0.01 `$LH`).
- `proxy/README.md` still documents `COST_PER_REQUEST_WEI` default as 0.01 LH
  and `MAX_COST_PER_REQUEST_WEI` default as 1 LH.
- `src/bin/localharness/main.rs` says the proxy default is 1 `$LH`.
- `src/docs_manifest.rs` says docs pricing is kept in sync with
  `proxy/api/_prices.ts` "by hand".

This is not just cleanup debt; it can become billing drift. Model calls, web
fetch, notifications, and scheduled runs are all described as "same price as a
model call", but their defaults are not actually the same unless the production
environment overrides all of them consistently.

Recommendation: make `proxy/api/_prices.ts` export the default flat price and
import it from `fetch.ts`, `notify.ts`, and `scheduler.ts`, or split explicit
capability prices into one shared `proxy/api/_metering.ts`. Docs should be
generated from the same table or from a small JSON manifest consumed by both
Rust docs and TypeScript.

### 2. Model tables are fragmented

Model IDs appear in several places:

- `src/types.rs` - Gemini default.
- `src/backends/anthropic/wire.rs` - Anthropic IDs.
- `src/backends/openai/wire.rs` - OpenAI IDs.
- `src/app/model.rs` - browser-selectable subset.
- `src/bin/localharness/models.rs` - CLI advertised list.
- `src/difficulty.rs` - consult-model routing/allowlist.
- `proxy/api/_prices.ts` - flat model price table.
- `proxy/api/_usage.ts` - token-rate table.
- `proxy/api/scheduler.ts` and `proxy/api/mcp.ts` - default run/ask model.
- generated docs via `src/docs_manifest.rs`, plus hand-written design docs.

Some tests pin CLI IDs against backend constants when features are enabled,
which is good. But the proxy price table, token-rate table, browser selector,
consult-model allowlist, and docs can still drift from each other.

Recommendation: introduce a versioned model catalog with fields such as
`provider`, `id`, `label`, `selectable_in_browser`, `cli_visible`,
`consult_allowed`, `flat_price_wei`, and optional `token_rates`. Generate:

- Rust backend constants/tests;
- browser selector;
- CLI `models`;
- proxy price table;
- docs pricing/model blocks.

### 3. Chain config is split Rust/proxy/docs

There is a good Rust-side chain seam in `src/registry/chain.rs` and a proxy seam
in `proxy/api/_chain.ts`, but they are separate sources with duplicated default
addresses. The docs also repeat the diamond/token/RPC constants in generated and
hand-written areas.

Evidence:

- `src/registry/chain.rs` holds `MODERATO` and `MAINNET`.
- `proxy/api/_chain.ts` holds `TEMPO_RPC`, `REGISTRY`, `CHAIN_ID`, `LH_TOKEN`.
- `contracts/README.md`, `AGENTS.md`, `CLAUDE.md`, `web/llms.txt`, design docs,
  and many Foundry script comments repeat the same diamond address.

Recommendation: keep Rust as the canonical source if that is the intended
owner, but generate a proxy JSON/TS config artifact from it during release, or
move both Rust and proxy to a checked-in `chain-config.json` consumed by codegen.
At minimum, add a test that compares `src/registry/chain.rs` defaults against
`proxy/api/_chain.ts`.

### 4. Agent docs are near-duplicates

`AGENTS.md` and `CLAUDE.md` are both 563 lines and mostly the same file with
agent-name substitutions. They already drift in wording:

- `Claude Messages API backend` vs `Codex Messages API backend`
- `Gemini/Claude/OpenAI` vs `Gemini/Codex/OpenAI`
- references to `CLAUDE.md` vs `AGENTS.md`

This is an obvious doc-generation candidate. Keeping both hand-edited means
every architecture update has to be applied twice.

Recommendation: make one source template plus a small target profile
(`agent_name`, filename, wording overrides), then generate both files. Or pick
one canonical file and make the other a short pointer.

### 5. Proxy route security/metering code is copied

Several proxy routes repeat the same primitives:

- origin/CORS allow checks;
- `localharness-proxy:<address>:<timestamp>` personal-sign recovery;
- freshness windows;
- `eth_call` helpers;
- `creditOf` reads;
- `meter` calls;
- chain construction with `defineChain`;
- payload-size guards.

Affected routes include:

- `proxy/api/gemini.ts`
- `proxy/api/fetch.ts`
- `proxy/api/notify.ts`
- `proxy/api/broadcast.ts`
- `proxy/api/mcp.ts`
- parts of `proxy/api/scheduler.ts`

There are helper modules (`_chain`, `_prices`, `_ratelimit`, `_webpush`,
`_x402`, `_tempo`, `_stripe`), but the security-critical request auth and meter
gate are still largely route-local. That makes it easy for one route to retain
old prices, old CORS rules, or stale auth behavior.

Recommendation: extract a proxy auth/meter package:

- `parseAuthToken(req)`;
- `recoverLocalharnessSigner(token)`;
- `requireFreshTimestamp`;
- `corsForOrigin`;
- `ethCall`;
- `creditOf`;
- `debitMeter`;
- `gatePaidCapability({ capability, costWei })`.

Then route files should only describe their own payload and upstream behavior.

### 6. Registry client repeats ABI/call patterns

`src/registry` has a central `abi.rs` and `rpc.rs`, but individual facet modules
still contain many local encoders and sponsored-call wrappers. The inventory:

- `src/registry/names.rs` - 944 lines, 16 encoder functions.
- `src/registry/tba.rs` - 856 lines, 10 encoder functions, 54 `sponsored`
  mentions.
- `src/registry/bounty.rs` - 690 lines.
- `src/registry/weighted_voting.rs` - 542 lines.
- `src/registry/guild.rs` - 538 lines.
- `src/registry/signaling.rs` - 523 lines.
- `src/registry/x402.rs` - 476 lines.
- `src/registry/voting.rs` - 429 lines.
- `src/registry/party.rs` - 427 lines.

This may have been the right move while facets churned, but it is now a
boilerplate field. Every new facet tends to add another copy of:

- hand-encoded selector and ABI layout;
- `read_view(selector(...), words)` calls;
- sponsored transaction wrapper;
- cursor/list decode logic;
- status enum label logic.

Recommendation: introduce a small generated or declarative ABI layer for the
facet client. Even a local macro/helper for "static args + dynamic bytes/string"
and a generic `FacetCall<T>` decoder would reduce copy-paste without adopting a
large dependency.

### 7. Backend implementations share shape but not enough code

The Gemini, Anthropic, and OpenAI backends each have similar structures:

- config builders;
- `Connection` implementations;
- history serialization;
- transcript projection;
- compaction adapters;
- tool declaration building;
- stream loop state machines;
- thinking/model override plumbing.

There is already shared code in `src/backends/{sse,dispatch,runners,compaction,
stream_timeout}.rs`, which is a good direction. But `gemini/mod.rs`,
`anthropic/mod.rs`, and likely `openai/mod.rs` still duplicate connection
lifecycle and transcript concepts.

Recommendation: do not force one generic backend abstraction prematurely, but
extract narrow "boring sameness" pieces:

- history bytes encode/decode trait;
- transcript projection helpers;
- common connection state container;
- shared tests for tool-call transcript projection across providers.

### 8. Browser app event handling has grown into a dispatch monolith

`src/app/events/mod.rs` is 1,424 lines and owns a large action enum, parsing,
and dispatch. This matches the project rule of one delegated listener, but the
module is still a high-churn center. Many handlers spawn local futures directly,
and related constants/actions are distributed across `templates`, `events`,
`chat`, and app state.

Recommendation: keep the single delegated listener rule, but split action
definitions by domain and generate/compose the dispatcher table. At minimum,
add a test that every `data-action` emitted by templates is parseable by
`Action::parse`, and every parsed action has a dispatch arm.

### 9. CSS/style source of truth is incomplete

`web/styles.css` says root tokens come from `src/app/style.rs`, but it remains a
2,424-line stylesheet with many hand-maintained rules. A legacy alias remains:

- `web/styles.css`: `button.ghost { /* legacy alias - same as base now */ }`

Recommendation: either generate the token block only and document the boundary,
or move more reusable component styles into a generated style manifest. Delete
legacy aliases once template usage is gone.

### 10. Legacy/back-compat files continue to define product behavior

Several local files exist primarily for back-compat and can hide old concepts:

- `.lh_pricing.json` via `src/app/pricing.rs`;
- `.lh_device_key` via `src/app/wallet_store.rs`;
- cwd `*.localharness.key` legacy reads in CLI identity paths;
- `.lh_owner` hint behavior;
- old session/meter bridge language across docs and code.

Recommendation: create a "compatibility ledger" with each legacy file/path,
reader, writer, migration status, and deletion condition. Without that, nobody
knows which files are still intentionally supported.

## Code quality / refactor hotspots

These files deserve focused review beyond dead-code removal:

- `proxy/api/scheduler.ts` - long autonomous money-moving worker; pricing and
  model-cost assumptions need SSOT.
- `proxy/api/mcp.ts` - x402 verification, settlement, discovery, and model call
  logic in one 1,500-line route.
- `proxy/api/gemini.ts` - multi-provider router still named `gemini.ts`, which
  obscures ownership.
- `src/app/templates.rs` - large template module with some dead pricing pieces.
- `src/app/events/mod.rs` - action parsing/dispatch monolith.
- `src/app/chat/tools/misc.rs` - many unrelated tools in one file.
- `src/bin/localharness/main.rs` - command docs, dispatch, constants, and shared
  re-exports in one place.
- `src/bin/localharness/colony.rs` - 1,664-line CLI workflow; likely needs
  domain structs and smaller pure cores.
- `src/registry/tba.rs` and `src/registry/names.rs` - broad modules with many
  encoders/readers/writers.
- `web/llms.txt` - 372-line agent-facing spec with massive paragraphs; easy to
  drift even with generated blocks.

## New concrete warnings from Clippy

`cargo clippy --all-targets --all-features --message-format=short` reports:

- `src/landing.rs:122`: unused `ios_unavailable`.
- `src/soliditylite/codegen.rs:1460`: doc list item without indentation.

Neither is severe, but both show the all-target/all-feature lint is useful and
should run in CI.

## Tooling / guardrail gaps

The codebase has some good tests, but cleanup-specific guardrails are thin:

- `cargo machete` is not installed in this workspace, so unused dependency
  checks are not readily available.
- `cargo udeps` is not installed either.
- `proxy/tsconfig.json` is strict and `npx tsc --noEmit` passes, but
  `proxy/package.json` has no `typecheck` script, so the clean check is not
  discoverable via `npm run`.
- `Cargo.toml` has no `[lints]` policy for warning levels or restricted
  `allow(...)` patterns.
- The generated-doc drift gate exists for managed blocks, but it does not cover
  `AGENTS.md` / `CLAUDE.md`, most design docs, proxy README, or Foundry script
  comments.
- There is no apparent automated check that template `data-action` strings
  match the browser `Action` parser/dispatcher.
- There is no automated check that proxy capability prices match the model
  pricing source.

Recommendation:

- Add `npm run typecheck` in `proxy/package.json`.
- Add a `scripts/audit-tech-debt` or `cargo xtask debt` that runs:
  `cargo check --all-targets --all-features`, `cargo clippy --all-targets
  --all-features`, `npx tsc --noEmit`, doc drift checks, and regex gates for
  broad `allow(...)`.
- Consider adding `cargo machete` in CI or as an optional local command.
- Add a custom test for proxy/Rust config drift: chain IDs, addresses, model
  catalog, and pricing defaults.

## High-confidence cleanup candidates

These are already documented as obsolete or parked and still remain as tracked
code/scripts/docs.

### 1. PairingFacet lineage

Status: removed on-chain, superseded by QR seed adoption, still retained in
source/scripts.

Tracked files:

- `contracts/src/facets/PairingFacet.sol`
- `contracts/script/AddPairingFacet.s.sol`
- `contracts/script/AddPairingFacetV2.s.sol`
- `contracts/script/RemovePairingFacet.s.sol`

Evidence:

- `AGENTS.md` and `contracts/README.md` say `PairingFacet` is removed.
- `design/cleanup-backlog.md` explicitly lists dormant PairingFacet references
  as cleanup.
- Changelog says PairingFacet routing was cut and old browser flow was removed.

Recommendation: archive or delete the facet and add scripts. Keep only
`RemovePairingFacet.s.sol` if it is still needed for reproducing old deployments;
otherwise move the whole lineage to a historical archive outside active
`contracts/src` and `contracts/script`.

### 2. Legacy flat registry / legacy deploy

Status: historical reference only.

Tracked files:

- `contracts/src/LocalharnessRegistry.sol`
- `contracts/script/Deploy.s.sol`

Evidence:

- `contracts/README.md` says the flat registry is historical only and abandoned
  after the reset.
- Live architecture is the EIP-2535 diamond.

Recommendation: move to `contracts/archive/` or delete. Keeping it beside live
contracts makes scans and mental models noisier.

### 3. BootstrapFaucet lineage

Status: documented dormant/broken legacy path.

Tracked files:

- `contracts/src/BootstrapFaucet.sol`
- `contracts/script/DeployBootstrapFaucet.s.sol`

Evidence:

- `design/mainnet-deploy-runbook.md` says to skip `DeployBootstrapFaucet`
  because it is broken.
- Source comments call it dormant after Tempo sponsorship.

Recommendation: delete or archive with an explicit "do not deploy" README.

### 4. `CreditMeterFacet.chargeFromWallet`

Status: known rejected path.

Tracked files:

- `contracts/script/AddChargeFromWallet.s.sol`
- `contracts/src/facets/CreditMeterFacet.sol`
- `contracts/test/CreditMeterFacet.t.sol`

Evidence:

- `design/cleanup-backlog.md` says this was added for a wallet-primary billing
  direction that was rejected and is inert.

Recommendation: include in the next diamond cleanup cut. Remove selector,
source method, script, and tests together after live cut coordination.

### 5. SessionFacet / coarse sessions

Status: shelved, but still partly live/documented as a compatibility path.

Tracked files:

- `contracts/src/facets/SessionFacet.sol`
- `contracts/script/AddSessionFacet.s.sol`
- `src/bin/localharness/session.rs`
- `src/registry/credits.rs`
- proxy/docs references to session gating

Evidence:

- `AGENTS.md`, `contracts/README.md`, and `design/cleanup-backlog.md` say
  metering is the live path and sessions are shelved.
- Proxy and CLI still reference session gating, so this is not safe to delete
  until compatibility requirements are decided.

Recommendation: make a product decision:

- keep sessions as supported compatibility and remove "shelved" language, or
- remove the CLI command, registry helpers, proxy gate, facet, and docs in one
  coordinated cleanup.

Half-keeping it is what creates debt.

### 6. Browser pricing UI remnants

Status: UI removed/hidden historically, but code remains.

Tracked files:

- `src/app/pricing.rs`
- `src/app/events/layout.rs` pricing handler paths
- `src/app/templates.rs` has dead-code pricing template functions

Evidence:

- `src/app/templates.rs` marks pricing card pieces as `#[allow(dead_code)]`.
- Changelog says pricing UI was removed/hidden in earlier passes.

Recommendation: decide whether owner price editing belongs in the current admin
surface. If not, delete the old UI code and keep only the active x402 price
read/write path. If yes, remove `dead_code` suppressions by wiring the template
back in intentionally.

### 7. OpenAI backend

Status: unclear. The cleanup backlog says "PARKED", but current docs/proxy code
advertise GPT model routing.

Tracked files:

- `src/backends/openai/*`
- `src/bin/localharness/models.rs`
- `proxy/api/_prices.ts`
- docs generated from `src/docs_manifest.rs`

Evidence:

- `design/cleanup-backlog.md` lists the OpenAI backend as parked.
- Current docs and proxy code mention GPT model IDs and pricing.

Recommendation: resolve the contradiction before deleting. Either:

- mark OpenAI as supported and remove it from the cleanup backlog, or
- remove SDK/backend/docs/proxy references together.

## Documentation debt

The repo has large historical docs that are useful but blur active truth:

- `docs/CHANGELOG-archive.md` is 4,256 lines.
- `docs/TESTING-0.24.0.md` is old and still mentions deprecated web-sys methods.
- `design/stripe-mainnet.md` and `design/custody-security.md` still describe
  chargeback-lock machinery that `design/cleanup-backlog.md` now says should be
  stripped.
- `contracts/README.md`, `AGENTS.md`, `web/llms.txt`, and generated docs repeat
  active/legacy status in several places.

Recommendation:

- Keep historical docs, but move old implementation runbooks under
  `design/archive/` or `docs/archive/`.
- Add a short "Active architecture only" doc that refuses historical caveats.
- Require any doc phrase like `REMOVED`, `SHELVED`, `PARKED`, `legacy`, or
  `historical only` to point at a cleanup issue/backlog entry.

## Large active files that amplify cleanup risk

These are not necessarily dead, but they are large enough that unused code can
hide inside them:

- `src/app/templates.rs` - 2,532 lines
- `web/styles.css` - 2,424 lines
- `proxy/api/scheduler.ts` - 2,040 lines
- `src/soliditylite/codegen.rs` - 2,038 lines
- `src/app/display.rs` - 1,968 lines
- `src/rustlite/parser.rs` - 1,767 lines
- `src/bin/localharness/colony.rs` - 1,664 lines
- `web/cartridge-worker.js` - 1,627 lines
- `src/app/chat/tools/misc.rs` - 1,607 lines
- `src/agent.rs` - 1,544 lines

Recommendation: treat cleanup in these files as extraction/removal campaigns,
not opportunistic edits. Add local tests before cutting behavior.

## Generated / local bulk

Filesystem scans show large local directories/artifacts:

- `proxy/node_modules/`
- `contracts/out/`
- `target/`
- `web/pkg/localharness_bg.wasm`

These are not tracked by git, so they are workspace bulk rather than source debt.
`.gitignore` already covers `node_modules/` and `/target`; `contracts/out` is
not tracked. No source cleanup needed here unless the developer experience wants
an explicit clean script.

## Recommended cleanup plan

### Phase 1: restore warning signal

1. Remove `#[allow(unused_imports)]` + `use crate::*` from CLI modules gradually.
2. Add a CI check that rejects new broad `allow(unused_imports)`,
   `allow(dead_code)`, and `allow(deprecated)` unless the line includes a short
   reason.
3. Add `cargo check --all-targets --all-features` to the standard verification
   list and fail on warnings.

### Phase 2: delete or archive obvious legacy systems

1. Archive/delete flat registry + legacy deploy.
2. Archive/delete BootstrapFaucet + deploy script.
3. Archive/delete PairingFacet add scripts and source after confirming no
   reproduction workflow needs them.
4. Remove the currently unused `src/landing.rs::ios_unavailable`.

### Phase 3: coordinate live-contract cleanup

1. Prepare a diamond cleanup cut for `chargeFromWallet`.
2. Decide SessionFacet fate; either fully supported or fully retired.
3. If retiring, remove facet, CLI command, registry helpers, proxy session gate,
   and docs in one campaign.

### Phase 4: reconcile product/backend status

1. Decide whether OpenAI backend is supported or parked.
2. Decide whether browser pricing UI is supported or retired.
3. Update `design/cleanup-backlog.md` so it is the canonical queue, not a note.

## Proposed policy

Use this rule going forward:

> A dead or parked system cannot stay in active source without an owner, an
> expiry condition, and a cleanup-backlog entry.

Suggested annotation format:

```text
// PARKED until <condition>. Owner: <name>. Cleanup: design/cleanup-backlog.md#...
```

Then enforce:

- no unexplained `allow(dead_code)`;
- no unexplained `allow(unused_imports)`;
- no active-source files whose README says "historical only";
- no docs saying "removed" while add/deploy scripts for that system remain in
  active script directories.

## Bottom line

Yes: the repo has accumulated significant legacy/dead-weight debt. The worst
offenders are not random unused functions; they are whole retired systems still
living in active directories, plus warning suppressions that make normal Rust
feedback less useful. The fastest improvement is to restore the warning signal
in the CLI and then archive/delete the PairingFacet, flat registry, and
BootstrapFaucet lineages.
