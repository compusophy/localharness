# contracts/archive — retired lineages (DO NOT DEPLOY)

Historical Solidity kept out of the active `src/` and `script/` trees so it stops
polluting scans, mental models, and `forge build` while preserving git history.
Foundry only compiles `src/`, `script/`, and `test/`, so nothing here is built or
deployable. The relative `../src/...` imports are preserved (each file kept its
`src/` vs `script/` slot) for reference only — they are not wired to the live
diamond.

## What's here and why

| Path | Status | Superseded by |
|------|--------|---------------|
| `src/LocalharnessRegistry.sol`, `script/Deploy.s.sol` | Flat pre-diamond registry, abandoned after the reset. | EIP-2535 diamond (`Diamond.sol` + `DeployDiamond.s.sol` + facets). |
| `src/BootstrapFaucet.sol`, `script/DeployBootstrapFaucet.s.sol` | Dormant/broken faucet; runbook says skip its deploy. | Tempo sponsorship (`src/app/sponsor.rs` + the sponsor relay). |
| `src/facets/PairingFacet.sol`, `script/AddPairingFacet.s.sol`, `script/AddPairingFacetV2.s.sol`, `script/RemovePairingFacet.s.sol` | Device-pairing facet, removed on-chain. | QR seed-adoption (`?adopt=1#s=…`, Option A). |

If you ever need to reproduce an old deployment, copy the file back into the
active tree first — don't point a live script at this directory.
