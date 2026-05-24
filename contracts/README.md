# `contracts/` — Localharness on-chain registry

Foundry project for `LocalharnessRegistry.sol`, the Tempo-deployed
subdomain registry the wasm bundle reads on every mount.

## One-time deploy (Tempo Moderato testnet)

You need:

- `foundry` installed (`forge --version` works).
- An EVM private key with some testnet TMP for gas. Faucet:
  see Tempo Moderato docs — `https://moderato.tempo.xyz/faucet` or
  ask Stripe's Tempo Discord for testnet funds.
- The `forge-std` library installed for `Script.sol`:
  `forge install foundry-rs/forge-std --no-git` from this directory.

Then:

```sh
cd contracts
export EVM_PRIVATE_KEY=0x...your-funded-testnet-key
forge script script/Deploy.s.sol \
    --rpc-url tempo_moderato \
    --private-key $EVM_PRIVATE_KEY \
    --broadcast
```

The script prints the deployed address. Paste it into
`src/app/registry.rs` as `REGISTRY_ADDRESS`. Rebuild the wasm bundle
(`./scripts/build-web.sh`) and `vercel deploy --prod --yes`. The apex
"is this name taken?" check will start working immediately.

## What the contract does

Mirrors ERC-8122's `register / ownerOf / setMetadata` surface plus a
`idOfName` reverse index so the apex chrome can answer "is this name
taken?" in one `eth_call`.

- `register(string name)` → mints a numeric `agentId` to `msg.sender`.
  Enforces `a-z 0-9 -`, 3–32 chars, no leading/trailing dash, name
  not already taken, sender doesn't already own one.
- `idOfName(string)` → `uint256` (0 means unregistered). The read the
  apex form makes on every keystroke.
- `ownerOfId(uint256)` → `address`. Used to render "owned by 0x..."
  on a subdomain that's already taken.
- `setMetadata(uint256, bytes32 key, bytes value)` → owner-only. Holds
  ERC-8004-style off-chain metadata (description, avatar, agent_uri).
- `transfer(uint256, address)` → owner-only handoff.

## What it deliberately doesn't do (yet)

- **Not ERC-721.** The eventual plan (M9+) is to upgrade so each
  registered name automatically gets an ERC-6551 token-bound account
  — the agent's wallet. Upgrade adds functions, doesn't remove them;
  the registry can be migrated.
- **Not ERC-8004 reputation / validation.** Comes later when
  agent-to-agent commerce starts mattering (M10+).
- **No expiry, no rent.** Free testnet registration, one per address,
  no reclaim. Policy can be tightened pre-mainnet.

## Files

- `foundry.toml` — Solidity 0.8.24, optimizer on, Tempo RPC alias
- `src/LocalharnessRegistry.sol` — the contract (~100 lines)
- `script/Deploy.s.sol` — forge script that does the one-shot deploy
