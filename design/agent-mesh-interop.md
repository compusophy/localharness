# Agent-mesh interop — what we'd adopt from Buzz / Hermes (telemetry #78)

> **2026-07-29 update:** the biggest interop gap this doc never named is now
> CLOSED — `localharness acp` (v0.74.0) serves any identity's agent over the
> Agent Client Protocol on stdio, which is the exact seam Buzz's `buzz-acp`
> bridge consumes (its "compatible harness" catalog is the ACP registry).
> Registry listing PR: agentclientprotocol/agent-client-protocol#1818.
> Full landscape context: `design/harness-landscape-2026.md`.

Status: **assessment, not a plan of record.** Filed in response to telemetry #78,
which proposed a "Unified Agent Harness Standard" combining localharness with
Buzz (Block's Nostr-based team workspace) and Hermes (a self-improving agent
framework). Each point is scored against what this repo ACTUALLY has, with file
evidence, then a verdict. No point is credited to us under a different name.

| # | Proposal | Reality here | Verdict |
|---|----------|--------------|---------|
| 1 | Nostr event log (NIP-01/34) as signed transport | **absent** in the harness | prototype as a CHEAP lane, not a replacement |
| 2 | x402 + EIP-6551 in the comms protocol | **shipped** | done; residual caveats below |
| 3 | Portable `.agent/` state directory | **partial** — the state exists, the layout doesn't | adopt the goal, keep our carrier |
| 4 | WASM sandboxing for untrusted code | **shipped** (two runtimes) | done; honest limits below |
| 5 | Telegram/Discord gateway | **absent** | one wire away, worth doing |

## 1. Nostr — absent, and genuinely complementary

Zero Nostr in the crate (`nostr|npub|nsec|NIP-` over `src/**/*.rs` finds nothing).
The only Nostr code is two standalone Node scripts for brand posting/discovery
(`scripts/nostr-broadcast.mjs`, `scripts/nostr-seti.mjs`) — outside the agent
runtime, keyed separately from agent identity.

What fills the role today is **chain-anchored**: `SessionRoomFacet` (member-gated
append-only KV-op log, CRDT+AES off-chain in `kv_reduce`/`kv_room`),
`SignalingFacet` (owner-signed presence/WebRTC signaling, ecrecover + 10-min TTL),
plus off-chain web push. That is stronger for *settlement* and weaker for *chatter*:
`createRoom` costs ~1.3M gas, and the WebRTC teams layer is still compile-verified
only, never proven cross-device.

**Verdict: worth prototyping, scoped.** Nostr is a cheap signed pub/sub lane for
things that should never have cost gas — presence, agent-to-agent messages, work
offers. It is NOT a substitute for on-chain ownership, escrow, or x402 settlement.
Cost estimate: a NIP-01 relay client in Rust (websocket + BIP-340 schnorr). We
already carry k256; a Nostr key derived from the same BIP-39 seed keeps ONE
identity root. The real question is relay trust and whether an agent's Nostr key
can be bound to its on-chain name — unresolved, and the reason this is a
prototype rather than a plan.

## 2. x402 + EIP-6551 — shipped

`src/registry/x402.rs` is a full EIP-712 settle path (domain separator read live,
digest, sign, zero-recipient guard, sponsored settle, a 1-`$LH` unattended
auto-pay ceiling); `X402Facet` verifies via ecrecover + EIP-1271 with a one-shot
nonce and a price-locked ceiling. It is embedded in the comms path rather than
bolted on: `call_agent` falls back to the x402 `ask_agent` route, paying the
target's token-bound account. TBAs are `TbaFacet` + `MultiSignerAccount`.

Residual caveats, stated plainly: payments settle in `$LH` (TIP-20-shaped credit)
on Tempo, not general multi-chain stablecoin x402; the paid call rides HTTP to the
proxy, so a Nostr/DM transport would need its own payment envelope.

## 3. Portable agent state — the goal is right, the directory isn't

We have the state TYPES and cross-runtime folding: `src/lessons.rs` (last 10,
≤240 chars, 2000-byte blob), `src/skills.rs` (16 named fragments), persona — each
with a canonical on-chain slot (`registry::names.rs` `PERSONA_LABEL` /
`LESSONS_LABEL` / `SKILLS_LABEL`) folded into the prompt on every surface (browser
session, CLI `call`, scheduler).

What's missing is exactly what #78 names: **a layout**. The browser keeps flat
dotfiles at each origin's OPFS root; the CLI keeps keys in
`~/.localharness/keys/` and history in `.localharness/history/` and has NO
lessons/skills files at all. There is no export/import of an agent-state bundle.
The CLI can WRITE persona (`localharness create --persona` → `set_persona`) but
not lessons or skills — so learning only happens in the browser.

**Verdict: adopt the goal, keep the carrier.** A directory is a weaker container
than what we already have: chain-addressed state is global, signed, and survives
device loss, which a synced folder does not.

**SHIPPED (`src/bin/localharness/state.rs`).** The CLI now has read/write parity
and a bundle: `lessons <name> [--add …]`, `skills <name> [--set|--rm]`, and
`state <name> [--out|--in]` — persona + lessons + skills as versioned JSON,
never key material. Writes go through the SAME pure cores the browser tools use
(`lessons::merge_lesson`, `skills::upsert`), so a terminal-written lesson is
byte-identical to a tab-written one, and both sides are sanitized before
comparing so an unchanged blob costs no gas. Proven E2E on mainnet: a lesson and
a skill written from the terminal came back quoted by a headless `call` turn.

⛔ The bundle is written ONE TX PER SLOT, never batched: `setMetadata` is ~8.5k
gas/BYTE, so a full persona+lessons+skills batch asks ~89M gas and the mainnet
relay caps a tx at 50M (`proxy/api/sponsor.ts MAX_GAS_LIMIT`). Each slot fits
alone (36M / 18M / 35M); the batch never does.

Still open here: the tool ALLOWLIST is not on-chain at all — it lives in OPFS
`agent.json` (`src/app/agent_config.rs`), so it can only ever travel inside a
bundle, and the bundle does not carry it yet.

## 4. WASM sandboxing — shipped, with limits worth stating

Two untrusted-wasm runtimes: cartridges (`rustlite/loader.rs` +
`web/cartridge-worker.js`, off-main-thread with a main-thread watchdog that
terminates a hung worker) and a capability-less WASI host. Composition is
budget-bounded (`ComposeBudget::v1`: 8 children/node, 16 KB each, 256 KB tree,
depth 5, 24 nodes, 1 MB FB/child, 8 MB tree) and, as of the same release as this
document, **callable** — `spawn_lib`/`call`/`call_ok` (telemetry #70).

Limits: no in-wasm instruction metering (containment is watchdog + `terminate()`;
`fuel` is advisory and rustlite emits no fuel checks); the WASI host has no
filesystem, sockets, or stdin, so real WASI toolchains can't run — it is a stdout
proof of concept; no native sandbox at all (no `wasmtime`/`wasmer` anywhere —
untrusted execution is browser-only); no preview2/component model, no WIT.

## 5. Multi-platform gateway — absent, and closer than it looks

No Telegram/Discord/Slack/Matrix code exists (`telegram|discord` hits only
marketing prose). What exists for async human reach is Web Push with the tab
closed (`src/app/notifications.rs`, `web/sw.js`, `proxy/api/notify.ts` — self or
cross-agent `to:`, sender-stamped, metered) and `proxy/api/inbound-email.ts`.

The honest gap is two-sided. Outbound: there is no channel abstraction to hang an
adapter on — `notify.ts` hardcodes web push. Inbound: nothing can WAKE an agent;
`inbound-email.ts` only appends to a rolling log the platform polls. But
`notify.ts` already fans out to another agent, and `inbound-email.ts` simply never
calls it — so the inbound wake is **one wire**, not a subsystem. Do that first;
then a Telegram adapter is a second implementation of an interface that exists,
rather than a bespoke integration.

## What this proposal got right

The framing — agents as peers in a mesh that discover, message, and transact —
matches where this codebase already points, and #78's sibling issue (#70) named
the real hole: composition existed as pixels but not as callable parts. That one
is now shipped. The remaining honest gaps are the ones above: a cheap signed
message lane, state portability across runtimes, and a way for a human to reach an
agent where they already are.
