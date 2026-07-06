# src/registry — on-chain layer subsystem spec

> Module-owned context (auto-loaded when an agent works in `src/registry/`).
> `feature=wallet`, all targets. The flat `registry::` surface is kept by
> `mod.rs` re-exports — one module per facet + `multichain` + `sponsor_relay` +
> `abi`/`rpc`/`tx`. This is the most EXPENSIVE place to be wrong (real gas, real
> money), so the gotchas below are load-bearing — read before touching a write path.

## The only durable handle is the DIAMOND address
Per-facet addresses are NOT pinned — facets churn via `diamondCut`. NEVER hardcode
a facet address; query live via DiamondLoupe (`facetAddress(selector)`). The
diamond address (`registry::REGISTRY_ADDRESS` / `chain::*`) is the one stable handle.

## Chain selection (runtime, not compile-time)
`chain.rs` holds `MAINNET` (4217) + `MODERATO` (42431). `resolve_chain` defaults
to MAINNET; CLI `--dev`/`LH_CHAIN=testnet` opts into testnet (0.53.0). Chain
accessors are **fn()s, not consts** — don't reintroduce const addresses. `is_mainnet()`
routes the submit chokepoints (and the browser's sponsored calls) to the keyless relay.

## Gas: ESTIMATE, never guess (data writes are gas-HUNGRY)
- `setMetadata` ≈ **7.6k gas/BYTE** → `1.2M + bytes*8500`. `gas::set_metadata_gas`
  is the one home for this formula (app side). A flat under-set CLIENT cap
  silently out-of-gasses big writes; block limit is 500M, so they fit.
- `cast estimate` before setting a cap. **Trust `debug_traceTransaction` (real exec)
  over `cast run` (replay).**

## Selectors: verify against the EXACT canonical signature
A wrong selector silently mis-routes or the relay 403s it (`LH_RELAY_SELECTOR`).
Compute `keccak256("name(types)")[:4]` from the canonical sig — don't eyeball or
trust memory. Real incidents this came up: `releaseName(uint256)` = `0x48e69e68`,
`transfer`/`settle` on the relay allowlist.

## Tempo native AA tx (0x76) — `tempo_tx.rs`
`examples/tempo_tx_live.rs` is the SOURCE OF TRUTH (live-verified). Key traps:
- **sender_sig is FLAT 65 bytes** (r‖s‖v, v=0/1); **fee_payer_sig is `rlp([v,r,s])`**.
- Sender hash = `keccak256(0x76 ‖ rlp([1..14 without sender_sig]))`.
- Fee-payer hash = `keccak256(0x78 ‖ rlp([1..10, fee_token, sender_address,
  aa_authorization_list, key_authorization?]))` — the spec page OMITS
  `aa_authorization_list` at pos 13; it's required (found by diffing wevm/ox).
- Sponsorship overhead ~275k gas on top of the inner call.

## $LH is TIP-20-shaped credit, NOT fee-token-eligible
Tempo `fee_token` requires TIP-20 + `currency()=="USD"`. `LocalharnessCredits`
returns `currency()=="credits"` → the chain REJECTS it as a fee_token (intentional:
$LH = in-system credits, not gas). Fee token is AlphaUSD (testnet) / USDC.e (mainnet).

## Sponsorship: embedded key (testnet) vs KEYLESS RELAY (mainnet)
- The `*_sponsored` wrappers take NO fee_payer/fee_token — the submit skeletons
  (`tx::default_fee`) resolve `sponsor::fee_payer()` + the active chain's fee
  token internally (`registry/sponsor.rs` is the ONE key home; `app::sponsor::
  signer()` is a thin alias). Custom sponsors → the explicit primitives
  (`submit_tempo_sponsored` / `create_sponsored`). Don't reintroduce per-call
  fee threading — every caller passed the same pair.
- Testnet: the committed low-budget fee_payer key pays (loss capped at its
  balance if extracted). Tempo access keys CANNOT sign as fee_payer — must be a root key.
- **MAINNET embeds NO money key.** `sponsor_relay` → `proxy/api/sponsor.ts` signs the
  fee_payer half SERVER-SIDE, gated by: selector allowlist + onboarding-only balance
  gate + rate window + float breaker. Gas-only, no-value selectors
  (register/releaseName/createInvite/settle/transfer) are in
  `ALWAYS_FREE_SELECTORS` so a FUNDED user can still do them (a funded caller is
  refused for value-sponsorship — `LH_RELAY_FUNDED`). `proxy/` is a SEPARATE deploy
  (`cd proxy && vercel --prod`) — a registry-side selector change needs the proxy
  redeployed to take effect. TS wire-port is pinned to Rust golden vectors.

## Two $LH pots, auto-bridged
Proxy debits the per-request METER (`creditOf`); `send`/`redeem` fund the WALLET;
x402 `settle` pulls the WALLET. Bridges both ways (`call.rs::ensure_meter_funded`
wallet→meter; `withdrawCredits` meter→wallet). "has $LH but 402s" = BOTH pots empty.

## Push enrollment + feedback are OFF-CHAIN — the bindings are GONE
Push subscriptions live in the proxy's store (`POST /api/push-sub` → GitHub
store; per-device `"dev"`-keyed dedup in `_pushstore.ts`/`_webpush.ts`) and
feedback is filed by the proxy telemetry endpoint as GitHub issues
(`proxy/api/telemetry.ts`). The on-chain client bindings (`push.rs` /
`feedback.rs` — setPushSub slots, FeedbackFacet submit/log-scan) were DELETED;
don't reintroduce an on-chain path for either.

## Full facet semantics live in `contracts/README.md` — this file is gotchas only.
