# reset-genesis — the one-shot chain-reset deploy

`contracts/script/ResetGenesis.s.sol` deploys a FRESH complete diamond with the
FULL CURRENT selector surface in one run. It consolidates `DeployDiamond.s.sol`
plus the 14 one-shot patch scripts (Replace*/Upgrade*/Add*Hardening/Swap*/
AddAdminReset/...) — selector sets extracted from the FACET SOURCES, not the old
scripts, so every later-added selector (adminBurnNames/adminResetAll,
setRegistrationCost/treasuryBalance/withdrawTreasury, setMainCost/mainCost,
withdrawableOf, lastRunOf-free world, settleUpto) is present by construction.
It is the blocker-remover for deleting the one-shot cut scripts and THE
pre-1.0.0 chain-reset tool.

## What it deploys

1. Diamond + DiamondCutFacet (constructor cut), then THREE batched `diamondCut`s
   (core identity / economy / coordination) with `DiamondInit.init` +
   `initErc721` initializers.
2. ERC-6551: fresh `ERC6551Registry` + `MultiSignerAccount` impl, wired via
   `setTbaConfig`.
3. `LocalharnessCredits` ($LH): supply cap, optional C1 rolling mint cap,
   `ISSUER_ROLE` granted to the diamond, `setCreditsToken`, faucet default 0.
4. Config: `setRegistrationCost` (default 1e18 = live value), optional
   `setMainCost` / `setMeter` / mint-gate keys (fiat signer, clawbacker, locks,
   windows) from env — unset keys log a `NEXT:` line instead.
5. A standalone `GuardedDiamondCutFacet` — the child-diamond genesis seed
   (`chain.rs::guarded_cut_facet`), NOT cut into the parent.

Post-flight asserts (revert = failed genesis): 27 facet addresses, 215 total
selectors, owner, `nextId()==1`, credits/tba wired, x402 domain separator live.

## Selector table (27 facets / 215 selectors)

Verified 2026-07-30 by set-diff against the LIVE mainnet loupe
(`facets()` on `0x8ab4…3a77`): live(249) − genesis(215) = exactly the 35
retired selectors; genesis − live = exactly `settleUpto` (source-ahead).

| facet | sel | | facet | sel |
|---|---|---|---|---|
| DiamondCutFacet | 1 | | X402Facet | 4 |
| DiamondLoupeFacet | 5 | | TitheFacet | 4 |
| OwnershipFacet | 2 | | BountyFacet | 13 |
| LocalharnessRegistryFacet | 14 | | PartyFacet | 15 |
| ERC721Facet | 12 | | GuildFacet | 16 |
| TbaFacet | 6 | | VotingFacet (own) | 9 |
| MainIdentityFacet | 7 | | WeightedVotingFacet (own) | 12 |
| DeviceRegistryFacet | 4 | | ReputationFacet | 4 |
| ReleaseFacet | 3 | | ValidationFacet | 13 |
| CreditsFacet | 7 | | SessionRoomFacet | 11 |
| RedeemFacet | 5 | | SignalingFacet | 7 |
| InviteFacet | 5 | | SubscribeFacet | 5 |
| CreditMeterFacet | 7 | | MessageFacet | 7 |
| MintGateFacet | 17 | | **total** | **215** |

Voting/WeightedVoting inherit GuildFacet; only their OWN selectors are cut —
the guild selectors stay routed to the GuildFacet address (the live-diamond
convention).

## Deliberately dropped

- **SessionFacet (6)**, **ScheduleFacet (18)**, **TeamFacet (11)** — retired;
  still cut on the live diamond today, the genesis is what finally sheds them.
  Sources for Session/Schedule remain in `facets/` untouched.
- **Not cut, by design:** CounterFacet (soliditylite demo target),
  OwnedTokensFacet (draft, never cut — `list_owned_tokens` scans),
  GuardedDiamondCutFacet (standalone child seed, deployed but not cut).
- **fiatLocked machinery: KEPT.** Not separable without source edits — the lock
  plumbing is internal to `mintFromFiat`/withdraw paths; dropping only the 3
  lock selectors would strand config/introspection while the machinery still
  runs. Fresh-storage default is locks-off (`fiatLockSecs=0`, the live value).

## Runbook (reset day)

```sh
cd contracts && forge install foundry-rs/forge-std --no-git   # once
EVM_PRIVATE_KEY=0x... MINT_WINDOW_CAP_WEI=... METER_ADDR=0x... \
FIAT_ISSUER_SIGNER=0x... CLAWBACKER=0x... \
forge script script/ResetGenesis.s.sol --rpc-url tempo_mainnet --broadcast
```

Then re-pin from the log: `src/registry/chain.rs`
(diamond / lh_token / guarded_cut_facet / loupe_facet / ownership_facet),
proxy env, wasm bundle; `gen-docs` regenerates the doc facts. Full env knob
list in the script header. Verify with `cast call <diamond> "facets()..."`.
