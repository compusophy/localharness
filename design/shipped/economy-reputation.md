# localharness — Agent Economy + Reputation Layer

> **Track B.** This is the *teeth*: the value-and-trust layer that turns the
> rails (x402, escrow, metering) into an economy where **value flows to agents
> that provide value**. It is explicitly **post-1.0 / mainnet-gated** per
> [`launch-1.0.md`](launch-1.0.md) §8.1 and §10: every mechanism here depends on
> `$LH` having real value, so none of it ships during the testnet betas. This
> doc specifies the design so the 1.0 seams (call_agent x402 hook, scoped keys,
> escrow, the capability descriptor) are cut to *not foreclose* it.
>
> **The 0.x→1.0→2.0 grammar this doc obeys:**
> - **0.x (testnet):** rails proven with `$LH` as *credit* (`currency()=="credits"`,
>   not fee-eligible). x402 settles, meter debits, feedback logs. No stake bites.
> - **1.0 (mainnet):** `$LH` has value + Stripe on-ramp. The **sybil gate** and
>   the first economy mechanism (module-revenue over x402) turn on.
> - **2.0 (this doc, full):** reputation market + validator network + marketplace
>   ranking, gated on the 1.0 economy *pilot signal* (launch §4: "do not invest
>   in Track B until the pilot says someone cares").

The whole design reuses the existing primitives rather than inventing parallel
ones. Concretely: **payment = X402Facet** (already live), **discovery = the
capability descriptor + ERC-721 enumeration** (already live), **trust = escrow +
a new ReputationFacet (ERC-8004-shaped)**, **sybil = a stake/bond gate on
identity creation**. Three new facets, two extended, one off-chain index.

---

## 0. The economic claim, stated precisely

> A composition that uses module M, per use, **pays M's owner**. An agent that
> does useful work (a QA pass that catches a real bug; a module that gets used a
> million times) **accrues reputation and revenue**. A buyer can **discover** the
> right specialist, see its **price and track record**, **pay** it over x402, and
> have a **trust mechanism** (escrow + attestation) that the work happened. Value
> flows down the dependency graph and toward the agents at its leaves.

Four mechanisms make that true, in dependency order:

1. **Module revenue** (§1) — `host::compose` settles a per-use x402 payment from
   the composition to each module dependency. This is the *first* and simplest
   value flow; it ships at 1.0 because it needs no new trust primitive (you see
   the module render, so the "did the work happen" question is trivially
   answered by the host).
2. **Reputation** (§2) — an ERC-8004 `ReputationFacet` accrues
   feedback/validation/reputation registries; validators **stake** to re-execute
   the *verifiable subset* and attest. (2.0.)
3. **Marketplace** (§3) — discovery + pricing + settlement, an off-chain forkable
   index over on-chain capability descriptors + reputation + the x402 rails.
4. **Sybil resistance** (§4) — the gate that makes 1–3 meaningful: identity
   creation costs something fakeable-only-at-a-price. (1.0 requirement.)

§5 draws the **credit-vs-value migration** line and the cutover story.

---

## 1. Module revenue — a composition pays its module deps per use

### 1.1 The flow today vs the flow we want

Today `?compose=foo,bar` (`src/app/compose.rs`) loads each module as a
*read-only* iframe at `<name>.localharness.xyz/?embed=1`. The host decides which
modules to load; modules render their identity card; **nobody pays anybody.** The
peer review and launch spec already gate fine-grained on-chain settlement behind
the micropayment floor (launch §8.3), so the 1.0 version is **coarse and
opt-in**, not a per-frame microtransaction.

The want: when a composition (the *host*) *uses* a module (a *dependency*), the
module's owner gets paid in `$LH`. The unit of "use" is **a compose session**,
not a frame — coarse by design (§8.3 floor).

### 1.2 Mechanism: `host::compose` settles x402 to each dep

A module declares a **per-use price** in its capability descriptor (the signed
`agent.json`-shaped doc, launch §6.2). When a host mounts a module in compose
mode, the host runtime:

1. Reads the module's descriptor (price + payee = the module's **TBA**, resolved
   via `registry::tba_of_name`). The payee is the module's *agent wallet*, not the
   owner EOA — revenue accrues to the identity, exactly like §0 says.
2. Constructs an `X402Challenge` (the existing struct in `src/x402_hook.rs`):
   `to = module TBA`, `value_wei = price`, a host-chosen window + nonce. **The
   caller sets every field** — the existing hook's security model (the comment in
   `x402_hook.rs` lines 14–24) already enforces that the *caller*, not the
   callee, decides `to`/`value`/window; we verify `to` against the descriptor's
   signed payee and cap `value` against the descriptor's signed price.
3. Calls `x402_hook::sign(challenge)` → an `X402Payment`, then
   `registry::settle_x402_sponsored(...)` (already live) to move `$LH`
   host→module-TBA. Sponsored: the host's scoped runtime key signs the sender
   hash; the embedded sponsor pays the Tempo fee.
4. **Only then** does the iframe actually mount (paint gated on settlement, or on
   an escrow lock for higher-value modules — §2.4).

This is **the same call path `call_agent` already uses** — compose is just
"`call_agent`, but the callee renders a UI instead of returning text." No new
payment facet. The diff is: (a) `host::compose` ABI in `display.rs`/`compose.rs`
that performs the settle before mount; (b) descriptor fields `price_wei` +
`payee` (the module's TBA); (c) a host-side **spend budget** so a composition with
N deps can't drain the runtime key (ties into the scoped-key spend-velocity caps,
launch §7).

### 1.3 Recursion: composition-of-compositions

A module can itself be a composition. Because each layer pays its *direct* deps
over x402 to *their TBAs*, revenue flows **one hop at a time down the dependency
graph** — no global accounting, no settlement waterfall. A leaf module used by 1000
compositions is paid 1000 times into one TBA. This is the §0 "value flows down the
graph" property, and it falls out for free from x402 being per-edge. **Caveat
(named, not waved):** deep graphs multiply on-chain settlements → the micropayment
floor (§8.3) bites. 2.0 mitigation: **payment channels** between
frequently-composed pairs (a `ChannelFacet`, batch-settle N uses in one tx). 1.0
stays one-tx-per-use, coarse.

### 1.4 What ships when

| Piece | Version | Why |
|---|---|---|
| Descriptor `price_wei`/`payee` fields | 1.0 | format is a 1.0 seam (launch §6.2) |
| `host::compose` settle-before-mount | 1.0 (gated off by default) | needs value; pilot reads the signal |
| Host spend budget (reuses scoped-key caps) | 1.0 | safety, ships with the dial |
| Payment channels for hot pairs | 2.0 | only worth it past the floor, at scale |

---

## 2. Reputation — an ERC-8004 facet cut into the diamond

### 2.1 Why ERC-8004 and what it actually is

ERC-8004 ("Trustless Agents") standardizes **three registries** an agent economy
needs, on top of plain identity:

- **Identity Registry** — agents have on-chain identity + a resolvable card. **We
  already have this**: the ERC-721 name + ERC-6551 TBA + the capability descriptor
  *are* the identity registry. The 8004 `AgentId` maps to our `tokenId`; the
  card URI maps to `tokenURI(id)` / the descriptor hash.
- **Reputation Registry** — feedback/attestations *about* an agent, authored by
  *clients* who paid for work. **We have a degenerate version**: `FeedbackFacet`
  is a global append-only log. The 8004 shape is feedback **keyed by
  (subject agent, author, job)** with a pointer to off-chain detail. This is the
  extension.
- **Validation Registry** — independent validators **re-execute or attest** a
  claim and stake their reputation/$LH on the verdict. **This is entirely new.**

So the work is: **extend FeedbackFacet into a ReputationFacet** (keyed, scored,
job-linked) and **add a ValidationFacet** (stake + attest), both cut into the
diamond via their own `script/Add<Facet>.s.sol` (the standard pattern, launch /
contracts README). The identity registry needs no new contract — it's a *view
adapter* mapping 8004 selectors onto our existing registry/TBA/descriptor.

### 2.2 `ReputationFacet` (extends, does not replace, FeedbackFacet)

Storage at `keccak256("localharness.reputation.storage.v1")` in a new
`LibReputationStorage`. The shape mirrors FeedbackFacet's append-only `Entry[]`
but **keyed and linked**:

```solidity
struct Attestation {
    uint256 subject;     // tokenId of the agent being rated
    address author;      // who authored it (must have a settled job, §2.3)
    bytes32 jobId;       // links to an escrowed job (§7 launch) or x402 nonce
    int8    score;       // -1 dispute / +1 accept  (lagging signal, not an oracle)
    uint64  timestamp;
    string  detail;      // bounded like feedback (2048B); off-chain detail by URI
}
```

Selectors:
- `attest(uint256 subject, bytes32 jobId, int8 score, string detail)` — **gated**:
  the author must prove they *paid* the subject for `jobId`. Cheap proof at 1.0:
  the author is the `from` of a settled X402Facet authorization (the facet can
  read `LibX402Storage.authState[author][nonce]` if `jobId==nonce`), or the buyer
  of a released escrow. **This is the anti-astroturf gate** — only counterparties
  who actually transacted can rate. Mirrors the "gas is the spam filter" logic of
  FeedbackFacet, but with *proof-of-transaction* as the filter.
- Views: `reputationOf(tokenId) → (uint256 accepts, uint256 disputes)`,
  `attestationsOf(tokenId, start, count)` (paged like `feedbackRange`),
  `attestationCount(tokenId)`.
- ERC-8004 adapter selectors (`getFeedback`, `getReputation`) thin-wrap these so
  an 8004-aware client reads us natively.

**Reputation is a lagging signal, never a stake-weighted oracle at 1.0** (launch
§7): accepts/disputes counts, gameable-at-the-margin, useful at scale. The
**score is unweighted** until §2.3 lands.

**Migration note:** `FeedbackFacet` stays cut in (global product feedback is a
different thing from per-agent reputation). ReputationFacet is *additive*. No
storage collision — different `keccak` storage slot.

### 2.3 `ValidationFacet` (the new, 2.0, stake-bearing piece)

This is where reputation gets **teeth via stake**, and where the launch spec's
hardest constraint (§7: "most agent work is non-deterministic and not
re-executable") forces a **scoped** design. We do **not** claim trustless
re-execution of judgment/creative work. We ship validation only for the
**verifiable subset**:

> A claim is *verifiable* iff a validator can deterministically recompute it: a
> rustlite cartridge that **compiles to the same wasm hash**; an x402 settlement
> that **landed on-chain**; a module whose output is a **pure function of declared
> inputs** (compile, hash, math, format-conversion). Non-deterministic work
> (an LLM answer, a design) routes to **escrow + acceptance** (launch §7), NOT
> validation.

`ValidationFacet`, storage `keccak256("localharness.validation.storage.v1")`:

```solidity
// A claim someone wants validated, e.g. "module X, input I, produced output
// with hash H" (a deterministic claim).
struct ValidationRequest {
    uint256 subject;      // agent/module making the claim
    bytes32 claimHash;    // keccak of (work descriptor || inputs || asserted output)
    uint256 bounty;       // $LH paid to the validator who attests correctly
    uint64  deadline;
}
```

- `requestValidation(uint256 subject, bytes32 claimHash, uint64 deadline)` —
  payable in `$LH` (the bounty), pulled to the diamond like every other
  `transferFrom` cost-gate.
- `stakeAndAttest(uint256 requestId, bytes32 resultHash, uint256 stake)` — a
  validator **bonds `$LH`** and submits its independently-computed `resultHash`.
  Bond is pulled via `transferFrom` (the established pattern).
- `finalize(uint256 requestId)` — if validators **agree** on a `resultHash`
  (deterministic work → they converge), the agreeing validators split the bounty
  and reclaim stake; their `reputationOf` accept-count increments. A validator who
  attested a **minority** hash is **slashed**: stake → the agreeing pool, dispute
  logged against it. This is the only place `$LH` is *destroyed/redistributed* by
  protocol verdict — the slashing that needs real value to mean anything (launch
  §8.1).

Because attestation is **deterministic-agreement**, no oracle and no human
arbiter is in the loop for the verifiable subset — the chain compares hashes. The
non-verifiable majority of work never enters this facet; it uses escrow. **This
respects launch §7 exactly: validation underwrites the checkable subset, escrow
underwrites the rest, and we never conflate them.**

### 2.4 How reputation gates the autonomy dial

Reputation is the **input to the dial** (launch §2.3). A scoped runtime key that
is allowed to "spend ≤X/day to counterparties with `reputationOf.disputes==0` and
`accepts≥N`" turns reputation into an **access-control predicate**, not just a UI
badge. This reuses the existing `policy.rs` `Predicate` trait on the client and a
read of `reputationOf` on-chain — no new mechanism, just wiring reputation into
the scoped-key spend policy. Compose (§1.2) mounts a high-value module only after
escrow when the module's reputation is below threshold.

---

## 3. The marketplace — discovery, pricing, settlement

### 3.1 It is forkable and mostly off-chain (by design)

Per the canonical-vs-forkable line (launch §2.2, §11.4): **the marketplace is
forkable; the primitives it reads are canonical.** A competitor cloning the
front end must still work against the same diamond. So the marketplace is **an
index, not a contract** — it reads on-chain state and serves discovery; it owns
no value and no trust.

### 3.2 The three canonical inputs it indexes

1. **Discovery** — enumerate identities (`registry::list_owned_tokens` /
   ERC-721 enumeration) and read each agent's **capability descriptor** (launch
   §6.2: signed `agent.json`, hash on-chain under a `metadata` key like the
   existing `app.wasm`/`public.html`/`public_face` keys, payload servable from
   `<name>.localharness.xyz`). The descriptor declares: what the agent *does*
   (tags/capabilities), its **price** (`price_wei` per use/job), its **payee**
   (TBA), and an x402 hint. New `metadata` key:
   `keccak256("localharness.capability")`. No new facet — `setMetadata` already
   exists.
2. **Pricing** — read straight from the descriptor (`price_wei`). On-chain truth,
   signed by the owner. The marketplace displays it; it does not set it.
3. **Reputation** — `ReputationFacet::reputationOf(tokenId)` (§2.2) +, at 2.0,
   validation outcomes. The ranking signal.

### 3.3 Settlement is x402, end to end

A buyer picks an agent in the marketplace UI → the UI constructs the x402
challenge from the descriptor's signed payee/price → `x402_hook::sign` →
`settle_x402_sponsored`. **For coarse paid jobs**, settlement wraps in **escrow**
(launch §6.5): payment held, released on acceptance or after the dispute window;
a dispute writes a `-1` attestation (§2.2). The marketplace is the *catalog +
the checkout button*; the chain is the *settlement + the receipt*. This is the
§3.1 forkability test made concrete: every step above is a public RPC call, so a
forked marketplace performs identical calls.

### 3.4 Templates are marketplace entries too

The forkable business templates (launch §5.2) are *static* marketplace entries:
a capability descriptor + a `public_face` + a `price_wei` of 0 (or a one-time
fork fee paid in x402). "Fork this template" = `create_and_publish_app` pointed at
the template's published bytes. Live specialist agents and forkable templates
share **one catalog schema** — the descriptor — so the marketplace lists both
uniformly (launch §5.2: "the catalog of forkable templates AND live, callable
agents").

---

## 4. Sybil resistance — the pre-mainnet gate

### 4.1 Why it is a 1.0 requirement, not a beta one

On testnet `$LH` is valueless credit, so a sybil army earns nothing — **no gate
during the betas** (launch §11.5; just rate-limit the relay and gate the cohort
to trusted invitees). But the moment `$LH` has value + Stripe (1.0/mainnet),
**reputation and module-revenue become farmable**: spin up 10k identities,
cross-attest, self-compose to mint reputation, drain redeem/referral bonuses. So
the sybil gate is a **1.0 launch requirement** (launch §9, §11.5).

### 4.2 Three layered defenses (cost-to-fake, not identity-verification)

We do **not** do KYC (betrays the sovereignty pitch, launch §2.1). We make fake
identities **cost more than they can extract**:

1. **Cost-to-create.** Identity creation (the sponsored mint) requires **one of**:
   - a **refundable bond** in `$LH` staked to the new identity (released on
     name-release; slashed on abuse), OR
   - a **Stripe-card-backed** identity (the card is the scarce resource; the
     on-ramp already exists at 1.0), OR
   - **proof-of-persistence** (an identity that has existed + transacted for T
     accrues trust; brand-new identities have a reputation floor and capped
     earning until they age).

   Mechanism: a `SybilGateFacet` (or an extension to the registration path) that
   makes `register` (currently FREE, `registrationCost==0`) **value-gated on
   mainnet** by setting `registrationCost()` > 0 in `$LH` — **the cost-gate
   already exists** (LocalharnessRegistryFacet pulls `registrationCost()` via
   `transferFrom`; currently 0). The only on-chain change is *turning it on* +
   making it a **refundable bond** rather than a sunk fee (a `bondOf(tokenId)`
   slot + `releaseName` refunds it; abuse slashes it). This is a **small
   extension to RedeemFacet/ReleaseFacet/Registry**, not a new economic engine.

2. **Earning gated by reputation age.** Module revenue (§1) and validation
   bounties (§2.3) to a *fresh, zero-reputation* identity are **capped/escrowed
   longer**. A sybil can't immediately monetize. Reuses §2.2 counts.

3. **Attestation requires proof-of-transaction.** §2.2's `attest` gate (only a
   paid counterparty can rate) **kills cross-attestation farms** unless the
   attacker actually *pays* `$LH` between sybils — which costs real value and
   nets nothing (it's their own money round-tripping minus fees). This is the
   single most important sybil defense and it's **already implied by §2.2's
   design** — sybil resistance and the attestation gate are the same gate.

### 4.3 The relay is the rate-limit chokepoint (1.0)

The sponsorship relay (launch §6.3) is where **rate limits + spend-velocity caps
+ a balance circuit-breaker** live. Pre-mainnet that *is* the whole sybil story
(invite-gated cohort + relay rate limit). At mainnet it's the *first* layer in
front of the §4.2 economic gates. No new contract — relay policy.

---

## 5. Credit vs value — what stays testnet-credit, what becomes mainnet-value

### 5.1 The line

`$LH` on testnet has `currency()=="credits"` → **Tempo rejects it as a
fee_token** (intentional; it's in-system credit, not gas — see CLAUDE.md Tempo
section). Everything in 0.x proves a *mechanism* with credits; everything that
needs **scarcity/slashing/real-cost** waits for the mainnet token with value.

| Mechanism | 0.x (testnet, CREDIT) | 1.0+ (mainnet, VALUE) |
|---|---|---|
| Metered model access (proxy/session/meter) | ✅ proven with credits | same code, real value |
| x402 agent-to-agent settle | ✅ proven (CLAUDE x402 LIVE) | same facet, real value |
| Escrow for paid jobs | rails proven, low stakes | trust mechanism *bites* |
| Module revenue over `host::compose` (§1) | demo only (pilot) | **turns on** — real revenue |
| Reputation attestation (§2.2) | log works, score advisory | gates the dial, ranks market |
| **Validation stake/slash (§2.3)** | ❌ inert (no value to slash) | **2.0 — needs value to mean anything** |
| **Sybil bond/cost (§4.2)** | ❌ none (gate the cohort instead) | **1.0 requirement** |
| Marketplace index (§3) | read-only catalog | catalog + live settlement |
| Stripe on-ramp | ❌ | ✅ the value bridge |

The rule (launch §8.1, verbatim intent): **0.x = rails proven with credits; 1.0 =
value makes them bite; 2.0 = the stake-bearing trust market, gated on pilot
signal.**

### 5.2 The migration story (testnet credit → mainnet value)

There is **no token migration of balances** — testnet `$LH` is abandoned at
cutover, exactly like the 2026-06-01 namespace reset abandoned every prior
address (CLAUDE.md). The migration is of **mechanisms and identities**, not
balances:

1. **Fresh mainnet diamond + `$LH` token + 6551 infra** (same playbook as the
   testnet reset — brand-new addresses, every prior address abandoned). The
   mainnet `$LH` keeps the TIP-20 surface but its value comes from the Stripe
   on-ramp + the fixed/controlled supply, not from a credit faucet.
2. **Mainnet `$LH` currency decision.** Two options, decided at 1.0:
   - *(Recommended)* keep `currency()=="credits"` even on mainnet — `$LH` stays
     an in-system unit (bought with fiat, spent on agents), and **AlphaUSD remains
     the gas fee_token** via the sponsor relay. Users still hold zero gas; the
     economy denominates in `$LH`-credit-with-real-value. This keeps the entire
     sponsored-tx machine unchanged.
   - keep credits-shaped but make `$LH` itself the unit of account only;
     settlement value tracked off the Stripe peg.
3. **Identities re-register.** Names are cheap to re-claim on the mainnet diamond;
   the seed is portable (it's the user's, off-chain), so a user re-derives the
   same wallet and re-registers their name. Reputation **does not carry over**
   (testnet reputation is unstaked/advisory and farmable — starting clean is
   correct). Mainnet reputation accrues from real, value-backed transactions only.
4. **The sybil bond turns on at genesis** (§4.2) so the mainnet namespace is
   cost-gated from block 1 — no sybil land-grab during the reset window.

This is deliberately the **same "abandon and re-mint" discipline** the project
already practices for testnet resets, applied once more at the testnet→mainnet
boundary. No bridge, no balance snapshot, no migration contract to audit.

---

## 6. Concrete contract/code change inventory

**New facets (each via its own `script/Add<Facet>.s.sol`, storage at its own
`keccak256("localharness.<facet>.storage.v1")`):**

- **`ReputationFacet`** (2.0 core; §2.2) — `attest` (proof-of-transaction gated),
  `reputationOf`, `attestationsOf`, `attestationCount`, + ERC-8004 adapter
  selectors. `LibReputationStorage`.
- **`ValidationFacet`** (2.0; §2.3) — `requestValidation`, `stakeAndAttest`,
  `finalize`; stake/slash in `$LH`. `LibValidationStorage`. **The only
  value-destroying facet** — needs mainnet value.
- **`ChannelFacet`** (2.0, post-floor; §1.3) — payment channels for
  hot composed pairs; batch-settle N uses in one tx. Optional, scale-gated.

**Extended (no new facet):**

- **LocalharnessRegistryFacet / RedeemFacet / ReleaseFacet** — turn on
  `registrationCost()` > 0 as a **refundable bond** (`bondOf` slot; refund on
  `releaseName`, slash on abuse). §4.2. The cost-gate path already exists; this
  flips it on + makes it refundable.
- **`setMetadata` key `keccak256("localharness.capability")`** — the capability
  descriptor hash. §3.2. No code change to the facet (generic `setMetadata`).

**No change (reused as-is):**

- **X402Facet** — module revenue (§1) and marketplace settlement (§3.3) are just
  more `settle(...)` calls. The facet already does exactly what's needed.
- **CreditMeter/SessionFacet** — model access metering, orthogonal to the agent
  economy. Unchanged.
- **`src/x402_hook.rs`** — the `X402Challenge`/`sign` surface is exactly the
  compose-revenue surface (§1.2). The caller-sets-every-field model is already
  the right security posture.

**Client (`src/registry.rs` + `src/app/`):**

- `registry::{attest_sponsored, reputation_of, attestations_of}` (mirrors the
  `submit_feedback`/`feedback_range` helpers).
- `registry::{request_validation_sponsored, stake_and_attest_sponsored,
  finalize_validation_sponsored}` (mirror the x402/deposit helpers).
- `registry::{capability_descriptor_of, set_capability_descriptor_sponsored}`
  (mirror `public_face_of`/`set_public_face` over the new metadata key).
- `src/app/compose.rs` — settle-before-mount (§1.2): read descriptor, build
  challenge, `settle_x402_sponsored`, then mount the iframe. Add a host spend
  budget (reuse the scoped-key cap).
- A new `host::compose` ABI surface (analogous to `host::net`/`host::display` in
  `display.rs`) if compositions are also expressed *inside* a cartridge rather
  than only via the `?compose=` URL.

**Off-chain (forkable, no server-of-record):**

- The **marketplace index** (§3) — reads ERC-721 enumeration + descriptors +
  `reputationOf` and serves discovery. Forkable; owns nothing. The one accepted
  off-chain component remains the credit proxy; the marketplace index is a *read*
  layer that any client (including a fork) can recompute from chain state.

---

## 7. The boundary, restated (what this doc does NOT authorize building now)

Per launch §10's A/B gate — **do not start any of §2.3, §3 live-settlement, or
§4.2 bond until: (a) 1.0 shipped on mainnet (value exists) AND (b) the economy
pilot showed someone cares.** What 1.0 ships from this doc is only the **seams**:

- the **capability descriptor format + metadata key** (§3.2) — a 1.0 seam,
- the **`ReputationFacet` keyed-attestation** as an *additive* facet whose score
  is advisory (§2.2) — cuttable at 1.0 without the stake layer,
- the **sybil bond** flipped on at the mainnet boundary (§4.2) — a 1.0 mainnet
  requirement,
- compose's **settle-before-mount path present but gated off** (§1.4).

The stake-bearing validation market (§2.3), payment channels (§1.3), and live
marketplace settlement at scale (§3.3) are **2.0**, gated on the pilot. This doc
exists so those are *additive cuts to an unchanged diamond*, never a rewrite —
which is exactly the launch spec's promise (§10: "Steps A1–A6 deliberately leave
every door open so starting B later is additive, not a rewrite").
