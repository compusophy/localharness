# A unified agent-harness standard — synthesis (telemetry #78)

Status: **position paper.** #78 proposed a five-pillar standard (Nostr event
log · x402+6551 economy · portable `.agent/` state · WASM sandbox ·
multi-platform gateway). `design/agent-mesh-interop.md` scored the pillars
against the repo; `design/harness-landscape-2026.md` supplies the field
evidence. This doc is the third step: for each pillar, what we HAVE (file
evidence), what the standard INTERFACE would concretely be, and the
adopt/propose split — ending with a do-not-build list. No point is credited
to us under a different name.

The one-line thesis: **the field signs messages (Buzz), opinions (ERC-8004),
payments (x402), and capability claims (A2A). A harness standard worth having
also signs STATE and COMPUTATION — and those two halves are the ones we ship.**

---

## 1. Event log (proposed: Nostr NIP-01/34 signed transport)

**Have.** Nothing in the harness. Zero Nostr in `src/**/*.rs`; the only Nostr
code is two brand-posting Node scripts outside the runtime
(`scripts/nostr-broadcast.mjs`, `scripts/nostr-seti.mjs`; key in
`.nostr_identity`, gitignored, keyed separately from agent identity — we hold
a live npub but it is a MARKETING identity, not an agent one). What fills the
role is chain-anchored: `SessionRoomFacet` (member-gated append-only opaque
KV-op log; CRDT+AES off-chain in `src/kv_reduce.rs`/`src/kv_room.rs`),
`SignalingFacet` (owner-signed presence, ecrecover, 10-min TTL), OPFS
conversation history (`src/app/history.rs`), and web push. Strong for
settlement, wrong for chatter: `createRoom` ≈1.3M gas.

**Standard interface.** An `EventLog` seam shaped like our L3 `Connection`
seam (`src/connections/`): `append(SignedEvent)` + `subscribe(Filter) →
EventStream`, with the event NIP-01-shaped (`kind, created_at, tags, content,
sig`) so a Nostr relay backend is a pass-through. Three backends, one trait:
`OpfsLog` (what `history.rs` does today), `SessionRoomLog` (the
settlement-grade lane — kv ops become signed events), `NostrRelayLog` (the
cheap lane for presence/offers/chatter that should never cost gas). Identity
binding is the unresolved half and the standard's real job: derive the Nostr
key from the SAME BIP-39 seed (we already carry k256; one identity root) and
bind npub↔name both directions — a `keccak256("localharness.npub")` metadata
slot on the agent's NFT, plus the name in the Nostr kind-0 profile. Either
side alone is spoofable; the pair verifies.

**Adopt:** NIP-01 client as the cheap lane, behind the seam, prototype-scoped
(relay trust unresolved — `agent-mesh-interop.md` §1). **Never** as ownership,
escrow, or settlement transport.

## 2. Economy (proposed: x402 + EIP-6551)

**Have — shipped, end to end.** `src/registry/x402.rs`: full EIP-712 settle
(domain separator read live from the diamond, one-shot nonce, zero-recipient
guard, 1-`$LH` unattended auto-pay ceiling); `X402Facet` verifies via
ecrecover + EIP-1271 with a price-locked ceiling (#72); the proxy mirrors the
digest byte-for-byte (`proxy/api/_x402.ts`) and gates MCP-over-HTTP with it
(`proxy/api/mcp.ts`). EIP-6551: `TbaFacet` + `MultiSignerAccount` — every
agent name is an ERC-721 whose token-bound account RECEIVES its x402
earnings (`call_agent` → `ask_agent` pays the target's TBA), with device
signers bound to the enroller so an NFT transfer revokes them. Escrow rungs on
top: bounty/party/guild/voting/reputation/validation facets. This pillar is
not a proposal for us; it is the substrate.

**Drift found while writing this doc.** x402 v2 (2025-12-11) renamed the
headers (`PAYMENT-REQUIRED`/`PAYMENT-SIGNATURE`/`PAYMENT-RESPONSE`) and
standardized CAIP-2 network IDs; we still parse the pre-v2 names
(`X-PAYMENT` / `x-x402-authorization`, `proxy/api/_x402.ts:237`). Audit before
claiming v2 compatibility (landscape §4). Honest caveats stay: settlement is
`$LH` (TIP-20-shaped credit) on Tempo, not multi-chain stablecoin x402, and
the paid call rides HTTP to the proxy.

**Propose others adopt:** settlement to the AGENT'S OWN token-bound account,
not the operator's wallet. Buzz's human-agent parity is parity of authorship;
6551 makes it parity of PROPERTY — the agent owns what it earns, and firing
the agent (transferring the NFT) transfers the treasury with it.

## 3. Portable state (proposed: `.agent/` directory)

**Have.** The state types with canonical on-chain slots — persona (≤4096B),
lessons (`src/lessons.rs`: last-10 × 240ch, 2000B), skills (`src/skills.rs`:
16 named fragments, 4000B) — folded into the prompt on EVERY surface (browser
session, CLI `call`, scheduler). Since #78 was filed, the bundle half
shipped (`src/bin/localharness/state.rs`): `localharness state <name>
[--out|--in]` exports/imports a versioned JSON bundle
(`localharness_agent_state: 1`, persona+lessons+skills, NEVER key material;
import diffs against chain through the pure cores and writes one sponsored tx
per changed slot). And `skills --export <dir>` writes each skill as an
agentskills.io `<skill>/SKILL.md` folder — loadable unmodified by the ~40
Agent-Skills harnesses (landscape §4).

**Standard interface — the `.agent/` layout, derived from the bundle**
(SHIPPED 2026-07-30: `localharness state <name> --dir <dir>` exports it;
`--in <dir>` imports it — same version gate + sanitize-diff-write path as the
file bundle; `skills/*/SKILL.md` round-trips via `skills::from_skill_md`):

```
.agent/
  manifest.json          # {"localharness_agent_state":1,"name":…,
                         #  "chain":{"id":…,"diamond":…,"token_id":…}}
  persona.md             # ≤4096 B system-prompt fragment
  lessons.md             # ≤2000 B, 10 × 240ch (lessons.rs invariants)
  skills/<slug>/SKILL.md # agentskills.io format (skills::to_skill_md)
  receipts.jsonl         # execution receipts (.lh_receipts.jsonl shape, §4)
  history/               # transport-local cache — NEVER canonical
```

Rules that make it a standard and not a folder: (1) the directory is a
**derived view** of chain-anchored slots — chain stays canonical, because a
signed global slot survives device loss and a synced folder does not; (2) no
keys, ever (`export_state` already enforces this); (3) versioned manifest —
unknown version = loud failure, not garbage writes; (4) import = sanitize both
sides through the pure cores, write only diffs.

**Known gap:** the tool allowlist lives in OPFS `agent.json`
(`src/app/agent_config.rs`), off-chain and outside the bundle — the one state
type that cannot travel yet.

## 4. Sandbox (proposed: WASM for untrusted code)

**Have — shipped twice, plus a shell.** (1) Cartridges: untrusted wasm
off-main-thread in a Web Worker with a main-thread watchdog that terminates a
hung run (`src/rustlite/loader.rs`, `web/cartridge-worker.js`); composition is
budget-bounded (`ComposeBudget::v1`: depth 5, 24 nodes, 8MB — the "bounded
recursion" shape every major vendor converged on, landscape §1d) and callable
(`spawn_lib`/`call`, trap-contained). (2) WASI-subset CLI runner
(`run_wasm_cli`, `src/app/cli.rs` + `web/wasi-worker.js`): 4s watchdog, 256KB
output cap, `path_open`=NOTCAPABLE, no sockets/stdin. (3) `bashlite`
(`src/bashlite/`): fuel-bounded interpreter over a `Rooted` filesystem with a
dry-run-manifest confirm gate. Honest limits (`agent-mesh-interop.md` §4): no
in-wasm instruction metering (containment = watchdog + terminate), no native
sandbox (no wasmtime — untrusted execution is browser-only by design), no
preview2/WIT.

**Standard interface — the receipt, not the runtime.** Runtimes won't
converge; RECORDS can. Because cartridges are deterministic wasm and
content-addressed, a run can emit a verifiable record binding
source→module→call: `src/receipt.rs` v1 pins the versioned preimage —
`keccak(source_keccak ‖ module_keccak ‖ compiler ‖ export ‖ args ‖ result ‖
fuel ‖ status)` — with build receipts checkable natively today (CLI
`receipt`) and call records wired to the browser's compose layer
(`.lh_receipts.jsonl`). This is the exact "outcome-weighted ranking" the SoK
paper says no production system has (landscape §7), and the only reputation
primitive a sybil cannot manufacture — forging one means actually executing
the code. Adjacent adoption from Codex: document sandbox ≠ approval as
orthogonal — wasm/bashlite containment decides what is POSSIBLE,
`confirm_guard` only decides when to PAUSE (landscape §6.3).

**Propose others adopt:** content-addressed modules + the receipt preimage as
the interchange format for "this skill ran and here is what happened." A
`SKILL.md` today is unsigned instructions plus scripts running with full agent
authority — the supply chain is demonstrably on fire (ClawHavoc, ~17%
malicious; landscape §7). Owned + signed + sandboxed + receipted is our lane.

## 5. Gateway (proposed: Telegram/Discord/multi-platform)

**Have — not built,** stated plainly. No Telegram/Discord/Slack/Matrix code.
Adjacent: web push with the tab closed (`src/app/notifications.rs`,
`proxy/api/notify.ts` — self or cross-agent `to:`), `proxy/api/inbound-email.ts`
(append-only log, polled, never wakes anyone), the cron scheduler
(`proxy/api/scheduler.ts` — proves no-tab headless turns work), and ACP
(`localharness acp` — editors reach the agent over stdio; that solved the
EDITOR gateway, not the CHAT one).

**Standard interface — a proxy-side webhook bridge,** because the proxy is
the one deliberate off-chain component and a gateway daemon in the crate is
exactly the infra we refuse: inbound `POST /api/channel/<kind>` normalizes
`{channel, from, text, reply_to}` → wakes the agent as a headless turn (the
scheduler already runs these) → outbound goes through a per-channel adapter
table in `notify.ts` (web push is entry #1; Telegram/Discord are entries, not
subsystems). First wire, already named in `agent-mesh-interop.md` §5:
`inbound-email.ts` → `notify` fan-out, so email WAKES instead of waiting to be
polled. After that a Telegram adapter is a second implementation of an
existing interface.

---

## 6. Adopt vs propose — the summary

| Direction | Item | Cost |
|---|---|---|
| ADOPT | `.agent/` layout as documented export target (unify `state --out` + `skills --export`) | one CLI flag + this schema |
| ADOPT | x402 v2 header names + CAIP-2 audit (`_x402.ts`, `registry/x402.rs`) | small, correctness |
| ADOPT | `EventLog` seam; Nostr NIP-01 backend as the cheap lane, seed-derived key + two-way npub↔name binding | prototype |
| ADOPT | sandbox ≠ approval documented as orthogonal (Codex rule) | docs only |
| ADOPT | proxy webhook bridge; first wire = inbound-email → notify | one wire, then adapters |
| PROPOSE | execution receipts as the standard's outcome record (`receipt.rs` preimage v1) | spec is written; browser emission open |
| PROPOSE | x402 settlement to token-bound accounts (agent-owned earnings) | shipped; evangelize |
| PROPOSE | SKILL.md as HARNESS onboarding, not just skill packaging — `web/skill.md` already onboards any harness's agent into the platform | shipped |

## 7. Do-not-build (superseded internally or hollow externally)

- **Nostr as settlement/ownership.** The diamond + x402 own that; Nostr is
  chatter. Mixing lanes is how signed pub/sub becomes crypto theater.
- **A discovery marketplace, reputation leaderboard, or agent directory.**
  Needs counterparties; ERC-8004's registries show the failure mode (0.67% of
  10,000 agents with any endpoint). Fails the pay-rent-at-N=1 test.
- **Migration onto ERC-8004 registries.** At most an 8004-shaped registration
  as a hedge; asserted reputation is worthless, derived reputation (receipts)
  is the product.
- **MCP Roots/Sampling/Logging.** Formally deprecated 2026-07-28.
- **A2A/AP2/card-rail commerce.** Requires a merchant of record; x402 under
  the LF is the rail we are already on.
- **ACP in the browser.** No subprocesses in a tab; ACP lives in the CLI and
  already ships there.
- **On-chain push subs / on-chain feedback.** REMOVED 2026-07-06; the
  off-chain paths (proxy store, telemetry→GitHub) superseded them. Never back.
- **Our own relay/team workspace.** Buzz exists and speaks ACP; we are
  reachable through it already. Bridge, don't clone.
- **`.agent/` as source of truth.** The directory is a view; the chain slots
  are canonical. A standard that blesses the folder re-introduces device-loss
  amnesia as a feature.

## 8. Sequencing

First: **the `.agent/` layout** (§3) — both halves exist (`state --out`,
`skills --export`); declaring the directory shape is the cheapest pillar to
make real and the one #78 could adopt tomorrow. Then the x402 v2 header audit
(correctness debt), the inbound-email wire (§5), the `EventLog` seam with the
Nostr backend behind it (§1), and browser-side receipt emission + a
re-execution verifier (§4) as the standing PROPOSE half.
