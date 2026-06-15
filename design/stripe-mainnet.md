# Stripe ↔ Tempo-mainnet on-ramp

> STATUS: open

Sell USD on Stripe → mint $LH on Tempo **mainnet**. Two coupled workstreams: (A) a
config seam to run mainnet without forking, and (B) a custody/issuer trust model so
minted $LH stays USD-backed. **1.0-only** (launch §11.1 "1.0 = mainnet"); mainnet
existence is UNVERIFIED in-repo and gates everything chain-bound.

## 1. Money flow

```
User --USD--> Stripe Checkout (lh_address bound at session-CREATE, authed route)
   --webhook checkout.session.completed--> proxy api/stripe-webhook.ts
   --verify HMAC + idempotency(event.id)--> EIP-712 sign FiatMint(to,amount,receiptId)
   --mintFromFiat()--> MintGateFacet (one-shot receiptId + on-chain window cap)
   --mint--> $LH lands NON-withdrawable in CreditMeter (fiatLockedOf, unlockAt)
   ... spendable on compute immediately; withdraw/transfer only after lock window ...
refund / charge.dispute.created --> clawbackFiatMint(receiptId) burns STILL-LOCKED $LH
```

Backing invariant (the whole point): `circulating_$LH (totalSupply − treasury sink)
≤ usd_held_at_stripe / peg`, in cents. Clawback burns track USD leaving Stripe.

## 2. Components

**On-chain (testnet-buildable)**
- `contracts/src/facets/MintGateFacet.sol` — `mintFromFiat(to,amount,receiptId,validBefore,sig)` (EIP-712 verify copied from X402Facet: domain-sep + ecrecover + low-s + EIP-1271), one-shot `receiptId`, on-chain rolling-window mint cap, per-receipt max, `clawbackFiatMint(receiptId)`; owner setters `setFiatIssuerSigner/setFiatMintCap/setFiatLockSecs/setClawbackRole`; views `mintedInWindow/fiatLockedOf/receiptUsed/circulatingSupply`.
- `contracts/src/libraries/LibMintGateStorage.sol` — `fiatIssuerSigner, clawbackRole, windowCapWei, windowStart, mintedInWindow, perReceiptMaxWei, fiatLockSecs, receiptUsed, fiatLocked{amount,unlockAt}`.
- `contracts/script/AddMintGateFacet.s.sol` — diamondCut add + owner one-time config. Diamond already holds ISSUER_ROLE; no new grant.
- `contracts/test/MintGateFacet.t.sol` — invariants: cap enforced, replayed receipt reverts, clawback burns only locked, signer rotation, forged/high-s reject, circulating ≤ minted.
- `src/registry/mint_gate.rs` — `circulating_supply / fiat_locked_of / encode_mint_from_fiat / sign_fiat_mint / mint_window` (mirrors credits.rs).

**Off-chain proxy (test-mode-buildable)**
- `proxy/api/stripe-checkout.ts` — authed route (reuse gemini.ts `recoverAddress`); create Checkout Session binding `lh_address` + `lh_nonce` in metadata; return redirect URL.
- `proxy/api/stripe-webhook.ts` — **Node runtime** (raw body for HMAC): verify `Stripe-Signature`, idempotency-gate on `event.id`, completed → sign+`mintFromFiat`; refund/dispute → `clawbackFiatMint`.
- `proxy/api/_stripe.ts` — Stripe SDK init, HMAC verify wrapper, peg const, receiptId derivation (from trusted Stripe data only).

**Config seam (testnet-buildable, no behavior change)**
- `src/registry/chain.rs` (NEW) — `ChainConfig {rpc_url, chain_id, diamond, lh_token, fee_token}`; presets `MODERATO` (= today's values) + `MAINNET` (TODO/placeholder); `active()` selected by cargo `mainnet` feature OR build.rs+`LH_CHAIN` env.
- `src/registry/mod.rs` — route existing `pub const` `RPC_URL/REGISTRY_ADDRESS/CHAIN_ID/LOCALHARNESS_TOKEN_ADDRESS` through `chain::active()`; names unchanged so 102 consumers / 27 files compile untouched.
- `src/registry/tx.rs` — `ALPHA_USD_ADDRESS` (l.117) from active preset's `fee_token`.
- `src/registry/x402.rs` — `x402_domain_matches_live_facet` (l.349) becomes per-preset expected hash.
- `src/app/sponsor.rs` — per-chain selection; mainnet uses the §6.3 relay, NOT a swapped embedded key (out of scope here).
- `proxy/api/_chain.ts` (NEW) — `TEMPO_RPC/REGISTRY/CHAIN_ID/LH_TOKEN` from `process.env` with Moderato defaults; refactor gemini/mcp/scheduler/notify/broadcast/fetch.ts to import it.
- `Cargo.toml` — `mainnet` feature.
- Foundry runbook — ordered manifest of every Add*/Swap*/Replace* that reproduces the LIVE diamond on a fresh chain.

**Docs**
- `design/custody-security.md` — invariant, loss-enumeration table, lock-window rationale, KYC decision log.
- `web/llms.txt` + `proxy/README.md` — buy-$LH flow, lock semantics, new env vars.

## 3. Sequenced build plan

1. **[SAFE NOW]** Config seam (`chain.rs`) with `MODERATO` = today's exact consts; route the const names through it; prove `cargo test` + `cargo check --target wasm32` byte-for-byte unchanged. `MAINNET` left placeholder/unselected.
2. **[SAFE NOW]** Proxy `_chain.ts` from env with Moderato defaults; refactor all 6 TS files; verify LIVE testnet proxy unchanged (env unset).
3. **[SAFE NOW]** Make `x402_domain_matches_live_facet` preset-aware (expected hash parameterized by active chain).
4. **[SAFE NOW]** Audit 102 const-consumers (27 files) for INLINE `42431`/addresses bypassing the const (tempo_tx.rs:705/716 embed `0x20c0…0001` in test fixtures — confirm test-only, not a 2nd source of truth).
5. **[SAFE NOW]** Build full `MintGateFacet` + `LibMintGateStorage` + `AddMintGateFacet` + Foundry invariant tests; cut into the **testnet** diamond (already holds ISSUER_ROLE); prove mint/cap/replay-revert/clawback/circulating≤minted with a throwaway signer + `cast`.
6. **[SAFE NOW]** `src/registry/mint_gate.rs` read/encode/sign helpers; native `cargo test`.
7. **[SAFE NOW]** Fiat-LOCK escrow in CreditMeter (`fiatLockedOf`, `unlockAt`): spends on `meter()`, reverts `withdrawCredits` until `unlockAt`.
8. **[SAFE NOW]** `stripe-checkout.ts` + `stripe-webhook.ts` against Stripe **TEST MODE** (`stripe listen --forward-to` + `stripe trigger checkout.session.completed`, card 4242…): full HMAC→idempotency→EIP-712→`mintFromFiat`→testnet-mint with zero real money.
9. **[SAFE NOW]** Refund/chargeback on test mode (`stripe trigger charge.refunded` / `charge.dispute.created`) → `clawbackFiatMint` → assert locked burned.
10. **[SAFE NOW]** Loss-enumeration doc + invariant def + read-only `circulatingSupply()` vs Stripe-balance reconciliation script.
11. **[SAFE NOW]** DRY-RUN the Foundry mainnet runbook on a fresh Moderato deploy; `diff` loupe `facets()` testnet-vs-fresh to prove completeness BEFORE spending mainnet gas.
12. **[BLOCKED: Tempo mainnet exists + params]** Fill `MAINNET` preset (RPC, chain_id, USD fee_token); deploy diamond + run the full ordered facet sequence (SKIP Pairing); grant diamond ISSUER_ROLE on new $LH.
13. **[BLOCKED: §6.3 relay decision + funded mainnet sponsor]** Replace embedded sponsor with rate-capped relay; fund with mainnet fee_token. Shipping mainnet on the embedded-key model = money-loss bug.
14. **[BLOCKED: mainnet diamond live]** Read mainnet `x402DomainSeparator()`; re-pin the x402 test hash for the MAINNET preset.
15. **[BLOCKED: Stripe LIVE keys + legal go]** Set live Stripe + `FIAT_ISSUER_KEY`; owner-set `fiatIssuerSigner` + caps on mainnet MintGate; set proxy env to mainnet; build wasm with mainnet preset; deploy web + proxy.
16. **[BLOCKED: sybil decision]** Decide `registrationCost` on mainnet — free real-value minting with no gate is a money-real sybil hole (launch §11.5).
17. **[BLOCKED: 15]** Live E2E: real card → mint → spend → refund → clawback on mainnet.

## 4. External inputs the maintainer must provide

- **[HARD BLOCKER]** Confirmation Tempo MAINNET exists and is open for deploys (launch-1.0.md: the mainnet moment has NOT happened).
- Tempo mainnet **RPC URL** (replaces `rpc.moderato.tempo.xyz`).
- Tempo mainnet **CHAIN_ID** (u64, replaces 42431) — wrong value invalidates every signature + the x402 domain.
- Mainnet canonical **USD-currency TIP-20 stablecoin** as sponsor fee_token (AlphaUSD-equivalent; replaces `0x20c0…0001`).
- Funded mainnet **deployer/owner key** controlling the mainnet diamond (today `0x313b…EF1e`; root `.env EVM_PRIVATE_KEY`).
- Funded mainnet **sponsor/relay** holding the fee_token (real money) + the §6.3 relay rewrite.
- Confirm **6551 Registry + MultiSignerAccount** re-deployed/canonical on mainnet.
- Stripe **live account** + **STRIPE_SECRET_KEY** + **STRIPE_WEBHOOK_SECRET** (+ test-mode equivalents — test keys unblock everything SAFE NOW).
- **FIAT_ISSUER_KEY** — NEW dedicated hot EOA for the EIP-712 fiat-mint signer (distinct from PROXY_METER_KEY + sponsor); address set on-chain via `setFiatIssuerSigner`.
- Owner signatures to: diamondCut MintGate, set `fiatIssuerSigner`, `windowCapWei`, `perReceiptMaxWei`, `fiatLockSecs`, `clawbackRole`.
- **Legal:** MTL/MSB analysis; Stripe TOS on stored-value/crypto; OFAC/sanctions screening (Stripe Identity/Radar) — gates going live, not buildable.
- **Treasury reserve** sizing (un-minted USD float as chargeback buffer) + chargeback-insurance decision.
- Stripe Radar / 3DS / first-buyer-cap policy values.

## 5. Open decisions

- **Mainnet $LH economics:** initial supply, mint policy, and whether `registrationCost` stays 0 (sybil gate is a hard 1.0 requirement once value is real — §11.5).
- **PEG:** $LH-wei per USD cent (e.g. 1 $LH = $1 → 1e16/cent); fixed vs floating. Mint against **NET settled USD** (Stripe fees deducted), not gross.
- **Lock window** `FIAT_LOCK_SECS` (7–30d) vs Stripe's ~120d dispute window — short = better UX, more uncovered long-tail risk on treasury.
- **Closed-loop vs open-loop:** keep fiat-$LH non-withdrawable / spend-only on platform compute (closest to "prepaid credits", lower regulatory surface) vs allow agent x402 cash-out (money-transmitter territory).
- **Seam mechanism:** cargo `mainnet` feature flag vs `build.rs` + `LH_CHAIN` env.

## 6. Money safety

- **Issuer key never mints directly.** `FIAT_ISSUER_KEY` is only an EIP-712 signer. ⚠️ **But the window cap is NOT a real ceiling as drawn — see §7 C1.**
- **Idempotency is the true backstop.** On-chain one-shot `receiptId` (derived from immutable Stripe event/PaymentIntent id, NEVER attacker-controllable fields) makes the mint idempotent regardless of Stripe's aggressive retries; a replayed/forged webhook fails HMAC or hits the used-receipt revert.
- **Address binding at session-CREATE.** `lh_address` set in the authenticated checkout route, read from the trusted session object — never from buyer-editable webhook/metadata fields.
- **Raw-body HMAC + 5-min skew check** (mirrors gemini.ts FRESHNESS_WINDOW); webhook MUST be Node runtime — Edge parses the body and silently breaks signature verify.
- **Clawback burns only STILL-LOCKED balance** (proxy clawback role can't touch wallet $LH). Already-spent/withdrawn fiat-$LH is unrecoverable — bounded by what the lock window let escape; that residual is accepted chargeback-fraud cost (mitigated by Radar + 3DS + first-buyer cap).
- **Reconciliation alarm:** periodic `circulatingSupply()` vs Stripe balance; alarm on drift (rounding, fees, FX, partial refunds).
- **Custody single points of failure:** Stripe can freeze the entire USD reserve (breaks backing for all circulating fiat-$LH at once; on-chain side can't detect/repair). Owner-tunable caps are themselves high-value targets — guard the owner key.
- **Three-place drift:** Rust crate, wasm bundle (stale bundle ships old addresses — see cache-buster gotcha), and proxy env must all point at the SAME chain, or auth sigs verify against the wrong chain and metering breaks.

## 7. Red-team must-fixes (CRITICAL — supersede §6 optimism)

Adversarial money-safety review (against the live contracts). Each is launch-blocking.

- **C1 — Window cap is fiction; `ISSUER_ROLE` is diamond-wide.** `LocalharnessCredits.mint` gates only on `_roles[ISSUER_ROLE][msg.sender]`, and the *whole diamond* holds it — every facet (`msg.sender == diamond`) can mint uncapped (`CreditsFacet.claimDaily` already does), and the owner can cut new mint paths. So "leak → max loss = windowCap" is FALSE; real blast radius = `supplyCap`. **Fix:** make MintGate a STANDALONE contract that is the SOLE `ISSUER_ROLE` holder (revoke from the diamond, re-route `CreditsFacet` through it) **or** enforce a global rolling-window ceiling inside `LocalharnessCredits._mint` itself. The cap must live where the only issuer lives. Invariant test: compromised signer + malicious 2nd facet still cannot exceed cap.
- **C2 — Clawback is non-functional today; the fiat-lock is load-bearing, not a footnote.** Token has no `burnFrom` (only holder-self `burn`), and `CreditMeterFacet.withdrawCredits` drains the entire balance to wallet `$LH` in any block — so a buyer can mint→withdraw→cash out *before* a chargeback and clawback recovers nothing. **Fix (first-class, not optional):** `mintFromFiat` mints into the diamond's escrow + a separate `fiatLockedOf{amount, unlockAt}`; `withdrawCredits`/`meter` become lock-aware (withdraw refuses locked until `unlockAt`; metered spend is final/non-clawable); `clawbackFiatMint` burns the diamond-held escrow. Test: withdraw-before-unlock reverts; clawback-before-spend burns full; spend-then-clawback recovers only the remainder. Until `withdrawCredits` is lock-aware, do not ship.
- **C3 — Webhook on Edge runtime breaks HMAC → open money-printer.** Every proxy route is `runtime:'edge'`, where raw-body Stripe signature verification silently fails; the copy-paste path either rejects all webhooks or a dev "fixes" it by skipping verify (forged POST → free mint). **Fix:** `stripe-webhook.ts` is a Vercel NODE function with raw body + `stripe.webhooks.constructEvent`. Deploy-time assert it is NOT Edge; test that a byte-mutated body 400s.
- **H1 — Lock window (≤30d) < Stripe ~120d dispute window = unbacked leak.** **Fix:** default a LONG lock (≥90d, 100% for new/unverified buyers; shorten only after clean history); make first-buyer mint cap + 3DS-over-threshold REQUIRED defaults (not knobs); ship the reconciliation alarm (`circulatingSupply()` vs settled-USD, alarm on drift); size + FUND the treasury reserve to the 120-day worst case = a launch gate. Name the residual as accepted-and-bounded.
- **H2 — LEGAL / money-transmitter (existential, maintainer-owned).** `$LH` is already OPEN-loop (`x402.settle`, `withdrawCredits`, `send_lh`, payouts) → the "closed-loop credits" framing is false → likely stored-value / money-transmitter territory → Stripe-termination + frozen-funds risk. **Fix:** either (a) get an MTL/MSB/stored-value legal analysis for the *actual* (transferable, third-party-payable) mechanics, or (b) ENFORCE on-chain that fiat-origin `$LH` is a permanently non-transferable, spend-on-compute-only balance class that never reaches transfer/x402/withdraw — much deeper than a time-lock. Confirm Stripe TOS permits selling these credits at all. **Gates go-live.**
- **M — Hot-key + owner-key custody.** `FIAT_ISSUER_KEY` must be distinct from `PROXY_METER_KEY` (assert at boot) and ideally in KMS/HSM (a proxy RCE then leaks a cap-bounded signing oracle, not the raw key). Cap-raise setters must be TIME-LOCKED or two-key — else an owner-key compromise does same-block `setCap(∞)`+mint. Alarm on any `FiatMinted` with no matching settled Stripe payment within N minutes.
- **M — Stripe freeze playbook.** Sweep settled USD to a separate reserve (shrink the freezable float); circuit-breaker view the webhook checks so a detected freeze pauses `mintFromFiat` instantly; carry the circulating-fiat exposure as a documented liability.
- **L — Domain-separator tests READ the live value.** The x402 + new FiatMint EIP-712 domain tests must `eth_call` the deployed getter and compare to the locally-computed hash (fail-closed on any chain-config mistake), not pin a hand-filled constant that can pass against a guess.

> Provenance: produced by a parallel design + adversarial-review workflow; 3 of 5 design facets (stripe-flow, usd→$LH, sponsor+sybil) were rate-limited mid-run — their substance is folded into §1–6 by the synthesis pass, but a dedicated deep-dive on each is worth a re-run before build.
