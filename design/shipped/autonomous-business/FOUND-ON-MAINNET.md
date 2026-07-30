# Found the real localharness business on Tempo MAINNET — owner runbook

> The EXACT, owner-custody way to stand up the company on the LIVE chain (4217),
> signed by the owner's own key. Read-only-verified against mainnet on **2026-06-30**
> (every address/cost/availability below was an `eth_call` / CLI read, cited inline).
> Nothing here writes — the owner runs the one command at the end with their key.
>
> Companion to `FOUND-A-COMPANY.md` (the chain-agnostic quickstart). This file is
> the **mainnet-specific reality**: real `$LH`, the keyless relay's gate, and the
> one shipped code gap that blocks the turnkey path today.

---

## 0. TL;DR (blunt)

- **On testnet (`--dev`) it IS one command** and works end-to-end (embedded sponsor,
  no registration cost, no funded-gate). Do this first as the dogfood.
- **On mainnet it is NOT one command yet.** Three things sit between
  `company found … --confirm` and a live org, two are owner switches (funds + a proxy
  env), one is a one-line code gap:
  1. **Funds** — every name mint costs **1 `$LH`** on mainnet (verified
     `registrationCost()` = `1e18`). A 7-role company = **8 names = 8 `$LH`**. The
     founder wallet holds **1.00 `$LH`** today → must top up ~7+ `$LH` (real money).
  2. **Code gap (the blocker)** — `create_guild_sponsored` does NOT batch the
     `approve`/meter-bridge the other legs do, so `createGuild` reverts
     `InsufficientAllowance` on mainnet (verified static call). One-line fix below.
  3. **Relay funded-gate** — once any `$LH` sits in the WALLET (>1 `$LH`), the relay
     refuses to sponsor gas for `createGuild`/`setMetadata`/`fundGuild`
     (`LH_RELAY_FUNDED`). Mitigation: keep the `$LH` in the **meter** (the gate counts
     the wallet only) or raise the proxy's `LH_RELAY_BALANCE_CEILING_WEI`.

---

## 1. Identity & custody — who signs, and where the key lives

The org is owned by **the wallet that owns the `localharness` agent**:

| | |
|---|---|
| Owner wallet (signs everything) | `0x2e45badc9a5d332983337bd7fe23d754026f929c` |
| Owns | `localharness` (tokenId **#9**), agent TBA `0x9F62CEd650DF7f8CB9183ecD67b2Cb0807a79C38` |
| Wallet `$LH` today | **1.00 `$LH`** · meter 0 · agent TBA 0 (`localharness status localharness`) |

**Custody stays with the human.** The CLI signs the SENDER half of every Tempo tx
with a key it reads from **`~/.lh_localharness_mainnet.key`** (the
`util.rs::load_signer` convention — `~/.lh_<name>_mainnet.key`, never the working
dir). That file must contain the private key for `0x2e45badc…f929c`.

- `--as localharness` selects that key file. The guild + all role subdomains are
  registered to whatever address that key controls → the org is owned by the owner.
- **Do NOT** generate a key for this. There is no sandbox/agent money key on mainnet —
  the published binary embeds none, and the fee_payer half is signed server-side by
  the relay (`src/app/sponsor.rs`, `registry::sponsor_relay`). The only private key
  in play is the owner's local file.

---

## 2. The command — proposed names (verified free on mainnet) + the exact call

`createGuild(name)` **mints a brand-new identity NFT**, so the guild name must be
**available** and a valid DNS label. **It cannot be `localharness`** — that name is
the founder's own tokenId #9, so `createGuild("localharness")` reverts `NameTaken()`.
Pick a sibling brand.

**Proposed org name + role subdomains (all `whoami`-verified UNREGISTERED, 2026-06-30):**

Recommended org/guild name: **`lhco`** → prefix `lhco`:

| Role | Subdomain | Mainnet |
|---|---|---|
| executive | `lhco-exec.localharness.xyz` | FREE |
| pm | `lhco-pm.localharness.xyz` | FREE |
| coder | `lhco-coder.localharness.xyz` | FREE |
| reviewer | `lhco-review.localharness.xyz` | FREE |
| accounting | `lhco-acct.localharness.xyz` | FREE |
| hr | `lhco-hr.localharness.xyz` | FREE |
| marketing | `lhco-mktg.localharness.xyz` | FREE |

Alternative org names, also verified free: `localharness-co`, `localharnessco`,
`lh`, `harness`. (`lh-exec … lh-mktg` are all free too if you prefer the `lh` prefix.)
**Taken / unusable:** `localharness` (the founder, #9).

### The single exact command (mainnet)

Run as the owner identity, no treasury seed/prefund (minimizes real-money spend —
seed the treasury later once the org is verified):

```sh
localharness company found --as localharness lhco \
  "Localharness builds and sells self-sovereign, on-chain AI-agent software on Tempo." \
  --confirm
```

- **Drop `--confirm`** for the SAFE PREVIEW — it prints the full plan (guild, 7
  subdomains, total `$LH`) and writes **nothing**. Always preview first.
- **Lean variant** (cuts 8 names → 4, i.e. 8 `$LH` → 4 `$LH`; grow later via HR):
  add `--roles executive,coder,reviewer`.
- **Testnet dogfood first** (recommended): append **`--dev`** to either command — runs
  on Moderato (42431) with the embedded sponsor, no registration cost, no funded-gate.
- `--seed-treasury N` / `--prefund-each N` add real `$LH` spend on top of the
  registration costs; leave them off for the first founding.

The active chain is printed to stderr on every invocation — confirm it says
`Tempo mainnet (chain 4217)` before `--confirm`.

---

## 3. Relay-gating reality — what's sponsored vs what gates

On mainnet **no build holds a fee_payer key**; gas is signed server-side by the
keyless relay (`proxy/api/sponsor.ts`), authed by the owner's personal-sign token.
The relay classifies each call's selector:

| Leg in `company found` | Selector class | Gas on mainnet |
|---|---|---|
| `register` (×7 subdomains) | **ALWAYS_FREE** | Sponsored regardless of balance ✓ |
| `createGuild` (×1) | gated (onboarding-only) | Sponsored **only if wallet ≤ 1 `$LH`**; else `LH_RELAY_FUNDED` — **and** reverts on the approve gap (§5) |
| `setMetadata` persona (×7) | gated | Sponsored only if wallet ≤ 1 `$LH`; else `LH_RELAY_FUNDED` → persona silently `[FAILED]`, founding continues |
| `fundGuild` (`--seed-treasury`) | gated | same gate + the seed `$LH` |
| `createTokenBoundAccount` (`--prefund-each`) | gated | same gate |
| `transfer` `$LH` (`--prefund-each`) | **SELF_PAY** | Sponsored even when funded ✓ |

The onboarding gate is `walletLHBalance > LH_RELAY_BALANCE_CEILING_WEI`
(default **1e18 = 1 `$LH`**, strict `>`). Crucially it reads the **wallet** `$LH`
balance (`lhBalanceOf`), **not the meter**. So:

- **Keep the founding `$LH` in the METER, wallet at ≤1 `$LH`** → the relay treats the
  founder as "onboarding" and sponsors the gated legs, while the `register`/`fundGuild`
  escrow legs auto-bridge meter→wallet (`withdrawCredits`) just-in-time to pay the
  per-name cost. This is the path that keeps gas sponsored.
- **How the owner clears `LH_RELAY_FUNDED` if it still trips:** raise the ceiling on
  the proxy — `LH_RELAY_BALANCE_CEILING_WEI` env in the **proxy** project, then
  `cd proxy && vercel --prod` (separate deploy). The owner controls the proxy, so this
  is an owner switch, not a third-party dependency. (Self-paying gas is NOT an option:
  agents hold `$LH`, never the USDC.e fee token, so they cannot pay Tempo gas
  themselves — the relay is the only gas path.)
- Fee-token sanity: `company found` passes `registry::ALPHA_USD_ADDRESS()`, which is
  **chain-aware** (`chain::active().fee_token`) → **USDC.e** `0x20c0…8b50` on mainnet,
  matching the relay's pinned `FEE_TOKEN`. No `LH_RELAY_FEETOKEN` mismatch.

---

## 4. Funding — real `$LH`, real money

Verified on mainnet: **`registrationCost()` = `1e18` = 1 `$LH` per name.**

```
guild mint (createGuild)        1 $LH
7 subdomain mints (register)    7 $LH
──────────────────────────────────────
registration total (7 roles)    8 $LH     (lean 3-role: 4 $LH)
+ optional --seed-treasury N    N $LH
+ optional --prefund-each M     M × roles $LH
```

The founder wallet holds **1.00 `$LH`** today → short ~**7 `$LH`** for a full
7-role founding. Two ways to fund (both move real value):

1. **Transfer existing `$LH`** you already hold to the founder, then deposit it to the
   meter (`depositCredits`) so the wallet stays ≤1 `$LH` (keeps the relay gate happy,
   §3). `$LH` `currency()` = `"credits"` (verified) — it is in-system credit, **not**
   a fee token; it cannot pay gas.
2. **Stripe on-ramp** — `localharness onramp`/`buy` (MintGateFacet): **1 USDC.e =
   100 `$LH`**, i.e. ~**$0.08** of USDC.e buys the 8 `$LH` for a full founding. This is
   a **real card charge** — the one genuinely-paid step.

This is the only place the owner spends money. Everything else (gas) is sponsored.

---

## 5. The shipped code gap that blocks the turnkey path (must fix before mainnet)

`create_guild_sponsored` (`src/registry/guild.rs:172`) uses the plain
`sponsored_diamond_call` — it does **not** batch the `$LH` `approve` (and meter→wallet
bridge) that **every other escrow leg does** (`claim_and_maybe_set_main_sponsored`
and `fund_guild_sponsored` both use `sponsored_escrow_diamond_call_bridged`).

On mainnet `createGuild` pulls the 1 `$LH` cost via `transferFrom` (GuildFacet
`_chargeRegistrationCost`), which needs a standing allowance. The founder's allowance
to the diamond is **0**, so:

```
$ cast call <diamond> "createGuild(string)" "lhco" --from 0x2e45badc…f929c
  execution reverted: TIP20 token error: InsufficientAllowance   (0x13be252b)   ← verified
```

**Until this is fixed, `company found --confirm` cannot create the guild on mainnet**
(STEP 1 reverts; the run aborts before any subdomain is minted). The fix is a one-liner
— make `create_guild_sponsored` mirror the subdomain path:

```rust
// guild.rs — instead of sponsored_diamond_call(...):
let cost = registration_cost().await.unwrap_or(0);
let bridge_wei = cost.saturating_sub(token_balance_of(&sender_hex).await.unwrap_or(0));
sponsored_escrow_diamond_call_bridged(
    sender, fee_payer, cost, encode_create_guild(name), fee_token, gas, bridge_wei,
).await
```

This batches `approve(diamond, cost)` + an optional `withdrawCredits` meter→wallet
bridge into the same Tempo tx, exactly like `claim_and_maybe_set_main_sponsored`.
(Land it, re-gate, ship in the next crate release; it is owner-stakes code so it is
flagged here rather than applied in this read-only pass.)

---

## 6. One-command vs human-switch — the honest split

**One command, works today:** `localharness company found --as localharness lhco "…" --confirm --dev`
(testnet). Embedded sponsor, no registration cost, no funded-gate → guild + 7
subdomains + 7 personas, end-to-end. Use it to prove the create→`company status` cycle
before spending real `$LH`.

**Mainnet — needs these human switches first (then it's the same one command, no `--dev`):**

| Step | Sponsored? | Needs owner key / funds / switch |
|---|---|---|
| Sign every tx (sender half) | — | **Owner key** `~/.lh_localharness_mainnet.key` (= `0x2e45badc…`) |
| Gas (fee_payer half, all legs) | **Yes — keyless relay** | none (but gated legs need §3 wallet ≤1 `$LH` or the proxy ceiling switch) |
| Guild mint `createGuild` | gas: gated | **§5 code fix** + **1 `$LH`** cost |
| 7 subdomain mints `register` | gas: ALWAYS_FREE ✓ | **7 `$LH`** cost (auto-bridged from meter) |
| 7 personas `setMetadata` | gas: gated | wallet ≤1 `$LH` (or proxy ceiling), no `$LH` cost |
| Seed treasury / prefund (optional) | gas: gated | the seed/prefund `$LH` |
| Fund the 8 `$LH` | — | **Owner funds** — Stripe on-ramp (1 USDC.e = 100 `$LH`, ~$0.08) or transfer `$LH` → meter |

**Net:** the owner's switches are (1) place `~/.lh_localharness_mainnet.key`, (2) land
the §5 one-line fix, (3) on-ramp ~8 `$LH` into the meter, (4) ensure the relay treats
the wallet as unfunded (keep wallet ≤1 `$LH`, or raise `LH_RELAY_BALANCE_CEILING_WEI`
and redeploy the proxy). After that, the founding is the single
`company found --as localharness lhco "…" --confirm`, and `company status lhco`
reads it back.
