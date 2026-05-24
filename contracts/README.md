# `contracts/` — Localharness on-chain registry

Two contract stacks live here:

1. **Flat `LocalharnessRegistry`** at `src/LocalharnessRegistry.sol`
   — the original ~110-line monolith. Currently deployed at
   `0x42c8D4EaF99bA80F6B6FCA8E163E077D9FC2F9db` on Tempo Moderato.
   This is what `src/app/registry.rs::REGISTRY_ADDRESS` in the wasm
   bundle reads.

2. **EIP-2535 Diamond** under `src/{Diamond,facets,interfaces,
   libraries,upgradeInitializers}/` — new architecture, ready to
   deploy. Replaces the flat contract so we can layer ERC-721,
   ERC-8004 reputation/validation, ERC-6551 helpers, and MPP
   payment facets without redeploying-the-world each time.

The flat contract stays in-tree as historical reference. The
diamond is the path forward; the cutover is "deploy diamond → swap
the address constant in the wasm bundle → redeploy bundle." Names
registered against the flat contract are NOT migrated automatically
(small enough population that this is fine for testnet).

## Deploy (Tempo Moderato testnet)

Requirements:

- `foundry` installed (`forge --version` works).
- An EVM private key with some testnet TMP for gas. Faucet via
  `tempo_fundAddress` RPC: see `src/app/registry.rs::request_faucet_funds`
  for the exact JSON-RPC shape.
- `forge-std` installed: `forge install foundry-rs/forge-std --no-git`
  from this directory (one-time).

### Diamond (new)

```sh
cd contracts
export EVM_PRIVATE_KEY=0x...your-funded-testnet-key
forge script script/DeployDiamond.s.sol \
    --rpc-url tempo_moderato \
    --private-key $EVM_PRIVATE_KEY \
    --broadcast
```

Prints the diamond address + each facet's address. Bake the
**diamond** address into `src/app/registry.rs::REGISTRY_ADDRESS`,
rebuild + deploy the wasm bundle.

### Flat (legacy)

```sh
forge script script/Deploy.s.sol \
    --rpc-url tempo_moderato \
    --private-key $EVM_PRIVATE_KEY \
    --broadcast
```

## Diamond architecture

The diamond proxy (`src/Diamond.sol`) holds storage and dispatches
every external call to the facet that owns its selector. Selectors
are wired in/out via `diamondCut` — the only way to upgrade.

```
contracts/src/
├── Diamond.sol                       proxy: fallback delegatecalls
│                                     to the facet that owns msg.sig
├── facets/
│   ├── DiamondCutFacet.sol           owner-only upgrade entry point
│   ├── DiamondLoupeFacet.sol         introspection (facets, selectors,
│   │                                 supportsInterface)
│   ├── OwnershipFacet.sol            EIP-173 owner() + transferOwnership
│   └── LocalharnessRegistryFacet.sol register / transfer / setMetadata
│                                     / isTaken / ownerOfName / ...
├── interfaces/
│   ├── IDiamond.sol                  FacetCut + DiamondCut event
│   ├── IDiamondCut.sol               diamondCut(...)
│   ├── IDiamondLoupe.sol             facets / facetFunctionSelectors / ...
│   ├── IERC173.sol                   ownership
│   └── IERC165.sol                   supportsInterface
├── libraries/
│   ├── LibDiamond.sol                THE library — storage slot,
│   │                                 enforceIsContractOwner,
│   │                                 diamondCut implementation
│   └── LibRegistryStorage.sol        isolated registry storage at
│                                     keccak256("localharness.registry.
│                                     storage.v1")
└── upgradeInitializers/
    └── DiamondInit.sol               one-shot init: sets ERC-165 flags
                                      and `nextId = 1`
```

### Adding a new facet (e.g. ERC-721, ERC-8004, ERC-6551 helpers, x402)

1. Write `src/facets/MyNewFacet.sol`. Use the diamond-storage
   pattern for any new state: define `LibMyNewStorage` with a
   `keccak256("localharness.mynew.storage.v1")` slot, never touch
   `LibRegistryStorage` directly.
2. `forge build`.
3. Cut it in via a one-off forge script (see `DeployDiamond.s.sol`
   for the template — same pattern, just one `FacetCut`):
   ```sh
   forge script script/AddMyNewFacet.s.sol \
       --rpc-url tempo_moderato \
       --private-key $EVM_PRIVATE_KEY \
       --broadcast
   ```
4. If the new facet needs initialisation, deploy a one-shot
   `MyNewInit.sol` and pass `(myNewInit, abi.encodeWithSelector(MyNewInit.init.selector))`
   to the cut.

### Upgrading a facet

Same as add, but with `FacetCutAction.Replace`. The selectors map
from the old facet to the new one; storage is preserved.

### Removing a facet

`FacetCutAction.Remove` with `facetAddress = address(0)`. The
selectors are removed from the dispatch table.

## Why a Diamond

The flat contract works fine for a single-purpose registry. But the
M9–M12 roadmap layers in:

- **ERC-721 conformance** — every name becomes a tradable NFT, which
  the permissionless ERC-6551 singleton registry then derives a
  token-bound account for (the agent's wallet).
- **ERC-8004 reputation + validation registries** — feedback storage,
  validator stake escrow.
- **MPP / x402 payment hooks** — per-call settlement layer.
- **Whatever else comes up.**

Each one of those would be a whole new flat contract under the
monolithic model — separate addresses, separate state, separate
migrations. With the diamond they're facets sharing the registry's
storage layout, addressable at one stable address. The bundle's
`REGISTRY_ADDRESS` constant doesn't change for the lifetime of the
project; only the facet selectors behind it do.

## Files (top-level summary)

- `foundry.toml` — Solidity 0.8.24, optimizer on, Tempo RPC alias
- `src/LocalharnessRegistry.sol` — legacy flat contract (~110 lines)
- `src/Diamond.sol` + `src/{facets,interfaces,libraries,upgradeInitializers}/`
  — the diamond stack
- `script/Deploy.s.sol` — legacy flat deploy
- `script/DeployDiamond.s.sol` — diamond deploy (atomic: facets +
  proxy + cut + init in one transaction sequence)
- `.gitignore` — `out/`, `cache/`, `broadcast/`, `lib/`
