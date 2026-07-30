# CLI mainnet onboarding + funding for autonomous agents

Status: PROPOSAL; **Phase 1A (the chain-default flip + error-on-junk) IMPLEMENTED**
(`resolve_chain` → mainnet default + `Result`, `validate_chain_env`, `--dev` flag,
inverted tests). Sibling of `design/cli-mainnet-relay.md` (which shipped the
keyless fee_payer relay, phases 1–4). This doc closes the last gap that doc named:
a stock CLI defaults to testnet while the proxy + on-ramp are mainnet, so an
autonomous agent gets a 402 on its first real call. The fix is two parts, shipped
TOGETHER: flip the default chain (A) and give a brand-new terminal identity its
first `$LH` without a human (B). C surveys the funding-rail spectrum; D phases it.

Grounding rule: REUSE before invent. Every rail below maps onto a primitive that
already exists — `chain.rs`, `InviteFacet`, `RedeemFacet`, the sponsor relay
(`registry::sponsor_relay` / `proxy/api/sponsor.ts`), `CreditMeterFacet`,
`MintGateFacet`, `X402Facet`, `send_lh`. We add at most one new endpoint.

---

## A. Testnet → "dev mode": make MAINNET the CLI default

Today (`src/registry/chain.rs::resolve_chain`): `Some("mainnet") → MAINNET`,
everything-else → `MODERATO`. The published binary embeds NO money key (relay
signs fee_payer), so defaulting to mainnet is safe key-wise; the only hazard is
that mainnet writes need real `$LH`, which is exactly what B supplies.

**Change `resolve_chain` to default-mainnet, testnet opt-in:**

```rust
pub(crate) fn resolve_chain(lh_chain: Option<&str>) -> &'static ChainConfig {
    match lh_chain {
        Some("testnet") | Some("moderato") | Some("dev") => &MODERATO,
        _ => &MAINNET, // default + "mainnet" + junk → the live chain
    }
}
```

Plus a `--dev` global flag in `main.rs` that sets `LH_CHAIN=testnet` before the
`OnceLock` first reads (it must run before any `active()` call — set the env in
the arg pre-pass, not after a registry read).

**Test churn (the two tests in `chain.rs`):**

- `active_is_moderato_by_default` → rename `active_is_mainnet_by_default`, assert
  chain_id 4217 / `rpc.tempo.xyz` / the mainnet diamond+token. This test runs only
  when CI does NOT set `LH_CHAIN`; confirm the harness leaves it unset (it does).
- `lh_chain_env_selects_chain` → invert the expectations: `None → 4217`,
  `Some("testnet") → 42431`, `Some("dev") → 42431`, `Some("mainnet") → 4217`,
  `Some("") → 4217` (junk now means mainnet, the safe-by-default money chain — but
  see open question 1: junk-means-mainnet is a footgun; consider erroring instead).
- `mainnet_addresses_pinned` is unchanged.

Also fix the stale doc-comment in `chain.rs` ("Default ([`MODERATO`])…",
"Default-to-testnet so the money path is opt-in") and the `publish.rs:203-206`
net-of-fees comment flagged in the rail audit.

**Risk, stated honestly:** every default invocation now signs mainnet intents.
An agent that can't fund itself just trades a testnet no-op for a mainnet 402.
That is why A does not ship without B. Caveat: the full economy ladder
(bounty/guild/voting) is NOT yet cut on mainnet — only the diamond+token+meter+
MintGate slice is. Commands that hit an un-cut facet must fail loudly ("not yet
live on mainnet; use `--dev`"), not silently mis-target. Add a startup capability
probe (DiamondLoupe `facetAddress(selector)` == 0 → that command is dev-only).

---

## B. The CLI onboarding package: first `$LH` for a terminal identity

The web grants a fresh visitor ~2 `$LH` (enough to claim a 1-`$LH` subdomain +
~1 `$LH` starting credit) — see `invite.rs` `INVITE_DEFAULT_AMOUNT_WEI`. We mirror
that exact grant for a CLI identity. Two pots exist and AUTO-BRIDGE: the wallet
(`creditOf`-funded via redeem/invite/send) and the meter (`CreditMeterFacet`,
what the proxy debits). The relay sponsors GAS for free; the hole is the first
`$LH` of VALUE.

Inventory of existing bootstrap primitives and why each alone is insufficient:

- **`invite accept <code>`** — works headless TODAY, pays the escrowed `$LH` to
  the caller, relay-sponsored. Needs a funder to mint a code first (supply-neutral
  escrow). This is the cleanest reuse.
- **`redeem <code>`** — mints to the wallet, but `RedeemFacet` REJECTS callers who
  own no name (catch-22: you need `$LH` to claim a name, but redeem wants a name).
- **`buy`** — hosted Stripe URL, needs a human + card. Not headless.
- **`send_lh` / x402 `call --pay`** — require another already-funded agent.

**Recommended onboarding command — `localharness onboard [--invite <code>]`:**

A single command that walks the new identity from zero to ready:

1. Load-or-create the local key (identity is a key file; no name yet).
2. If `--invite <code>` given → `invite accept` it (headless, relay-sponsored).
   This is the autonomous-friendly path: an operator/parent agent runs
   `invite create` once (escrowing 2 `$LH` from its own wallet, the web-parity
   gift), hands the code to the new agent's env, and the new agent self-onboards.
   Supply-neutral, no faucet, no sybil hole, no new server.
3. Else (no invite) → print the human-in-the-loop options from C-1 (Stripe link
   or device-link) and exit non-zero with a clear "needs first funding" message.
   NEVER silently generate value.
4. Once funded → optionally `create_subdomain <name>` (claim an identity name) and
   verify `creditOf` > 0 so the next proxy call won't 402.

**Why not a free faucet:** `dailyAllowance` is deliberately 0 (sybil hole). A
headless faucet would let one operator spin unlimited funded identities. Invites
keep funding operator-paid and supply-neutral — the funder's own `$LH` is escrowed
and refundable. That is the web model; we mirror it, not weaken it.

**The catch-22 fix (redeem-without-name):** relax `RedeemFacet` to allow a
name-less caller to redeem INTO the wallet (the name requirement predates the
wallet pot). Tracked, not blocking — invites already give a headless path.

---

## C. The payment-rail spectrum (least → most autonomous)

### C-1. Human-in-the-loop (Stripe link OR device-link). SHIP (device-link MARK).

- **Stripe link:** `buy` already prints a hosted Checkout URL bound to the caller's
  personal-sign address (`stripe-checkout.ts`); webhook GROSS-mints `$LH` into the
  meter (`MintGateFacet`, 1 USD = 100 `$LH`). Zero new code — just surface it from
  `onboard` as fallback. SHIP (it exists).
- **Device-link:** mirror the browser's QR seed-adoption (`?adopt=1#s=<ct>`) for
  the terminal — CLI prints a code/QR, a funded WEB session encrypts its seed under
  it, CLI imports the SAME seed → instantly inherits the web wallet's `$LH`. The
  CLI has NO seed-adoption today (grep-confirmed). Effort: medium (reuse
  `encryption.rs` ECIES + the adopt URL format; new `localharness link` command).
  **MARK** — high-value for a human operator funding once, but not autonomous.

### C-2. Tempo MPP — agent pays stablecoin to mint `$LH`. MARK (primary future autonomous rail).

localharness is ALREADY on Tempo mainnet (4217) and already pays gas in the exact
USDC.e (`0x20c0…b9537d11c60e8b50`) that Stripe's MPP docs name as the mainnet
settle token — same chain, same stablecoin, no bridge. Build: a new MPP
**charge** endpoint in `proxy/` (the one sanctioned server) that returns
`402 + WWW-Authenticate: Payment` (`tempo.charge`, USDC.e, to a treasury TBA);
the agent (mppx) pays + retries with the Credential; the proxy verifies on-chain
settlement and GROSS-mints `$LH` into the buyer's meter — a like-for-like swap of
the Stripe webhook for a stablecoin verify, REUSING `MintGateFacet` +
`mintFromFiat` + `ISSUER_ROLE`. This is the only rail that makes a crypto-native
agent fully self-onboard with no human, no card, no parent agent. Effort:
medium-high (new payment-verify path to harden like x402; pin the preview API
version). Honest gaps: it re-introduces a USD→`$LH` rate (decouple from the peg —
make it a policy knob, 0.47.0 deliberately unpegged `$LH`); the Sessions escrow
contract + EIP-712 voucher struct are NOT yet verified from a primary source
(charge intent doesn't need them — confirm before any Sessions/metered work).
**MARK as the #2 build** — the highest-value autonomous rail after invites.

### C-3. x402 / agent-to-agent. PARTLY SHIPPED — finish.

`call --pay <amt|auto>` already signs an x402 auth and the sponsored settle pays
the TARGET's TBA (`X402Facet`, EIP-712 exact). `send_lh` already transfers `$LH`
agent-to-agent. So a funded agent CAN fund/pay a new one TODAY. The remaining hole
(memory `project_handoff_smoke_screenshots`): the relay's onboarding-only gate
refused the `transfer()`/`settle` selectors for funded callers — fixed on branch
`fix/send-lh-relay-transfer`, NOT yet deployed (classifier blocked prod). SHIP =
get that proxy allowlist deploy approved. No new design; it's a deploy.

### C-4. Mercury (agent-controlled card → fiat on-ramp). MARK (weak fit).

Mercury is business banking, not an agent-card startup; its AI/MCP layer is
read-only and firewalled from money movement (human approval required). An
approved operator COULD pre-provision a limited virtual card, inject PAN/CVV, and
have the CLI pay the Stripe on-ramp — but that's operator subsidy, not agent
self-pay, it's off-substrate fiat (conflicts with the Tempo+browser, no-off-chain-
infra rule), and it repeats the fragile Stripe webhook path. **MARK only** as a
treasury back-office option; if fiat→`$LH` autonomy is ever truly needed, prefer
agent-card natives (AgentCard/Alchemy, Crossmint, Privacy.com, Stripe Issuing,
Coinbase Agentic Wallets) over Mercury. Not for the agent paying its own fee.

### C-5. Cross-chain USDC / splits.org. MARK (future financial rails).

x402 and Circle CCTP both OMIT Tempo as of mid-2026 — neither bridge reaches our
chain, so external USDC can't natively flow in; lean on the Tempo-native on-ramp
(C-2) instead. 0xSplits V2 (zero-fee, pull-based, ERC-6909 Warehouse, CREATE2
deterministic) is the right colony revenue-split primitive but is NOT on Tempo —
**MARK** for a Tempo port driven by `ScheduleFacet` keeper when colony revenue
splitting becomes real. AP2 Mandates settle via x402 (which we already have) —
**MARK to watch**, no build. Neither is onboarding.

---

## D. Recommendation — phased

**Phase 1 (ship together — the unblock):**
1. Flip `resolve_chain` to default-mainnet + `--dev` flag + invert the two chain
   tests + the capability probe for un-cut facets (A).
2. Ship `localharness onboard [--invite <code>]` reusing headless `invite accept`,
   with the Stripe-link fallback message (B + C-1 Stripe half).
3. Deploy the already-fixed relay allowlist so funded agents can `send_lh`/settle
   (C-3) — get the blocked proxy deploy approved.

This is the smallest change that makes a stock CLI usable by an autonomous agent
on mainnet: an operator pre-funds via one `invite create`, the agent self-onboards
with the code, and the relay sponsors all gas.

**Phase 2 (the autonomous leap):** Tempo MPP charge endpoint (C-2) — the first
truly no-human first-`$LH` path. Decide the USDC.e→`$LH` rate as policy. Harden the
verify like x402.

**Phase 3 / MARK (tracked, not built):**
- **Device-link CLI** (C-1) — terminal inherits a funded web wallet's seed; medium
  effort, human-funds-once convenience.
- **Mercury / agent-cards** (C-4) — off-substrate operator card subsidy; weak fit,
  revisit only if fiat-autonomy is required and prefer agent-card natives.
- **Tempo MPP Sessions + splits.org** (C-2/C-5) — streaming metered billing and
  colony revenue splits; both blocked on unverified primitives (MPP voucher struct;
  splits-on-Tempo port).
- **redeem-without-name** catch-22 relaxation (B) — small `RedeemFacet` change.

**Single highest-leverage next step:** ship Phase 1 step 1+2 in one PR — flip the
default to mainnet AND land `onboard --invite`. Without B, A is a regression
(testnet no-op → mainnet 402); with B, a stock CLI agent goes zero→funded→working
on mainnet using only primitives that already exist.

**Open questions:**
1. RESOLVED — junk `LH_CHAIN` is a HARD ERROR (clean message + exit 2 via
   `validate_chain_env`), never a silent default-to-mainnet. Implemented in A.
2. Should `onboard` auto-claim a subdomain name, or stop at a funded keypair?
   (Name costs `$LH`; claiming eats the grant.) Lean: stop at funded, name is a
   separate explicit step.
3. RESOLVED — MPP USDC.e→`$LH` rate is WEB PARITY: 1 USDC.e = 100 `$LH`, exactly
   the fiat on-ramp peg ($1 = 100 `$LH`). A policy knob set to parity, not a
   reintroduced market peg (0.47.0's `$LH`-vs-`$` decoupling stands for circulation).
4. Relay onboarding-gate vs. self-onboarding agents: the gate refuses callers
   holding >~1 `$LH`. A funded parent running `invite create` is fine (escrow is
   self-pay), but confirm `onboard`'s `invite accept` for a zero-balance new agent
   passes the gate cleanly end-to-end on mainnet before declaring Phase 1 done.
