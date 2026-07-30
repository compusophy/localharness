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
`abtest.rs` runs the same turn across personas. METER-path turns register the
pure-read `evm_*` toolset (`crate::evm_tools`, deny-by-default + allowlist) so
identifier resolution is REAL (fleet F2: tool-free turns fabricated addresses);
the x402 pay-per-call path stays TOOL-FREE (one-shot nonce ⇒ exactly one
upstream request) and its system note (`identifier_note`) instructs the model
to REFUSE unverifiable identifier asks instead.

## Sponsorship on mainnet = the KEYLESS RELAY
Sponsored writes route through `registry::sponsor_relay` → `proxy/api/sponsor.ts`
(no embedded fee_payer key on mainnet). The relay is onboarding-gated: a WALLET-
funded caller is refused value-sponsorship (`LH_RELAY_FUNDED`) and must self-pay;
gas-only selectors are ALWAYS_FREE. `onboard`/`onramp`/`link` are the autonomous
onboarding path (USDC.e on-ramp, 1 USDC.e = 100 $LH). Detail →
`src/registry/CLAUDE.md` + `design/cli-mainnet-relay.md`.

## `lessons` / `skills` / `state` = the agent's LEARNED state (state.rs)
The blobs were always READ here (`call` folds them into the headless prompt) but
only the browser could WRITE them, so a CLI-only agent could never learn
(telemetry #78). `state.rs` closes it through the SAME pure cores the browser
tools use (`localharness::{lessons,skills}`), so a terminal-written lesson is
byte-identical to a tab-written one; `sanitize` both sides before comparing so an
unchanged blob costs no gas. Reads need no key (inspect any agent); writes are
owner-gated + sponsored, mirroring `publish::set_persona`.
⛔ NEVER batch the slots into one tx: `setMetadata` is ~8.5k gas/BYTE, so
persona+lessons+skills asks ~89M and the relay caps a tx at 50M
(`proxy/api/sponsor.ts MAX_GAS_LIMIT`). One tx per changed slot — each fits
alone. `state --out` bundles are PUBLIC on-chain state and never carry keys.

## `acp` = the Agent Client Protocol server (acp.rs)
`localharness acp [--as <name>] [--model <id>]` serves this identity's agent over
ACP (JSON-RPC 2.0, newline-delimited stdio) — the editor↔agent standard (Zed +
JetBrains; Buzz's `buzz-acp` bridge consumes it). Sessions ride
`call::start_headless_agent(…, multi_turn: true)`: on-chain persona/lessons/
skills, per-request meter billing. ⛔ multi_turn FORCES the meter path — the x402
one-shot nonce cannot survive a second `session/prompt`. ⛔ STDOUT IS THE WIRE:
one flushed JSON-RPC frame per line, nothing else (stdout is block-buffered when
piped; an unflushed frame deadlocks the client) — all chatter to stderr. stdin
EOF mid-turn = "no more requests", NOT cancel: drain the turn, then exit.
Declared capabilities stay MINIMAL (text-only prompts, no auth) with ONE widened:
`loadSession: true` — each prompt persists history_bytes to
`.localharness/acp/<sessionId>` (ids carry a timestamp so restarts can't collide
and overwrite), and `session/load` re-seeds a fresh agent + REPLAYS the transcript
(user/agent_message_chunk + completed tool pairs) before returning null. Widen a
capability only WITH its implementation. Proven live: multi-turn continuity AND a
two-process restart (codeword recalled through session/load) on mainnet. Pure wire
helpers unit-tested in-module.

## `sh` = bashlite (sandboxed shell)
`sh.rs` runs `.bl` scripts through the bashlite interpreter (fuel-bounded fs +
`lh-*` platform reads/writes behind a dry-run confirm gate). `--as <name>` runs as
that identity. design/bashlite.md.

## LESSON: never run two dev agents on one working tree — `git add -A` once swept a
parallel WIP into a broken commit. Stage explicit paths.
