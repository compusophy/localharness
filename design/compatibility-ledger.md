# Compatibility ledger — legacy / dormant paths and their deletion conditions

Tech-debt report §10 recommended this: a single place that records, for each
legacy file/path or intentionally-dormant code, **what reads/writes it, why it's
kept, and the condition under which it can be deleted** — so "is this trash or is
it load-bearing?" stops being re-investigated every cleanup pass.

> Rule of thumb: nothing in active source is "dead" if it's listed here with a
> reason. If you want to remove an entry, satisfy its **Delete when** first.

## On-disk compatibility files (per-origin OPFS / cwd)

| File | Canonical home | Status | Delete when |
|------|----------------|--------|-------------|
| `.lh_wallet` | `wallet_store.rs` | LIVE — the seed IS the identity root. `EXEMPT_FILES` (never at-rest-encrypted; the seed is the key). | never (identity) |
| `.lh_device_key` | `wallet_store.rs` (`DEVICE_KEY_FILE`) | LIVE pre-wallet boot key; `EXEMPT_FILES` (read before the seed-derived key exists). | the boot sequence no longer needs a pre-seed device key |
| `.lh_owner` | `owner.rs` (`OWNER_FILE`) | LIVE — the on-chain owner address this device last *proved*; only a paint-order hint, re-verified every load. `EXEMPT_FILES`. | ownership paint stops needing a first-paint hint |
| `.lh_linked_owner` | `wallet_store.rs` (`LINKED_OWNER_FILE`) | LIVE second-device linking marker; `EXEMPT_FILES`. | device-linking is reworked off this marker |
| `.lh_pricing.json` | `app/pricing.rs` (`PRICING_FILE`) | LIVE — owner's local x402 price working copy (`pricing::load` in `mod.rs`, `pricing::save` from the live pricing-save handler). | the x402 price moves fully on-chain with no local working copy |
| `*.localharness.key` (cwd) | CLI `identity.rs` (`KEY_SUFFIX`) | LIVE legacy CLI identity path — a cwd key still works as identity (also `$LOCALHARNESS_HOME`). | the CLI drops cwd-key support (would break existing operators — coordinate) |

`EXEMPT_FILES` (never encrypted at rest) is pinned in `builtins/mod.rs` +
`filesystem/encrypted.rs`; see CLAUDE.md "Filesystem trait".

## Intentionally-dormant code (compiled, not yet wired)

| Item | Where | Status | Delete when |
|------|-------|--------|-------------|
| `pricing_card` / `pricing_readonly_line` | `app/templates.rs` | DORMANT — full pricing-card UI removed from the agent card in 0.10.15; kept `#[allow(dead_code)]` "warm" for a future visitor-pays surface. (The single-line pricing-save path IS live.) | a product decision lands that owner price-editing won't return to the card |
| `shared_fs`, `webrtc`, `sharedfs_sync`, `teams_sync` | `app/mod.rs` (`#[allow(dead_code)]`, each documented) | DORMANT — P2P shared-folder layers 3–5; compile-verified only (needs two browsers). | the teams/sync UI orchestration ships (then they're live, allow removed) or the P2P direction is dropped |

## Shelved on-chain

| Item | Where | Status | Delete when |
|------|-------|--------|-------------|
| `SessionFacet` (coarse time-boxed sessions) | `contracts/src/facets/SessionFacet.sol`, `registry/credits.rs`, CLI `session.rs`, proxy `fetch.ts`/`notify.ts` session refs | SHELVED — the per-message `CreditMeterFacet` is the live billing path; sessions remain referenced as a compatibility gate. | a decision to fully retire sessions → remove facet + CLI cmd + registry helpers + proxy gate + docs in ONE coordinated cut (see cleanup-backlog.md) |

## Already retired (for reference)

Archived to `contracts/archive/` 2026-06-21: the flat `LocalharnessRegistry` +
`Deploy.s.sol`, `BootstrapFaucet` + deploy, and the `PairingFacet` lineage.
