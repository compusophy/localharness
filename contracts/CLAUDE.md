# contracts — EIP-2535 Diamond subsystem spec

> Module-owned context (auto-loaded when an agent works in `contracts/`). Foundry
> project: the EIP-2535 Diamond + facets + ERC-6551. **Facet SEMANTICS + ABI + gas
> notes live in `contracts/README.md` (the SSOT)** — THIS file is the cut/storage/
> deploy gotchas. The Rust-side on-chain gotchas are in `src/registry/CLAUDE.md`.

## Diamond-storage: each facet keys its OWN slot — never collide
Every facet's storage lives in a `LibXyzStorage` lib at
`keccak256("localharness.<facet>.storage.v1")`. A new facet MUST use its own keyed
struct (never a bare state var, never another facet's lib) or it corrupts a
neighbor's storage on cut. Mirror the existing `libraries/LibXyzStorage` pattern.

## diamondCut safety — RESERVED selectors + the static guard
`DiamondCut` / `DiamondLoupe` / `Ownership` selectors are RESERVED — cutting over
them bricks upgrade/introspection/admin. `src/cut_guard.rs` (Rust) is a STATIC
facet-cut safety lint over reserved selectors — run/respect it before a cut. Per-facet
addresses are NOT pinned (they churn via cuts); the DIAMOND address is the only
durable handle — query live via DiamondLoupe, never hardcode a facet address.

## Cuts + deploys: one Add<Facet>.s.sol each, run via forge with ./.env
Each facet is cut by `script/Add<Facet>.s.sol`; the diamond is deployed by
`DeployDiamond.s.sol`. forge does NOT auto-load the repo-root `.env` — load
`EVM_PRIVATE_KEY` from `./.env` explicitly. The maintainer's convention: the agent
runs EVERY cut/deploy itself (key in `./.env`), never tells the user to. Recovery
helpers like `MintForReceipt.s.sol` exist for fiat-mint gaps.

## Data writes are gas-HUNGRY — set length-scaled caps, never guess
`setMetadata` ≈ 7.6k gas/BYTE. Block limit 500M, so big
writes fit — the bug is always an under-set CLIENT cap. `cast estimate` before
capping; trust `debug_traceTransaction` (real exec) over `cast run` (replay).

## ERC-6551 `MultiSignerAccount`
CALL-only; additional device signers on top of the NFT holder + EIP-1271
`isValidSignature` (no seed sharing); signers are bound to their enroller (an NFT
transfer revokes them); rejects high-s signatures. Detail in `contracts/README.md`.

## On-chain feedback + push are REMOVED (2026-07-06)
FeedbackFacet + PushFacet selectors are cut from the mainnet diamond. Feedback =
the off-chain telemetry repo ONLY (`src/app/telemetry.rs` → `proxy/api/telemetry.ts`
→ GitHub Issues); push enrollment = the proxy's `/api/push-sub` store ONLY. Don't
reintroduce on-chain paths, fallback reads, or opt-in toggles for either.
