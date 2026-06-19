# CLI ↔ mainnet — runtime chain selection + rate-capped sponsor relay

> STATUS: phases 1-4 SHIPPED (CLI). Money-sensitive + cross-cutting.
> Supersedes the "owner's call" framing in `design/chain-coherence.md` (option C
> + the relay half of option A) and fleshes out `design/stripe-mainnet.md` §6.3
> (the "rate-capped relay") and step 13.
>
> **Shipped:** §2.1 runtime `chain::active()` (0.48.0, commit 297b262). §2.2 the
> relay endpoint `proxy/api/sponsor.ts` + the TS wire-port `proxy/api/_tempo.ts`
> (pinned to the Rust golden vectors, `proxy/test/`). §2.2 CLI client
> `registry::sponsor_relay` — `registry::tx`'s sponsored-submit chokepoints route
> the fee_payer half through the relay when `chain::active()` is mainnet; the CLI
> `LH_MAINNET_SPONSOR_KEY` env arm is gone (no CLI build carries a mainnet key).
> §3-L the x402/mint-gate domain tests are chain-agnostic.
>
> **Remaining (gated):** the BROWSER `src/app/sponsor.rs` still `env!`s a mainnet
> key under `cfg(mainnet)` (the live web bundle) — its single fee_payer chokepoint
> (`run_sponsored_tempo_call`) must move to the relay via an iframe-built auth
> token (+ web redeploy). A real ON-CHAIN E2E + setting/funding the mainnet
> sponsor key (phases 5-6) stay behind the §4 security checklist + user sign-off.

The published `localharness` CLI cannot transact against the live **mainnet**
platform, and the document an external agent reads (`web/skill.md`) tells it to
`cargo install localharness` and start onboarding. This proposes the two coupled
fixes that close the gap **without** shipping a money-moving key in the crate.

---

## 1. Problem statement (grounded in the code)

### 1.1 The chain is baked in at compile time

`src/registry/chain.rs` selects the active network with a **compile-time const**:

```rust
#[cfg(not(feature = "mainnet"))]
pub const ACTIVE: ChainConfig = MODERATO;   // chain 42431, diamond 0x6c31…
#[cfg(feature = "mainnet")]
pub const ACTIVE: ChainConfig = MAINNET;     // chain 4217,  diamond 0x8ab4…
```

`src/registry/mod.rs` then exposes the five canonical handles as `pub const`,
each a field projection of `ACTIVE`:

```rust
pub const RPC_URL: &str               = chain::ACTIVE.rpc_url;          // mod.rs:70
pub const REGISTRY_ADDRESS: &str      = chain::ACTIVE.diamond;          // mod.rs:79
pub const CHAIN_ID: u64               = chain::ACTIVE.chain_id;         // mod.rs:82
pub const LOCALHARNESS_TOKEN_ADDRESS: &str = chain::ACTIVE.lh_token;    // mod.rs:102
```

plus `pub const ALPHA_USD_ADDRESS: &str = super::chain::ACTIVE.fee_token;`
(`src/registry/tx.rs:148`). These are consumed **pervasively** — the RPC layer
posts to `RPC_URL` (`rpc.rs:111/246/540/597/646`), every sponsored tx is built
with `TempoTxBuilder::new(CHAIN_ID)` (`tx.rs:167/219/267`), every diamond call
targets `REGISTRY_ADDRESS` (`rpc.rs:150`, `tx.rs:339/389`), and the **x402 EIP-712
domain separator binds `CHAIN_ID` + `REGISTRY_ADDRESS`** (`x402.rs:31-42`). Because
they are `const`, the chain is frozen at `cargo build` time. There is no runtime
switch.

### 1.2 The mainnet fee_payer key is a compile-time `env!` — and the binary embeds it

A mainnet build needs a *funded* `fee_payer` to sponsor user gas (users hold zero
of anything). The key is pulled at **compile time**:

- `src/app/sponsor.rs:58-59` — `#[cfg(feature="mainnet")] const SPONSOR_PRIVATE_KEY_HEX: &str = env!("LH_MAINNET_SPONSOR_KEY");`
- `src/bin/localharness/main.rs:171-174` — the now chain-gated CLI twin:
  `#[cfg(feature="mainnet")] const SPONSOR_KEY: &str = env!("LH_MAINNET_SPONSOR_KEY");`
  (testnet falls back to the committed const that derives `0x0aff88…a922c`).

`env!` keeps the key out of the repo, so an external agent doing `cargo install
localharness --features mainnet` can't even compile (the env is unset → build
fails closed). But that is the *wrong* failure: it means **no published artifact
can target mainnet**, because the only artifact that *could* — one we build with
the env set — would **embed a real, money-moving mainnet fee_payer in every
download**. `sponsor.rs`' own trust note (lines 5-17, 51-57) says the embedded-key
model is "accepted on testnet … and **must change before mainnet**"; the proper
fix is "the rate-capped relay (stripe-mainnet §6.3)."

### 1.3 The consequence (confirmed by dogfooding)

`web/skill.md` ("Get live in one command") routes every external agent to
`cargo install localharness --features wallet` — a **default-features, testnet**
build. So a stock CLI:

1. Signs every on-chain intent with `CHAIN_ID = 42431` and targets the **testnet**
   diamond. Its x402 domain separator (`x402.rs`) is the testnet separator.
2. Hits the **live proxy**, which is mainnet (chain 4217 — `proxy/api/_chain.ts`
   env flips `REGISTRY`/`CHAIN_ID`/`LH_TOKEN`, `/prices` confirms 4217). The auth
   token `address:timestamp:signature` (`registry::proxy_auth_token`, `names.rs:751`;
   preimage `localharness-proxy:<addr>:<ts>`) recovers fine — it isn't chain-bound —
   but the proxy then gates on **mainnet** `creditOf`/session, which a testnet-funded
   identity does not have → `402`.

This is exactly the split documented in `design/chain-coherence.md` (point 2:
"a vanilla `cargo install` agent likely **cannot complete a paid `call`** against
the live proxy"). It was confirmed earlier this session: a paid `call` and a
sponsored mainnet `create` both failed on a stock build, and worked **only** after
a local `--features mainnet` build with `LH_MAINNET_SPONSOR_KEY` set (the fix that
landed `main.rs:171`). That local build is not reproducible by an external agent —
which is the whole point of this doc.

The two faults are coupled: **(a)** a published binary can't *select* mainnet
(§1.1), and **(b)** even if it could, it must not *embed* the sponsor key (§1.2).
Fix one without the other and you either ship a testnet-only CLI against a mainnet
proxy, or leak a hot key in every download.

---

## 2. The two coupled fixes

### 2.1 (a) Runtime chain selection — one published binary, `LH_CHAIN` selects the network

**Goal:** `cargo install localharness` produces ONE binary that targets testnet by
default and mainnet when the operator sets `LH_CHAIN=mainnet` (env), with no
recompile and no `mainnet` cargo feature on the published artifact.

#### What must become runtime vs stay const — honest blast-radius audit

I enumerated every `chain::ACTIVE` / canonical-const consumer (`grep` across
`src/`). Three tiers:

**Tier 1 — trivially runtime-able (the bulk).** Everything that reads `RPC_URL` /
`REGISTRY_ADDRESS` / `CHAIN_ID` / `LOCALHARNESS_TOKEN_ADDRESS` / `ALPHA_USD_ADDRESS`
**inside a function body**. These are already lazy reads of a `const`; swapping the
`const` for an accessor call is mechanical:

- `src/registry/rpc.rs` — `rpc_value`/`eth_call_batch`/`fetch_revert_reason`/
  `wait_for_receipt`/`receipt_contract_address` post to `RPC_URL`; `read_view`
  targets `REGISTRY_ADDRESS`.
- `src/registry/tx.rs` — `TempoTxBuilder::new(CHAIN_ID)` ×3, `sponsored_diamond_call`
  / `escrow_call_batch` use `REGISTRY_ADDRESS` + `LOCALHARNESS_TOKEN_ADDRESS`,
  `ALPHA_USD_ADDRESS`.
- `src/registry/x402.rs` — `x402_domain_separator()` reads both `REGISTRY_ADDRESS`
  and `CHAIN_ID` (this is the load-bearing one: get it wrong and every signature is
  invalid on the target chain).
- `src/app/self_docs.rs:135`, `src/app/chat/prompt.rs:46` — `chain::ACTIVE.name`
  for prompt/self-doc text.

**Tier 2 — const/static contexts that CANNOT call a runtime accessor (the real
cost).** A `const` initializer can only call `const fn`, and a runtime accessor
that reads an env var or a process global is not `const`. These are the items that
block a naive "just make `ACTIVE` a function":

- `src/registry/mod.rs:70/79/82/102` — the five `pub const` handles themselves.
  Their *type* changes from `const &str`/`const u64` to `fn() -> &'static str` /
  `fn() -> u64`. **This is the source of the whole API ripple**: every Tier-1 site
  becomes `REGISTRY_ADDRESS()` instead of `REGISTRY_ADDRESS`.
- `src/registry/tx.rs:148` — `pub const ALPHA_USD_ADDRESS` (same treatment).
- `src/registry/multichain.rs:45-60` — `pub const CHAINS: &[EvmChain]` is an array
  **literal** that inlines `super::chain::ACTIVE.rpc_url` / `.chain_id` for the
  `"tempo"` entry. A `const` slice cannot hold a runtime value. This must become a
  function that builds the slice (or a `Vec`) at call time, or the `tempo` row must
  be resolved at lookup time in `chain_by_name`.

**Tier 3 — `#[cfg(feature="mainnet")]` attributes that disappear entirely.** With
runtime selection there is no `mainnet` feature, so these get rewritten as runtime
branches or runtime-aware tests:

- `src/app/sponsor.rs:58` and `src/bin/localharness/main.rs:171` — the embedded-key
  `cfg`. **These are replaced by the relay (§2.2), not by a runtime key switch** —
  we are explicitly NOT shipping a runtime-selectable embedded mainnet key.
- `src/registry/mint_gate.rs` — a `#[cfg(feature="mainnet")]` gate.
- `src/registry/x402.rs:345/362` — `#[cfg(not(feature="mainnet"))]` on the pinned
  domain-separator tests. Per stripe-mainnet §7-L these should READ the live
  `x402DomainSeparator()` and compare, so they pass on whichever chain is active.
- `src/bin/localharness/main.rs:967-970` (sponsor-address expectation) and the
  `llms_txt_publishes_canonical_onchain_constants` test — currently `cfg`-split;
  become runtime-aware (assert against `chain::active()`).

**Count:** the five canonical handles feed roughly the figure stripe-mainnet §3.2
cites ("102 consumers / 27 files"). The good news from the audit: that count is
dominated by Tier 1 (call-site reads), which is a mechanical `X` → `X()` rename.
The genuinely-thoughtful work is the **~6 Tier-2/3 sites** above.

#### Recommended mechanism: a process-start global behind a `chain::active()` accessor

Two viable shapes:

- **Thread an accessor everywhere** (`chain::active() -> &'static ChainConfig`,
  reading a `OnceLock<ChainConfig>` initialized from `LH_CHAIN` on first use). The
  five `mod.rs`/`tx.rs` handles become thin `fn`s over it; `multichain::CHAINS`
  becomes `chains()`. **Recommended.**
- **A `build.rs` + `LH_CHAIN`** (compile-time, mentioned in stripe-mainnet §5).
  Rejected for the published-binary goal: it bakes the choice at build time again,
  so it doesn't give us ONE binary that an operator can point at either chain. Keep
  `build.rs`/feature only as the *web bundle's* mechanism (the wasm app has no env;
  it stays `--features mainnet`).

**`chain::active()` design:**

```rust
// chain.rs
static ACTIVE_CHAIN: OnceLock<ChainConfig> = OnceLock::new();

/// The active chain, resolved ONCE at first read. Native: `LH_CHAIN` env
/// ("mainnet"/"moderato"/"testnet"; default MODERATO). wasm: compile-time
/// (`#[cfg(feature="mainnet")]`) — the browser has no env, and the bundle's
/// chain is fixed at build by build-web.sh.
pub fn active() -> &'static ChainConfig {
    ACTIVE_CHAIN.get_or_init(|| {
        #[cfg(target_arch = "wasm32")]
        { #[cfg(feature="mainnet")] { MAINNET } #[cfg(not(feature="mainnet"))] { MODERATO } }
        #[cfg(not(target_arch = "wasm32"))]
        { match std::env::var("LH_CHAIN").as_deref() {
            Ok("mainnet") => MAINNET,
            _ => MODERATO,
        } }
    })
}
```

**Safety properties this preserves:**

- **Resolve-once.** A `OnceLock` read on first use means the chain can't flip
  mid-process (a tx signed for 4217 then submitted to a node reconfigured to 42431
  would be a silent money/identity bug). The whole process is one chain for its
  lifetime. *Caveat:* a test that wants to exercise both presets must call the
  pure-data constructors (`MODERATO`/`MAINNET`) directly, never `active()` — keep
  those `pub`.
- **Default is testnet.** Env unset → `MODERATO`, byte-for-byte unchanged. The
  `mainnet` cargo feature can stay as the wasm bundle's selector during migration
  and be retired from the CLI.
- **Fail-loud on a live-empty preset.** If a `MAINNET` field is ever empty (the
  pre-deploy guard, chain.rs:33-37), `active()` callers must surface a clear
  "mainnet not deployed" error rather than transact against a zero address.

**Migration ordering (so it lands without a flag-day):** keep the existing `pub
const` names as a *deprecated compatibility shim* during the rename — e.g. provide
`pub fn registry_address() -> &'static str` and convert call sites in batches,
deleting the consts last. This is purely a release-hygiene note; the end state has
no const.

**Proxy side is already done.** `proxy/api/_chain.ts` is runtime (`process.env`
with Moderato defaults). No proxy code change for chain selection — only the
*relay* endpoint below is new.

### 2.2 (b) Rate-capped sponsor relay — fee_payer signing moves server-side

**Goal:** the CLI (and any client) gets its Tempo tx sponsored on mainnet WITHOUT
the published crate containing a fee_payer key. The proxy — already the one
deliberate server — holds the mainnet sponsor key and signs `fee_payer` on demand,
behind hard abuse caps, authed by the caller's *existing* personal-sign token.

Why this fits the wire format: a Tempo `0x76` sponsored tx is signed in two
independent places — the **sender** signs the intent (the CLI's identity key, no
funds needed) and the **fee_payer** signs the fee authorization (`tempo_tx::
sign_sponsored`, the fee-payer hash is `keccak256(0x78 || rlp([…]))` per CLAUDE.md
"Wire format"). The two signatures are over disjoint preimages, so the fee_payer
signature can be produced by a *different party* than the sender. The relay is that
party. CLAUDE.md confirms Tempo access keys **cannot** sign as `fee_payer` (only
the root key can), which is exactly why today's design embeds the key — the relay
keeps the root key server-side instead.

#### Endpoint: `POST <proxy>/api/sponsor`

**Auth.** Reuse the existing token verbatim: header `x-goog-api-key:
<address>:<timestamp>:<signature>`, signature = personal-sign over
`localharness-proxy:<address>:<timestamp>` (the exact preimage `gemini.ts:690`,
`names.rs:753` already share; freshness `FRESHNESS_WINDOW_SECS = 300`). No new auth
surface — the CLI calls `registry::proxy_auth_token(&signer, now)` it already uses
for `call`. The recovered address is the **caller identity** the caps key on.

**Request.** The relay signs the *fee-payer half only* — it must NOT be handed a
fully-built tx it blindly signs, or it becomes an open fee-payer oracle. Instead the
caller submits the **sender-signed intent fields**, the relay independently
re-derives the fee-payer hash from them, applies the allowlist/cap checks against
the *decoded calls*, signs `fee_payer`, and returns the signature for the CLI to
assemble + submit. Shape:

```jsonc
// request body
{
  "chainId": 4217,
  "calls": [ { "to": "0x…", "value": "0", "input": "0x…" }, … ],  // the inner calls
  "nonceKey": "0", "nonce": "42",
  "gasLimit": "1200000",
  "maxFeePerGas": "…", "maxPriorityFeePerGas": "…",
  "validBefore": 0, "validAfter": 0,
  "senderAddress": "0x…",          // MUST equal the recovered auth address
  "senderSignature": "0x…(65)"     // the caller's sender-half sig (lets the relay
                                   //   verify it's signing for a tx the caller
                                   //   already committed to — no blind signing)
}
```

```jsonc
// response (success)
{
  "feePayer": "0x066E748367df1c2bfEdA9C445fBaAa093e10168f",
  "feeToken": "0x20c0…",           // USDC.e on mainnet
  "feePayerSignature": "0x…",      // rlp([v,r,s]) per the wire format
  "feePayerHash": "0x…"            // what was signed, so the CLI can verify locally
}
// response (refused)
{ "error": "selector 0x… not in sponsor allowlist", "code": "LH_RELAY_SELECTOR" }
```

The CLI assembles the final `0x76` tx from `senderSignature` + `feePayerSignature`
and submits via its own RPC (`eth_send_raw_transaction`) — the relay never touches
the chain for sends, only signs. (This keeps the relay stateless w.r.t. submission
and means a relay outage degrades to "can't get sponsored," never "tx half-sent.")

#### Abuse caps (all three, enforced server-side before signing)

The embedded-key trust model bounded loss at "the sponsor wallet's small balance"
(sponsor.rs:40-45). The relay must bound it at least as tightly, because the key is
now reachable to *anyone with an identity* rather than anyone who extracts the
bundle:

1. **Selector allowlist.** Decode each `calls[].input` selector and refuse anything
   not on a curated list of *sponsorable* operations: `register`, `setMetadata`
   (the publish/persona/price path), `createTokenBoundAccount`, the escrow/approve
   pair, `submitFeedback`, schedule/invite/bounty writes — i.e. the onboarding +
   participation surface. Refuse `transfer`/`approve`-to-arbitrary, raw value sends,
   and any call whose `to` is not the diamond / $LH token / a known TBA. This stops
   the sponsor from paying gas for an attacker's unrelated mainnet activity. (Mirror
   the set the browser already sponsors via `events::run_sponsored_tempo_call`.)
2. **Per-address rate limit.** Reuse `proxy/api/_ratelimit.ts SlidingWindow`, keyed
   on the recovered address (post-auth, unlike the pre-auth notify limiter — a fee
   signature is valuable, so verify first). E.g. N sponsorships per address per
   hour + a global per-isolate ceiling. Honest limitation (documented in
   `_ratelimit.ts:12-20`): per-isolate, not global — acceptable as the *rate* floor;
   the spend ceiling below is the real wall.
3. **Per-address spend ceiling.** Track cumulative *sponsored gas cost* per address
   against a hard lifetime/window cap (e.g. "enough to onboard + a few writes, then
   you fund your own"). Because Edge isolates share no state, the durable ceiling
   needs the **on-chain** signal that already exists: gate sponsorship on whether
   the address has *ever* been funded / has a positive mainnet meter or wallet
   balance — i.e. sponsor only the genuinely-new, zero-balance onboarding writes,
   and once an agent has $LH it pays its own fees (the platform already debits
   compute via the meter). This converts "spend ceiling" into a near-stateless
   "first-N-onboarding-ops-only" gate, sidestepping the no-off-chain-infra rule.
   `gasLimit * maxFeePerGas` is also bounded by the existing `clamp_gas_price` /
   `MAX_GAS_PRICE_WEI` discipline (tx.rs:50) — the relay re-clamps server-side.

#### Failure modes

- **Refused (cap/allowlist/auth):** structured 4xx with a stable `LH_RELAY_*` code;
  the CLI surfaces "sponsorship unavailable — fund <addr> with mainnet $LH (`buy`)
  to self-pay" rather than a raw 500.
- **Relay down / 5xx:** the CLI must NOT silently fall back to a self-paid tx (that
  reintroduces the zero-funds break + the native-transfer ban). Surface the outage;
  the sponsored write simply isn't available this moment. (A funded agent could opt
  into a self-paid Tempo tx via `submit_tempo_self_paid`, but that's an explicit,
  funded path, not a fallback.)
- **Sender/auth mismatch:** `senderAddress != recovered` → reject (can't sponsor a
  tx you didn't sign).
- **Sponsor float exhausted:** the fee_token balance monitor (the same one
  sponsor.rs:24-35 documents for testnet) alarms; relay returns a clear "sponsor
  underfunded" code. This is now a single server wallet to keep topped up, not a
  bundle-embedded one.
- **Replay:** the freshness window bounds auth-token reuse; the tx nonce
  (sender's 2D nonce) makes a replayed *signed tx* a chain-level no-op anyway.

#### What this buys

- The published crate contains **no money-moving key** — `main.rs:171` and
  `sponsor.rs:58` lose the mainnet `env!` arm; the CLI calls the relay for the
  fee-payer half.
- Loss is bounded by the allowlist + onboarding-only gate, not by "whoever extracts
  the bundle drains the float."
- It is Tempo-native (signs the real `0x76` fee-payer half), reusing the existing
  auth, rate-limit, and gas-clamp infrastructure — no new trust primitive.

---

## 3. Rejected alternatives

- **Tempo access keys signing `fee_payer`.** CLAUDE.md ("Access keys can't sign
  fee_payer", confirmed from Tempo's SDK; sponsor.rs:13-17 lists it as a "TBD"
  that the live test settled): access keys sign the **sender** half only; the
  `fee_payer` authorization requires the root key. So a scoped access key can't
  replace the embedded sponsor — there is no Tempo-native way to delegate fee_payer
  to a low-privilege key. The relay keeps the root key server-side instead, which
  is the only place it can live.
- **A 4337 paymaster with EntryPoint policy.** This reinvents the relay on top of a
  parallel account-abstraction stack that Tempo's **native** AA (`0x76`) already
  supersedes. We'd be running an EntryPoint + paymaster contract + bundler to do
  what the native fee_payer field does in one signature, and we'd still need a
  server holding the paymaster's funding key with the same caps. Strictly more
  moving parts, same trust model, fights the chain's native primitive. (sponsor.rs:17
  lists it as a candidate; rejected on cost/alignment.)
- **Self-funded gas (the agent pays its own fees).** Breaks the zero-funds
  onboarding promise that is the platform's entire pitch (`skill.md`: "you need no
  wallet, no gas, no funds") — a brand-new identity has nothing to pay with. Worse,
  on Tempo there is no native coin and **EOA↔contract native value transfers are
  banned** (MEMORY: "Tempo no native transfers"), so even a funded agent can't
  trivially self-pay in native gas; fees are paid in a USD TIP-20 the new agent
  doesn't hold. Self-funding is the *graduated* path (once an agent earns $LH it
  pays its own way), not the onboarding path.
- **WebAuthn passkey-per-user (each user is their own sponsor).** Listed in
  sponsor.rs:16. Doesn't apply to a headless CLI (no browser passkey ceremony) and
  still leaves the new-agent-has-no-funds problem. Browser-only, out of scope here.

---

## 4. Phased build plan

Ordered so each phase is independently shippable and the money path is gated last.

1. **[✅ DONE — 0.48.0] Runtime `chain::active()` accessor + Tier-1/Tier-2 rename.**
   Add the `OnceLock`-backed `active()` (§2.1); convert the five `mod.rs`/`tx.rs`
   consts to accessor `fn`s (keep deprecated const shims during the rename);
   convert `multichain::CHAINS` to a function. Default env-unset = byte-for-byte
   Moderato. Verify: full registry suite + wasm32 (SDK+wallet) + the existing
   "active is moderato by default" test, plus a new "`LH_CHAIN=mainnet` selects
   4217" test. **No money path touched** — testnet still default, no relay yet.
2. **[✅ DONE] Tier-3 test/cfg cleanup.** Make the x402 domain test READ the live
   `x402DomainSeparator()` (stripe-mainnet §7-L) so it's chain-agnostic; make the
   `llms_txt` + sponsor-address tests runtime-aware off `chain::active()`. Retire
   the `mainnet` cargo feature from the *CLI* (wasm bundle keeps it until phase 6).
3. **[✅ BUILT + offline-verified; live deploy + on-chain E2E pending] Relay
   endpoint (`proxy/api/sponsor.ts`), TEST chain.**
   Implement auth (reuse `recoverAddress` + freshness), the selector allowlist,
   the `_ratelimit.ts` window, the fee-payer-hash re-derivation, and the
   onboarding-only spend gate — pointed at **Moderato** with the *testnet* sponsor
   key. Prove end-to-end against testnet: CLI gets a fee-payer sig from the relay
   instead of the embedded key, assembles + submits a sponsored `create`. This
   exercises the entire money path with play-money before any mainnet key exists.
4. **[✅ DONE (CLI); browser `sponsor.rs` deferred] CLI relay client + remove the
   embedded mainnet `env!`.** Add the
   `<proxy>/api/sponsor` client to the crate; route sponsored writes through it when
   the active chain is mainnet (testnet keeps the committed local sponsor const for
   the dev sandbox, OR also routes the relay — decide in review). Delete the
   `#[cfg(feature="mainnet")] env!("LH_MAINNET_SPONSOR_KEY")` arm in both
   `main.rs:171` and `sponsor.rs:58`. After this, **no build of the crate embeds a
   mainnet money key.**
5. **[GATED: security review of the money path] Sign-off checklist (below).** Must
   pass before the relay holds a *real* mainnet key.
6. **[GATED: review + funded mainnet sponsor] Go live.** Set the mainnet sponsor
   key in the proxy env (NOT the crate); fund it with mainnet fee_token (USDC.e);
   point the proxy `_chain.ts` env at 4217 (already supported); rebuild the wasm
   bundle `--features mainnet` (the browser path); update `skill.md` (§5).

### Security review checklist (the money path — phase 5 gate)

- [ ] **No blind signing.** The relay re-derives the fee-payer hash from the
      submitted intent fields and verifies `senderSignature` recovers
      `senderAddress == recovered auth address`; it never signs a caller-supplied
      opaque hash.
- [ ] **Selector allowlist is default-deny.** Unknown selector, or `to` not in
      {diamond, $LH token, known TBA factory/account}, is refused. Reviewed against
      the browser's `run_sponsored_tempo_call` sponsorable set so the two agree.
- [ ] **Gas re-clamped server-side.** `gasLimit * maxFeePerGas` bounded by
      `MAX_GAS_PRICE_WEI`-equivalent; absurd limits refused (mirror `clamp_gas_price`).
- [ ] **Onboarding-only spend gate.** Sponsorship gated on a zero/near-zero mainnet
      balance for the caller; funded agents self-pay. Documented as the durable
      ceiling given per-isolate rate-limit limits.
- [x] **Sponsor key custody.** Mainnet sponsor is a dedicated low-budget EOA
      `0x066E748367df1c2bfEdA9C445fBaAa093e10168f`, distinct from
      deployer/owner/issuer/meter (verified). Lives ONLY in the proxy env
      (`LH_SPONSOR_KEY`), never in the bundle. ROTATED from `0xE70f4B…065E` (which
      had been exposed in earlier bundles — funds rescued, ~0.007 USDC.e dust left
      on the dead key). A proxy RCE leaks a *cap-bounded signing oracle*, not an
      uncapped key. (KMS/secret-store custody is a future hardening.)
- [ ] **Float monitor + circuit breaker.** Alarm when fee_token balance is low;
      a tripped breaker refuses new sponsorships with a clear code (never silently
      self-pays).
- [ ] **Auth replay bounded.** 300s freshness window; per-address rate window;
      Tempo 2D-nonce makes a replayed signed tx a no-op.
- [ ] **Fail-closed, never fall back to self-paid silently** (the zero-funds +
      native-transfer-ban trap).
- [ ] **Chain coherence asserted.** Relay refuses a `chainId` that doesn't match its
      configured chain; the CLI's `CHAIN_ID` (now `chain::active().chain_id`), the
      relay's chain, and `_chain.ts` all agree (stripe-mainnet §6 "three-place drift").

### How `skill.md` changes once this lands

Today `skill.md` says "Tempo **Moderato testnet**", "free and sponsored", and
`cargo install localharness --features wallet`. After phase 6:

- **Drop the testnet framing for the default onboarding path** (or state both): the
  one command becomes `cargo install localharness` (no `--features mainnet`, no key
  needed — the binary selects mainnet via `LH_CHAIN=mainnet`, or mainnet becomes the
  CLI default and testnet is `LH_CHAIN=moderato`). The "free and sponsored" claim
  stays true **for onboarding writes** because the relay sponsors them; add a line
  that participation beyond onboarding is paid in $LH (buy/earn), matching the
  mainnet economy `chain-coherence.md` point 1 describes.
- **`llms.txt` re-stamps mainnet constants** — the `llms_txt` test (now runtime-aware)
  reads `chain::active()`, and `build-web.sh` builds the bundle on the mainnet preset,
  so the agent-facing diamond/token/RPC/chainId are the **live mainnet** values, not
  testnet (fixes `chain-coherence.md` point 3).
- **No instruction to set `LH_MAINNET_SPONSOR_KEY`** ever appears — the key lives in
  the proxy, and the external-agent flow never sees it.

---

## 5. Recommended path (summary)

Ship **runtime chain selection** (`chain::active()` + env `LH_CHAIN`, one published
binary) **and** the **rate-capped sponsor relay** (`proxy/api/sponsor.ts` signs the
fee-payer half server-side, authed by the existing personal-sign token, behind a
selector allowlist + per-address rate window + onboarding-only spend gate). Reject
access keys (can't sign fee_payer), 4337 (reinvents the relay), and self-funded gas
(breaks zero-funds onboarding + the native-transfer ban). Land the runtime-chain
refactor + relay-on-testnet first (all play-money safe), remove the embedded mainnet
`env!` from the crate, then gate the real mainnet key behind the security checklist.
The end state: a stock `cargo install localharness` transacts on live mainnet, the
crate ships no money-moving key, and `skill.md` stops sending agents to a chain the
proxy doesn't meter.
