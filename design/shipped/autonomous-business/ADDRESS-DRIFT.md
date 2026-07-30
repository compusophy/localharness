# Diamond address drift — investigation + verdict

**Status:** RESOLVED (analysis). Fix proposed, NOT applied (read-only investigation).
**Date:** 2026-06-30
**Backlog item:** `design/autonomous-business/BACKLOG.md` "[Ops][S][M] Fix the
diamond-address drift"; `LEDGER.md` "Address drift".

## TL;DR

There are **two different diamonds on two different chains**, and BOTH addresses are
correct for their own chain — there is no code-level bug in `registry::chain`:

| Address | Chain | Role | Status |
|---|---|---|---|
| `0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77` | **mainnet (4217)** | **live platform + CLI** | **CANONICAL** |
| `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` | testnet Moderato (42431) | dev opt-in | testnet-only |

The "drift" is purely a **documentation labeling** problem: root `CLAUDE.md` (and its
lockstep mirror `AGENTS.md`) present a table titled **"Canonical addresses (post-reset)"**
whose Diamond/`$LH`/fee-token rows are the **Moderato TESTNET** values, with no
"(testnet)" qualifier — while the live platform is now mainnet-primary. A reader who
treats that table as authoritative gets the wrong (testnet) diamond for the live platform.

## Source-of-truth evidence table

Per `CLAUDE.md` Documentation SOP, chain facts are DERIVED from
`registry::chain::{MAINNET,MODERATO}` → `src/docs_manifest.rs` → GEN blocks in
`web/llms.txt` / `web/skill.md`. Tracing each surface:

| Surface | File:loc | Mainnet diamond it states | Verdict |
|---|---|---|---|
| **SOURCE OF TRUTH** | `src/registry/chain.rs:65` (`MAINNET.diamond`) | `0x8ab4f3a5…f3a77` | ✅ correct |
| (testnet const) | `src/registry/chain.rs:47` (`MODERATO.diamond`) | `0x6c31c01e…Da30c` | ✅ correct (this is chain 42431) |
| chain.rs unit test | `src/registry/chain.rs:143,172` | asserts `MAINNET.diamond == 0x8ab4f3a5…` | ✅ guards it |
| docs_manifest | `src/docs_manifest.rs:286` `render_chain()` → `chain::MAINNET` | `0x8ab4f3a5…` (derived) | ✅ correct |
| llms.txt GEN block | `web/llms.txt:287,294` | `0x8ab4f3a5…f3a77` | ✅ correct |
| skill.md GEN block | `web/skill.md:82` | `0x8ab4f3a5…f3a77` | ✅ correct |
| README.md | (hand-written, no address) | — | ✅ n/a |
| **root CLAUDE.md** | `CLAUDE.md` "Canonical addresses (post-reset)" table | `0x6c31c01e…Da30c` (TESTNET) under unqualified "Canonical" header | ❌ **stale/misleading** |
| **AGENTS.md** | `AGENTS.md:26-30` same table (lockstep mirror) | `0x6c31c01e…Da30c` | ❌ **stale/misleading (same as CLAUDE.md)** |

The whole CLAUDE.md/AGENTS.md "Canonical addresses" table is the **Moderato testnet**
set — every row matches `MODERATO`, not `MAINNET`:
- Diamond `0x6c31c01e…` = `MODERATO.diamond`
- `$LH` `0x90B84c7234…` = `MODERATO.lh_token` (mainnet `$LH` is `0x7ba3c9a3…aea814`)
- AlphaUSD `0x20c0…0001` = `MODERATO.fee_token` (mainnet fee token is USDC.e `0x20c0…8b50`)
- Sponsor `0x0AFf88…922C` = the TESTNET embedded fee_payer (`src/app/sponsor.rs:42`;
  mainnet embeds NO money key — relay-signed)
- Diamond owner `0x313b…EF1e` = the SAME owner on BOTH chains (verified on-chain below)

## On-chain test results (decisive)

`eth_call` against both candidate addresses on both RPCs. `owner()` = `0x8da5cb5b`,
`facets()` = `0x7a0ed627`.

| # | RPC (chain) | Target | Call | Raw result | Reading |
|---|---|---|---|---|---|
| 1 | `rpc.tempo.xyz` (mainnet) | `0x8ab4f3a5…` | `owner()` | `0x…313b1659f5037080aa0c113d386c5954f348ef1e` | **owner = `0x313b…EF1e`** ✓ live diamond |
| 2 | `rpc.tempo.xyz` (mainnet) | `0x6c31c01e…` | `owner()` | `error: execution reverted` | **no contract on mainnet** ✓ |
| 3 | `rpc.tempo.xyz` | — | `eth_chainId` | `0x1079` = **4217** | mainnet confirmed |
| 4 | `rpc.moderato.tempo.xyz` (testnet) | `0x6c31c01e…` | `owner()` | `0x…313b1659…f348ef1e` | owner `0x313b…EF1e` ✓ live **testnet** diamond |
| 5 | `rpc.tempo.xyz` (mainnet) | `0x8ab4f3a5…` | `facets()` | ABI array, length `0x24` = **36 facets** | richly-cut live diamond ✓ |
| 6 | `rpc.moderato.tempo.xyz` | — | `eth_chainId` | `0xa5bf` = **42431** | testnet confirmed |

**Conclusion from chain:** `0x8ab4f3a5…f3a77` is a live 36-facet diamond on mainnet
(4217) owned by the documented owner `0x313b…EF1e`. `0x6c31c01e…Da30c` reverts on
mainnet and is only a live diamond on the Moderato testnet (42431). The two are
distinct deployments on distinct chains.

## Verdict

- **Canonical (live platform / mainnet) Diamond = `0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77`.**
  Confirmed by the SOP source of truth (`registry::chain::MAINNET`), both managed docs,
  and a live on-chain `owner()`/`facets()` check.
- `0x6c31c01e…Da30c` is the **Moderato testnet** diamond (chain 42431) — correct for
  testnet (a dev opt-in), but NOT the live platform.
- **Stale/misleading files:** `CLAUDE.md` and `AGENTS.md` only. The "Canonical
  addresses (post-reset)" table documents the testnet address set without labeling it
  testnet, while the live platform is mainnet-primary.
- `src/registry/chain.rs`, `src/docs_manifest.rs`, `web/llms.txt`, `web/skill.md`,
  `README.md` are all **already correct** — no change needed.

## Proposed fix (minimal; NOT applied)

Doc-only hand-edit to the **`CLAUDE.md`** "Canonical addresses (post-reset)" table,
mirrored in lockstep into **`AGENTS.md`** (a `cargo test` drift guard reddens CI until
they match). Restructure the table so the **mainnet** set is the primary "canonical"
entry and the existing rows are explicitly marked testnet, e.g.:

```
## Canonical addresses

**Mainnet (chain 4217 — the live platform + CLI):**
| What | Address |
|------|---------|
| Diamond (`registry::chain::MAINNET.diamond`) | `0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77` |
| `$LH` token (`LocalharnessCredits`, TIP-20) | `0x7ba3c9a39596e438b05c56dfc779700b58aea814` |
| Fee token (USDC.e, sponsor `fee_token`)     | `0x20c000000000000000000000b9537d11c60e8b50` |
| Diamond owner (cut/admin key, NOT in repo)  | `0x313b1659F5037080aA0C113D386C5954F348EF1e` |

**Moderato testnet (chain 42431 — `LH_CHAIN=testnet` dev opt-in):**
| Diamond `0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c` · $LH `0x90B84c72…` ·
  AlphaUSD `0x20c0…0001` · embedded sponsor `0x0AFf88…922C` |
```

(Keep the 6551 Registry/Account-impl + Credit-proxy rows; those are chain-agnostic /
infra.)

### Notes on the fix scope

- **No `registry::chain` edit is needed or wanted.** The consts are already correct and
  guarded by unit tests; editing them is owner-stakes (it moves where every signed tx
  goes) and would be WRONG here — the source is the thing the docs should match.
- **`gen-docs` regen does NOT fix this.** `CLAUDE.md` and `AGENTS.md` are hand-written
  and deliberately excluded from `docs_manifest::MANAGED_DOCS` (`= ["web/skill.md",
  "web/llms.txt"]`); the generator never touches them. This is a manual edit.
- After editing, run `cargo test` (the CLAUDE.md↔AGENTS.md drift guard) and `wc -c
  CLAUDE.md` (40K harness cap).
