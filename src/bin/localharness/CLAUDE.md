# src/bin/localharness — agent-onboarding CLI subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/bin/localharness/`).
> `feature = wallet + native`. This is the HARNESS-AGNOSTIC, server-free front door
> that `web/skill.md` tells external agents to run — keep it self-contained and
> dependency-light. `main.rs` is the dispatcher; one module per command family.

## Structure
`main.rs` (arg dispatch + active-chain stderr print) → one module per family:
identity · publish · call · abtest · mcp · status · credits · schedule · invite ·
bounty · party · guild · vote · reputation · validation · colony · tba · probe ·
session · notify · onboard · onramp · link · buy · models · facet ·
diamond_bytecode · sh. Shared helpers in `util.rs` (`load_signer*`,
`take_value_flag`, `parse_id`). ~40 commands total. Smoke: `scripts/smoke-cli.sh`.

## Conventions (enforced by the tech-debt gate)
- Every command module imports EXPLICITLY — NO `use crate::*` (a drift guard fails
  on it). Reuse `util.rs` helpers; don't re-roll signer loading or flag parsing.
- Harness-agnostic + server-free: no daemon, no DB. The ONLY off-chain dependency
  is the `$LH` credit proxy (for `call`/inference) — see [[feedback_no_offchain_infra]].

## Chain selection — MAINNET by default (0.53.0)
`resolve_chain` defaults to MAINNET. `--dev` (or `LH_CHAIN=testnet`) opts into
testnet; a bad `LH_CHAIN` is a HARD ERROR. `main.rs` prints the active chain to
stderr (the footgun fix — testnet/mainnet mismatch caused "39 agents on CLI vs 7
in browser"). The PUBLISHED binary embeds NO mainnet money key.

## Keys live in $HOME, never the working dir
`util.rs::load_signer*` reads `~/.lh_<name>_mainnet.key` / per-name testnet keys.
Writing keys into the CWD was a git-leak hazard (fixed) — never reintroduce it.
Names are sanitized (no path traversal) before any key write / on-chain register.

## `call` = HEADLESS turn via the proxy (NOT the browser `?rpc=1` path)
`call.rs` runs a full agent turn server-side through the credit proxy and persists
per caller/target under `.localharness/history`. This is NOT the browser's hidden
`?rpc=1` iframe (that's caller-machine-local and only serves YOUR OWN agents).
`--pay <amt|auto>` settles a caller-signed x402 payment to the target's TBA.
`abtest.rs` runs the same turn across personas.

## Sponsorship on mainnet = the KEYLESS RELAY
Sponsored writes route through `registry::sponsor_relay` → `proxy/api/sponsor.ts`
(no embedded fee_payer key on mainnet). The relay is onboarding-gated: a WALLET-
funded caller is refused value-sponsorship (`LH_RELAY_FUNDED`) and must self-pay;
gas-only selectors are ALWAYS_FREE. `onboard`/`onramp`/`link` are the autonomous
onboarding path (USDC.e on-ramp, 1 USDC.e = 100 $LH). Detail →
`src/registry/CLAUDE.md` + `design/cli-mainnet-relay.md`.

## `sh` = bashlite (sandboxed shell)
`sh.rs` runs `.bl` scripts through the bashlite interpreter (fuel-bounded fs +
`lh-*` platform reads/writes behind a dry-run confirm gate). `--as <name>` runs as
that identity. design/bashlite.md.

## LESSON: never run two dev agents on one working tree — `git add -A` once swept a
parallel WIP into a broken commit. Stage explicit paths.
