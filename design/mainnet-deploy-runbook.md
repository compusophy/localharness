# Tempo mainnet deploy runbook (chain 4217)

> STATUS (2026-06-16): the **on-ramp slice IS LIVE on mainnet** — diamond
> `0x8ab4f3a57643410cdf4022cdaf1faeef234f3a77`, $LH token
> `0x7ba3c9a39596e438b05c56dfc779700b58aea814`, with CreditMeter (lock-aware) +
> MintGate cut & configured, C1 cap = 100k $LH/day, and a direct
> mint→lock→clawback proven on-chain. Fee token = USDC.e (deployer + submitter
> fee-token set via Fee Manager `setUserToken`). The full economy ladder
> (steps 7–14 below) is NOT yet cut on mainnet. Remaining for a LIVE card buy:
> the maintainer's LIVE Stripe key + a live webhook (then redeploy the proxy with
> ONRAMP_SUBMITTER_KEY + FIAT_ISSUER_KEY).
>
> Bootstrap gotcha (solved): Tempo charges gas in pathUSD by default; the
> deployer held only USDC.e. Fix = Fee Manager `setUserToken(USDC.e)` (precompile
> `0xfeec…`) via a viem/tempo 0x76 tx that pays its own fee in USDC.e — after
> that, plain forge/cast txs from the account pay gas in USDC.e.

Reproduces the live Moderato diamond's final facet state on Tempo **mainnet**
(chain 4217, `https://rpc.tempo.xyz`, fee token = USDC.e
`0x20c000000000000000000000b9537d11c60e8b50` or pathUSD
`0x20c0000000000000000000000000000000000000`). `tempo_mainnet` RPC alias is in
`foundry.toml`.

## 0. THE ONE BLOCKER — fund the deployer (human step)

Tempo has **no native coin** (gas is paid in USD stablecoins) and **no mainnet
faucet**. Stripe's crypto onramp does NOT reach Tempo directly. So the first
real value must be bridged in:

1. Buy ~$40–60 **USDC** with a card on Coinbase/Kraken **or** Stripe's hosted
   onramp — choose the **Base** or **Ethereum** network.
2. Bridge it to Tempo at [stargate.finance](https://stargate.finance/) (source =
   Base/ETH, dest = Tempo) → you receive **USDC.e** on Tempo. (Ethereum↔Tempo is
   the documented zero-fee route.)
3. Send the USDC.e to the **deployer EOA** (reuse the testnet owner
   `0x313b1659F5037080aA0C113D386C5954F348EF1e` or a fresh key). Deploy costs
   ~$6–10; keep the rest as **sponsor runway**.

Then: `export EVM_PRIVATE_KEY=0x<deployer>` and `export RPC=tempo_mainnet`. Every
`forge script` below adds `--rpc-url $RPC --broadcast`; export `DIAMOND` after step 1.

## 1. Ordered deploy sequence

The facet *sources* already hold the final logic; the live diamond was upgraded
in place, so we run base `Add*` then the `Swap*/Replace*/Upgrade*` upgrades.
**SKIP** `AddPairingFacet*`/`RemovePairingFacet` (Pairing removed), `Deploy.s.sol`
(legacy), `DeployBootstrapFaucet` (broken), `AddErc721Facet` (use the Fresh variant).

```sh
# 1. Diamond (cut+loupe+ownership+base registry)
forge script script/DeployDiamond.s.sol ...        # → export DIAMOND=0x...

# 2. Registry + main identity to final state (ORDER MATTERS — AddMainIdentity
#    BEFORE SwapTreasuryAndMainCost, which Removes the 5 base main selectors)
forge script script/AddMainIdentityFacet.s.sol ...
INITIAL_REGISTRATION_COST_WEI=0 forge script script/SwapRegistryFacetAddCost.s.sol ...
forge script script/SwapTreasuryAndMainCost.s.sol ...
forge script script/AddRegistryNameValidation.s.sol ...

# 3. ERC-721 (fresh variant runs initErc721)
forge script script/AddErc721Fresh.s.sol ...

# 4. TBA (two-step → MultiSignerAccount). Capture ERC6551_REGISTRY from output.
forge script script/AddTbaFacet.s.sol ...          # → export ERC6551_REGISTRY=0x...
forge script script/SwapTbaImplToMultiSigner.s.sol ...

# 5. Credits token + CreditsFacet + ISSUER_ROLE + setCreditsToken + C1 CAP.
#    dailyAllowance defaults 0 (sybil-safe). MINT_WINDOW_CAP_WEI is the C1 launch
#    gate — set a finite cap (sized at HALF tolerable per-window loss; tumbling
#    window → ≤2x across a boundary). MINT_WINDOW_SECS default 1 day.
MINT_WINDOW_CAP_WEI=50000000000000000000000 MINT_WINDOW_SECS=86400 \
  forge script script/DeployCreditsFacet.s.sol ... # → export LH_TOKEN=0x...

# 6. Redeem, Session(+price), Meter(+upgrade, setMeter)
forge script script/AddRedeemFacet.s.sol ...
forge script script/AddSessionFacet.s.sol ...      # then cast setSessionDuration/Price
forge script script/AddCreditMeterFacet.s.sol ...
forge script script/UpgradeCreditMeterFacet.s.sol ...   # adds withdrawCredits
cast send $DIAMOND "setMeter(address)" $PROXY_METER_ADDR ...

# 7. Invite, DeviceRegistry, Feedback(base→update→clear)
forge script script/AddInviteFacet.s.sol ...
forge script script/AddDeviceRegistryFacet.s.sol ...
forge script script/AddFeedbackFacet.s.sol ...
forge script script/UpdateFeedbackFacet.s.sol ...
forge script script/AddFeedbackClear.s.sol ...

# 8. Release(base→adminReset→guildGuard)
forge script script/AddReleaseFacet.s.sol ...
forge script script/AddAdminReset.s.sol ...
forge script script/ReplaceReleaseGuildGuard.s.sol ...

# 9. Schedule(base→hardening→lastRun→completeJob), setScheduler
forge script script/AddScheduleFacet.s.sol ...
forge script script/AddScheduleHardening.s.sol ...
forge script script/AddScheduleLastRun.s.sol ...
forge script script/UpgradeScheduleFacet.s.sol ...
cast send $DIAMOND "setScheduler(address)" $PROXY_SCHEDULER_ADDR ...

# 10. x402 + economy ladder
forge script script/AddX402Facet.s.sol ...
forge script script/AddBountyFacet.s.sol ...
forge script script/AddPartyFacet.s.sol ...
forge script script/AddGuildFacet.s.sol ...
forge script script/AddVotingFacet.s.sol ...
forge script script/ReplaceVotingFacetSnapshot.s.sol ...   # quorum-snapshot fix
forge script script/AddWeightedVotingFacet.s.sol ...
forge script script/AddReputationFacet.s.sol ...
forge script script/AddValidationFacet.s.sol ...
forge script script/AddTitheFacet.s.sol ...

# 11. P2P / rooms / messaging
forge script script/AddSignalingFacet.s.sol ...
forge script script/ReplaceSignalingAnnounce.s.sol ...
forge script script/ReplaceSignalingLeave.s.sol ...
forge script script/AddSessionRoomFacet.s.sol ...
forge script script/AddTeamFacet.s.sol ...
forge script script/AddMessageFacet.s.sol ...
forge script script/AddPushFacet.s.sol ...
forge script script/AddSubscribeFacet.s.sol ...

# 12. ★ MintGate (fiat on-ramp) + lock-aware CreditMeter. Diamond already holds
#     ISSUER_ROLE (step 5), bounded by the C1 token cap + this fiat window.
FIAT_ISSUER_SIGNER=0x<dedicated signer> CLAWBACKER=0x<proxy clawback addr> \
FIAT_LOCK_SECS=7776000 PER_RECEIPT_MAX_WEI=<finite> \
FIAT_WINDOW_CAP_WEI=<finite> FIAT_WINDOW_SECS=86400 \
  forge script script/AddMintGateFacet.s.sol ...

# 13. Completeness gate: diff loupe facets() vs live Moderato (selector sets must
#     match minus Pairing) BEFORE trusting the deploy.
cast call $DIAMOND "facets()((address,bytes4[])[])" --rpc-url $RPC > fresh.txt
cast call 0x6c31c01e10C44f4813FffDC7D5e671c1b26Da30c "facets()((address,bytes4[])[])" --rpc-url https://rpc.moderato.tempo.xyz > live.txt
```

## 2. Post-deploy wiring

1. `src/registry/chain.rs` MAINNET: set `diamond = $DIAMOND`, `lh_token = $LH_TOKEN`
   (fee_token already = USDC.e). Delete the `mainnet_addresses_unset_until_deploy`
   `is_empty()` assertion so the `mainnet` feature can ship.
2. Re-pin the x402 + FiatMint EIP-712 domain test hashes for mainnet — read the
   live `x402DomainSeparator()` / `fiatMintDomainSeparator()` and compare (don't
   hand-pin; the domains bind chainId+diamond, both differ from testnet).
3. `src/app/sponsor.rs`: ROTATE the embedded key + move to the rate-capped relay
   (do NOT ship mainnet on the embedded-key model). Wire per-chain selection.
4. Proxy env (Vercel `proxy` project): `TEMPO_RPC=https://rpc.tempo.xyz`,
   `CHAIN_ID=4217`, `REGISTRY=$DIAMOND`, `LH_TOKEN=$LH_TOKEN`; swap Stripe to LIVE
   keys + a mainnet `FIAT_ISSUER_KEY` (distinct from `PROXY_METER_KEY`). Redeploy.
5. Rebuild wasm with `--features mainnet` (+ cache-buster) + deploy web. All three
   (crate / wasm bundle / proxy env) MUST point at chain 4217.

## 3. The go-live gate (decide before real money)

`$LH` is currently transferable (x402/withdraw/send) → fiat-for-transferable-token
is likely money-transmitter territory (see `design/custody-security.md` H2). The
**lower-risk launch** is **H2(b): make fiat-origin `$LH` permanently
non-transferable, spend-on-compute-only** (a separate balance class that never
reaches transfer/x402/withdraw) — maps to Stripe's "seller-maintained prepaid
credits" lane. The alternative, **H2(a)**, is a real MTL/MSB legal analysis of the
transferable mechanics. Confirm Stripe's TOS permits selling these credits either
way. This gates go-live regardless of how much plumbing is shipped.
