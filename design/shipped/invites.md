# localharness — User-Created Invite System (`$LH`-escrowed invites)

> **STATUS: SHIPPED** — bearer `InviteFacet` (create / accept / reclaim) is cut +
> LIVE and proven E2E; CLI `invite create/accept/reclaim/list` and the `?invite=`
> auto-redeem UX ship (0.27.0). Kept for the three-state-machine + supply-neutrality
> + front-running reasoning. STILL OPEN (Phase 2): **bound vouchers**
> (recipient-bound, front-run-proof), batch reclaim, and local code persistence.
>
> *(Original pre-implementation triage note, preserved below.)*

> **Status: DESIGN ONLY — no code.** This is the triage/plan the user asked for
> *before* any implementation. It specifies a new `InviteFacet`, its storage,
> the lifecycle, the security model, the `?invite=` UX, and a phased plan with
> the genuine open questions called out. Read alongside
> [`economy-reputation.md`](economy-reputation.md) (the post-1.0 economy seams)
> and [`launch-1.0.md`](launch-1.0.md) (the 0.x→1.0→2.0 grammar).

## 0. The ask, stated precisely

Verbatim user intent:

> "Eventually users will be able to spend `$LH` to invite others (invite expires
> and returns the prefunded `$LH`). Even cooler if I could send different invite
> codes with different redeem amounts — I don't want to give everyone 1000 `$LH`
> but I do want to give it to some people I trust; 10 or 100 is more
> appropriate." Plus: `?invite=CODE` links auto-onboard the recipient.

Decomposed into hard requirements:

1. **Anyone** (not just the diamond owner) can create an invite, funded from
   **their own `$LH`**.
2. The funding `$LH` is **escrowed** the moment the invite is created (it leaves
   the inviter's spendable balance — this is the "spend `$LH` to invite"
   property).
3. The invite carries a **chosen amount** — 10 / 100 / 1000, or arbitrary —
   because trust is graded (give trusted people more).
4. The invite **expires**. On expiry, if unclaimed, the escrowed `$LH` **returns
   to the funder**.
5. A recipient redeems via the **same `?invite=CODE` link UX that already
   exists** — they land on a subdomain, the code auto-accepts, they're credited.

This is the **user-facing evolution of `RedeemFacet`**. RedeemFacet is
owner-only and *mints* fresh `$LH` from `ISSUER_ROLE`. The invite system lets
*any holder* move *their existing* `$LH` to a newcomer, with escrow + expiry +
refund that RedeemFacet has no concept of. They are siblings, not replacements
(§6).

---

## 1. On-chain design — a new `InviteFacet`

### 1.1 New facet, not an extension of RedeemFacet

**Decision: new `InviteFacet` + `LibInviteStorage`.** Reasons:

- **Different funding model.** RedeemFacet codes **mint** via `ISSUER_ROLE`
  (`ILocalharnessCredits.mintWithMemo`, `RedeemFacet.sol:82`) — they create
  supply. Invites **escrow existing supply** (`transferFrom` funder→diamond) and
  later either pay it out to the accepter or refund it to the funder. No minting,
  no `ISSUER_ROLE`. Welding an escrow lifecycle onto the mint facet would
  entangle two different value semantics in one storage lib.
- **Different authority.** RedeemFacet is `enforceIsContractOwner` everywhere
  except `redeem`. InviteFacet is **permissionless to create** (gated only by the
  funder having the `$LH` + approval). Mixing owner-only and open functions in
  one facet invites a selector-collision footgun.
- **Storage isolation is the diamond convention** (CLAUDE.md on-chain stack):
  each facet at `keccak256("localharness.<facet>.storage.v1")`. A fresh
  `LibInviteStorage` at `keccak256("localharness.invite.storage.v1")` collides
  with nothing already cut.
- The hashing primitive is **identical** — `keccak256(bytes(code))`, with only
  the hash on-chain and the plaintext distributed off-chain — so we keep the
  exact mental model `add-redeem-codes.sh` already documents
  (`RedeemFacet.sol:72`; `add-redeem-codes.sh:12-18`). The accepter never needs
  to know it's a different facet.

The cut follows the standard template (`script/AddRedeemFacet.s.sol`):
`new InviteFacet()` → `diamondCut(Add, selectors)`. New
`script/AddInviteFacet.s.sol`.

### 1.2 Storage layout (`LibInviteStorage`)

Slot `keccak256("localharness.invite.storage.v1")`. One record per invite,
keyed by `keccak256(code)`:

```text
struct Invite {
    address funder;     // who escrowed the $LH; the refund recipient
    uint128 amount;     // $LH escrowed (18-dec wei). uint128 packs; $LH supply ≪ 2^128
    uint64  expiry;     // unix seconds; 0 is disallowed (every invite expires)
    Status  status;     // Open | Claimed | Reclaimed  (uint8 enum, packs with above)
}
// mapping(bytes32 => Invite) invites;   // codeHash -> record
// mapping(address => uint256) escrowedOf; // funder -> total currently escrowed (rate-limit/cap input, §2.4)
```

`funder(160) + amount(128) + expiry(64) + status(8)` packs into **two storage
slots** per invite — cheaper than RedeemFacet's two separate mappings, and the
single struct makes the state machine atomic. `Status` (not a bare `claimed`
bool) is deliberate: an invite has **three** terminal-or-active states, and a
bool can't distinguish "paid out to accepter" from "refunded to funder" — the
event log can, but on-chain reads (and double-spend guards) need the trichotomy.

`escrowedOf[funder]` is a running sum maintained on create/reclaim, so a
front-end can show "you have N `$LH` locked in pending invites" and the facet can
enforce an optional per-funder escrow cap (§2.4) without iterating.

> **Append-only rule** (mirrors `LibRedeemStorage.sol:6` comment): new fields go
> at the **end** of the struct, never reordered — diamond storage is positional.

### 1.3 Functions + events

```text
// --- create (permissionless; funder escrows their own $LH) ----------------
createInvite(bytes32 codeHash, uint256 amount, uint64 ttlSeconds)
    -> requires amount > 0, ttl in [MIN_TTL, MAX_TTL], invites[codeHash] empty
    -> transferFrom(msg.sender, address(this), amount)   // escrow (needs prior approve)
    -> store {funder: msg.sender, amount, expiry: now+ttl, status: Open}
    -> escrowedOf[msg.sender] += amount
    -> emit InviteCreated(codeHash, msg.sender, amount, expiry)

// --- accept (the recipient redeems the plaintext code) --------------------
acceptInvite(string code) -> returns (uint256 amount)
    -> h = keccak256(bytes(code))                       // same hash as redeem()
    -> load invite[h]; require status == Open; require now <= expiry
    -> status = Claimed                                 // CEI: state before transfer
    -> escrowedOf[funder] -= amount
    -> transfer(msg.sender, amount)                     // pay the accepter from escrow
    -> emit InviteAccepted(h, msg.sender, funder, amount)

// --- reclaim (funder gets the escrow back after expiry, if unclaimed) -----
reclaimInvite(bytes32 codeHash)
    -> load invite; require status == Open; require now > expiry
    -> status = Reclaimed                               // CEI
    -> escrowedOf[funder] -= amount
    -> transfer(funder, amount)                         // refund regardless of caller
    -> emit InviteReclaimed(codeHash, funder, amount)

// --- views ----------------------------------------------------------------
inviteOf(bytes32 codeHash) -> (address funder, uint256 amount, uint64 expiry, uint8 status)
isAcceptable(bytes32 codeHash) -> bool       // status==Open && now<=expiry
escrowedBalanceOf(address funder) -> uint256
```

**Events** (indexed for off-chain harvest, same discipline as
`Redeemed(user, amount, codeHash)`):
`InviteCreated(bytes32 indexed codeHash, address indexed funder, uint256 amount, uint64 expiry)`,
`InviteAccepted(bytes32 indexed codeHash, address indexed accepter, address indexed funder, uint256 amount)`,
`InviteReclaimed(bytes32 indexed codeHash, address indexed funder, uint256 amount)`.

### 1.4 The escrow flow (gas + token mechanics)

Escrow is **exactly the approve→pull pattern already shipped** in
`deposit_credits_sponsored` (`registry.rs:2360`): two calls in one sponsored
Tempo tx —

1. `approve(diamond, amount)` on the `$LH` token, then
2. `createInvite(codeHash, amount, ttl)` which does `transferFrom(funder,
   diamond, amount)`.

So `createInvite` reuses the **identical batching, sponsorship, and gas profile**
as `depositCredits` (`registry.rs:2378-2381`): approve + `transferFrom` (one
cold→warm balance pair) + the invite struct's **two cold SSTOREs** + the
`escrowedOf` SSTORE + event. Budget like `depositCredits`'s 1.5M, verified with
`cast estimate` before pinning (**CLAUDE.md gas gotcha** — never guess; cold
SSTOREs dominate; `submitFeedback`/`redeem` both under-set their first cap and
silently out-of-gassed).

`acceptInvite` and `reclaimInvite` are **one cold SSTORE flip (status) + one
`transfer` + one `escrowedOf` decrement + event** — cheaper than create. Mirror
`redeem`'s budget (its mint path is the comparable cost) and re-estimate.

All three go through `events::run_sponsored_tempo_call` (CLAUDE.md migration
status: every user-facing write already routes there), so the funder/accepter
hold **zero gas** — the embedded sponsor pays AlphaUSD fees, the user only signs
the sender hash via the apex iframe. Crucially: the user spends **`$LH`** (the
escrow) but **no gas** — which is the whole "spend `$LH` to invite" UX.

New `registry.rs` helpers mirroring the existing ones:
`create_invite_sponsored` (approve+create, like `deposit_credits_sponsored`),
`accept_invite_sponsored` (like `redeem_sponsored`, `registry.rs:2247`),
`reclaim_invite_sponsored`, and reads `invite_of` / `is_acceptable` /
`escrowed_balance_of` (like `credit_balance_of`, `registry.rs:2342`).

---

## 2. Tiers + trust

### 2.1 How the inviter picks an amount

The amount is a **free `uint256` argument to `createInvite`** — the contract
imposes no enumerated tier. Tiers are a **UI affordance**, not a chain
constraint: the create panel offers three buttons **`[10]` `[100]` `[1000]`**
plus a free-entry field. This matches the user's framing exactly ("10 or 100 is
more appropriate… 1000 to people I trust") without baking trust levels into the
contract, where they'd be rigid and need a cut to change.

> **Why tiers are off-chain.** RedeemFacet already proves the pattern: tiers are
> just *different `amountWei` batches* (`add-redeem-codes.sh <amount_lh>
> <count>`), the contract takes a plain `uint256`. The "tier" is a label on a
> denomination, not a type. Same here — the chain sees an amount, the UI
> presents the choice.

### 2.2 Why graded amounts (the trust model)

The invite amount **is** the trust signal. The funder is spending *their own*
`$LH`, so over-funding is self-punishing — a natural economic governor that
RedeemFacet's owner-minted codes lack (the owner mints from thin air, so there's
no cost discipline; an invite costs the funder real balance). Giving a stranger a
1000-`$LH` invite means locking 1000 of your `$LH` on a bet they'll be a good
actor; giving 10 is a cheap "try the platform" gesture. The mechanism makes the
user's intuition ("trusted people get more") **the funder's own cost-benefit
call**, not a policy the platform enforces.

### 2.3 Funding source

`createInvite` pulls from the **funder's own `$LH` balance** (their EOA, or
whichever address the front-end signs as — for an agent, its TBA). It does **not**
mint. So the total `$LH` an inviter can give away is bounded by what they hold,
which is bounded by redeem codes + `send_lh` + (future) Stripe on-ramp. This is
the key safety property: **invites can never inflate supply** — they only
redistribute it. A sybil farm can't invite-mint infinite credits the way the
disabled daily-allowance allowed (CLAUDE.md: `dailyAllowance` set to 0 precisely
because free-account × free-mint = infinite credits).

### 2.4 Limits — cap per-invite? rate-limit? prevent draining?

The user worried about "draining." Three layered options, ranked by how much they
distort the simple model:

1. **`MIN_TTL` / `MAX_TTL` bounds on expiry (REQUIRED, cheap).** An invite with
   no expiry, or a 100-year expiry, locks `$LH` forever and defeats the refund
   loop; a 1-second expiry is a griefing trap. Bound `ttlSeconds` to e.g.
   `[1 hour, 90 days]`. This is the one limit the contract *must* enforce —
   everything else is optional.

2. **Per-funder escrow visibility (`escrowedOf`, REQUIRED for UX, no cap by
   default).** The running sum lets the UI show "you have N `$LH` locked." A
   **hard cap** (`maxEscrowPerFunder`, owner-tunable, **default unlimited /
   `type(uint256).max`**) can be flipped on later if abuse appears. Default off
   because a funder draining *their own* balance into escrow hurts only
   themselves — the refund loop returns it. "Draining" of *someone else's* funds
   is impossible (you can only escrow what you hold).

3. **Per-invite cap (OPTIONAL, default off).** A `maxInviteAmount`
   owner-knob exists as a safety valve but defaults to unlimited. The economic
   self-punishment of over-funding (§2.2) makes a hard cap mostly unnecessary on
   testnet where `$LH` is credit; revisit at mainnet (§7 open question).

**The honest read:** the only *necessary* limit is the TTL bound. "Draining"
fears mostly dissolve once you see the funder can only escrow their own balance
and always gets it back on expiry. The owner-knobs (2,3) are seams left in the
storage/setters so we *can* clamp without a re-cut, not switches we turn on at
launch.

---

## 3. Expiry + refund lifecycle

### 3.1 The state machine

```
                       createInvite (escrow $LH)
                                 │
                                 ▼
                            ┌─────────┐
                  accept    │  Open   │   now > expiry
              ┌─────────────┤(escrowed├─────────────┐
              │             └─────────┘              │
              ▼                                       ▼
        ┌───────────┐                          (reclaimable)
        │  Claimed  │                                │
        │ (paid     │                      reclaimInvite (refund)
        │  accepter)│                                │
        └───────────┘                                ▼
                                              ┌─────────────┐
                                              │  Reclaimed  │
                                              │ (refunded   │
                                              │  funder)    │
                                              └─────────────┘
```

Both `Claimed` and `Reclaimed` are **terminal**. The transition guards are the
double-spend defense (§4.1): `acceptInvite` requires `status==Open &&
now<=expiry`; `reclaimInvite` requires `status==Open && now>expiry`. These two
windows are **disjoint** (the `now<=expiry` vs `now>expiry` split), so a given
invite can be accepted XOR reclaimed, never both, with no overlap even at the
exact expiry second.

### 3.2 Who triggers the refund

**Decision: `reclaimInvite` is permissionless to *call*, but always pays the
*funder*.** Anyone can poke the reclaim (the `$LH` only ever goes to
`invite.funder`, so a third-party caller gains nothing), but in practice the
**funder's own front-end triggers it** from their "pending invites" list once an
invite shows expired. Rationale:

- **No auto-refund.** Solidity has no cron; "auto-refund on expiry" would require
  either a keeper (off-chain infra we reject — CLAUDE.md no-off-chain-infra) or
  refunding lazily inside some other call (couples unrelated txs). Explicit
  `reclaimInvite` is the honest model: the escrow sits in the diamond until
  someone reclaims it, and the funder is motivated to (it's their money).
- **Permissionless caller** means a good-citizen indexer or even the accepter's
  failed-claim path *could* sweep expired invites back to funders, but nobody is
  *obligated* to — the funder bears the responsibility for their own refund,
  which is correct (they chose the TTL).
- The browser sweeps this for the user: on studio/admin open, the "pending
  invites" panel lists the funder's invites with state; expired-unclaimed ones
  show a **`[reclaim]`** button (one sponsored tx). Optionally, a single
  **`[reclaim all expired]`** batches several `reclaimInvite` calls in one Tempo
  tx (same multi-call batching as publish-app's two `setMetadata` calls).

### 3.3 Refund accounting (exact)

On `reclaimInvite(codeHash)` for an `Open`, expired invite:
`transfer(invite.funder, invite.amount)` moves **exactly the escrowed amount**
back, `escrowedOf[funder] -= amount`, `status = Reclaimed`. No fee is skimmed —
the funder gets 100% back (the sponsor ate the gas, in AlphaUSD, both on create
and reclaim). The `$LH` round-trips funder→diamond→funder with zero `$LH`
leakage. (Whether the *platform* should skim a tiny creation fee is an open
question, §7 — default **no fee**.)

---

## 4. Security

### 4.1 Double-claim / replay

- **One-shot via status flip before transfer (CEI).** `acceptInvite` sets
  `status = Claimed` **before** the `transfer` — identical to
  `RedeemFacet.redeem` setting `s.claimed[h] = true` before the mint
  (`RedeemFacet.sol:77`, with its "burned before the mint (CEI)" comment). A
  re-entrant or replayed accept re-reads `status != Open` and reverts
  (`CodeAlreadyUsed`-equivalent). Same guard kills double-reclaim and
  accept-then-reclaim.
- **Replay across invites is impossible** — the code hash *is* the key; a
  different code is a different record.

### 4.2 Front-running the code (the real threat)

This is the one materially new risk vs RedeemFacet, and it must be stated
plainly: **the plaintext code is a bearer secret.** `acceptInvite(string code)`
hashes the code on-chain, so whoever submits a valid plaintext first gets the
`$LH`. An attacker watching the mempool who sees an `acceptInvite("lh-…")` tx can
**copy the plaintext from the calldata and front-run it** with higher priority,
stealing the escrow. RedeemFacet has the identical exposure today (its codes are
bearer secrets too) — but invites make it sharper because *the value is funded by
a user who'll notice the theft*, and invites are shared as **URLs** (`?invite=`),
which are more leak-prone than DM'd redeem codes.

Mitigations, ranked:

1. **Short expiry + treat the code as a secret (MVP, matches redeem).** The code
   is meant for *one* recipient over a *trusted* channel (the user's whole
   framing is "people I trust"). Short TTLs shrink the theft window. This is the
   status-quo redeem security model and is **acceptable for MVP** — it's how the
   live `?invite=` path already works.

2. **Bind to a recipient address (RECOMMENDED for the funded-trust case).** Add
   an optional `address recipient` to `createInvite`; if non-zero, `acceptInvite`
   requires `msg.sender == recipient`. Now a front-runner who copies the
   plaintext **can't** accept — the escrow only pays the named address. The funder
   gets the recipient's address out-of-band (they're inviting someone they
   *trust*, so they likely know their `<name>.localharness.xyz` → owner address,
   resolvable via `registry::owner_of_name`). This converts a bearer code into a
   **bound voucher** and defeats mempool front-running entirely. Cost: the funder
   must know the recipient's address up front, which is fine for "people I trust"
   and wrong for "post a public invite link." So: **support both** — zero
   recipient = open bearer code (link-shareable, short TTL); non-zero = bound
   voucher (front-run-proof, any TTL).

3. **Commit–reveal (REJECTED for v1).** Full front-run immunity for *bearer*
   codes needs a two-tx commit then reveal, doubling tx count and UX friction for
   a testnet-credit threat. Note it as a mainnet-value option (§7), don't build
   it now.

### 4.3 Reentrancy on escrow / refund

`$LH` is the `LocalharnessCredits` TIP-20 token (CLAUDE.md). A malicious *token*
can't exist (it's our own fixed token), but discipline holds regardless:
**Checks-Effects-Interactions everywhere** — every state mutation (`status`,
`escrowedOf`) lands **before** the `transfer`/`transferFrom`. So even if `$LH`
ever gained a transfer hook, no reentrant call finds an exploitable mid-state.
`createInvite`'s `transferFrom` is the inbound pull (no payout to reenter);
accept/reclaim are CEI as specified. No external calls other than the token
transfer; no need for a reentrancy guard, but the CEI ordering is the contract.

### 4.4 Griefing (create + reclaim spam)

- **Self-funded ⇒ self-limited.** Spamming `createInvite` costs the spammer their
  own `$LH` (escrowed) and the sponsor's gas. The escrow lock is the spam filter
  ("gas is the spam filter" → here, *your own escrowed credit* is the filter).
- **The sponsor is the real chokepoint** for gas-based spam, exactly as today
  (CLAUDE.md §4.3 of economy-reputation: the relay is the rate-limit chokepoint).
  The embedded sponsor pays AlphaUSD; a flood of invite txs drains the sponsor,
  not the chain. Mitigation is the **same relay rate-limit/balance-circuit-breaker
  already planned**, plus the optional `escrowedOf` cap (§2.4) to bound a single
  funder's footprint. No invite-specific mechanism needed.
- **Dust invites** (1-wei `$LH`, max TTL, never claimed) just lock the funder's
  own dust; harmless. `amount > 0` is the only floor (revisit a `MIN_AMOUNT` if
  dust-storms ever matter).

### 4.5 Interaction with disabled daily allowance + live x402/redeem

- **Daily allowance (DISABLED).** Invites are **supply-neutral** (redistribute,
  never mint), so they do **not** reintroduce the infinite-credit sybil hole that
  got `dailyAllowance` zeroed (CLAUDE.md CreditsFacet). A sybil ring passing
  invites among themselves is just their own `$LH` round-tripping minus sponsor
  gas — they net **nothing** (same logic as the §4.2 attestation gate in
  economy-reputation: round-tripping your own value among sybils gains nothing).
- **RedeemFacet (live).** Coexists (§6). An invite can *only* be funded from
  already-existing `$LH`; redeem codes are how that `$LH` first enters supply.
  Orthogonal: redeem = mint-in, invite = move-around.
- **x402 / SessionFacet / CreditMeter (live).** Accepting an invite credits the
  recipient's `$LH`, which then funds the **same** session/meter/x402 gates
  unchanged. The invite is a *funding on-ramp for a newcomer*; it touches none of
  the spending paths. (Note: `sessionPrice` is now 1 `$LH`/hr — CLAUDE.md — so an
  invited newcomer needs the invite `$LH` to actually chat, which is precisely the
  point: the invite is what bootstraps a brand-new user past the credit gate.)

---

## 5. UX — the `?invite=` flow

### 5.1 What exists today (and what changes)

The `?invite=CODE` plumbing is **already built** and currently points at
RedeemFacet:

- `mod.rs::capture_invite_code` (`mod.rs:1031`) reads `?invite=CODE`, stashes it
  in `localStorage` as `lh_pending_invite`, strips it from the URL.
- `events::try_redeem_pending_invite` (`events.rs:1766`) auto-**redeems** it via
  `redeem_sponsored` once a credit identity exists — fired from every paint path
  (`mod.rs:521/554/838`), idempotent (clears the pending code *before* the call
  so a refresh can't double-spend, `events.rs:1782-1786`).

**The change is small and mostly server-side-of-the-wasm:** the auto-accept path
calls `accept_invite_sponsored` (InviteFacet) instead of, or in addition to,
`redeem_sponsored`. Because both hash `keccak256(bytes(code))` and both are
one-shot bearer codes, the recipient experience is **byte-identical** — they
click a link, land, and get credited. **Disambiguation** (is this code a redeem
code or an invite?): cheapest is a **code prefix** convention — redeem codes are
`lh-<amount>-…` (`add-redeem-codes.sh:113`); mint invite codes as
`inv-<amount>-…` and route on prefix. Alternatively, **try-accept-then-fallback**:
attempt `acceptInvite`, and if the code isn't a known invite, fall back to
`redeem`. Prefix-routing is simpler and avoids a wasted on-chain read; **lean
prefix**.

### 5.2 Create an invite (the new UI)

A new **`[invites]`** panel in the studio/admin chrome (the same place the
public-face picker and `send_lh` live). All HTML via `maud` templates +
fragment swaps — **no imperative DOM, no JS alert/confirm** (memory:
`feedback_ui_no_dom`, `feedback_no_js_alerts`):

```
┌─ invites ─────────────────────────────────────────────┐
│  amount   [ 10 ] [ 100 ] [ 1000 ]   or  [____] $LH     │
│  expires  [ 24h ▾ ]   (1h … 90d)                       │
│  for      [ anyone (link) ▾ | a specific name ]        │  ← §4.2 bound voucher
│                                   [ create invite ]    │
├────────────────────────────────────────────────────────┤
│  your pending invites                                   │
│   inv-100-A8kZ…   100 $LH   expires 23h   [copy link]   │
│   inv-10-Qm2p…     10 $LH   EXPIRED        [reclaim]    │
│   inv-1000-… (claimed by alice)  1000 $LH   ✓           │
│                              [reclaim all expired]      │
└────────────────────────────────────────────────────────┘
```

Create flow (one `Action::CreateInvite` → an `events::run_create_invite`):
1. Generate a CSPRNG code **client-side** (`inv-<amount>-<10 url-safe chars>`,
   same shape/entropy as `add-redeem-codes.sh:101-104` but in-wasm via
   `getrandom`), compute `keccak256(bytes(code))`.
2. `create_invite_sponsored(signer, fee_payer, codeHash, amount_wei, ttl,
   recipient?)` — approve + create in one sponsored Tempo tx (§1.4).
3. On success, build the link `https://<some>.localharness.xyz/?invite=<code>`
   and show it with a **`[copy link]`** action. **The plaintext lives only in the
   user's browser/clipboard** — never on-chain, never in OPFS unless we choose to
   persist a local "my invites" record (see §7: how to remember plaintext codes
   for the pending-list; on-chain we only have the hash, so the UI either
   re-derives links from a local stash or shows hash-only entries with no
   re-copyable link after the tab closes).

The **escrow happens at create** — the moment the user clicks `[create invite]`,
the `$LH` leaves their balance (visible: the credits pill drops by the amount,
`escrowedBalanceOf` rises). That *is* "spend `$LH` to invite."

### 5.3 The recipient auto-onboards

Unchanged from today's flow (§5.1): recipient opens `…/?invite=<code>`, the code
is captured, and once they have an identity the auto-accept fires and credits
them. The only nuance for **bound vouchers** (§4.2): if the invite names a
recipient address, the accepter's identity must match — so the link should land
them on a flow that ensures they're signing as the named identity (for a
`<name>`-bound voucher, they accept from that subdomain where their owner address
is the signer). For open bearer invites, any newly-created identity accepts.

### 5.4 The inviter sees pending / claimed / refundable

The pending-invites list (§5.2) reads `inviteOf` per code the user created.
Because on-chain we only store the **hash**, the front-end needs the **plaintext
(or at least the codeHash + amount + a local label)** to render rows — see §7
open question on local persistence. State per row from `inviteOf`:
`Open & !expired` → `[copy link]`; `Open & expired` → `[reclaim]`; `Claimed` →
greyed ✓ (with accepter from the `InviteAccepted` event if we index it);
`Reclaimed` → greyed "refunded."

---

## 6. Relationship to existing pieces — when to use which

| Mechanism | Who | Source of `$LH` | Expiry/Refund | Recipient | Use when |
|---|---|---|---|---|---|
| **RedeemFacet** (live) | **owner only** | **minted** (`ISSUER_ROLE`) | none (one-shot) | bearer code | Platform bootstraps supply; you (owner) seed trusted users / fleets. |
| **`send_lh`** (live) | any holder | sender's balance | none (instant, irreversible) | a known address/name | You already know who, want them funded **now**, no onboarding link. |
| **InviteFacet** (this doc) | **any holder** | **funder's balance (escrowed)** | **yes — refundable on expiry** | bearer link or bound voucher | You want to **onboard a newcomer** with a shareable link, spend *your* `$LH`, and get it **back if they never show**. |

The three are complementary, not redundant:

- **Redeem vs invite:** redeem **creates** `$LH` (owner privilege, supply
  control); invite **moves** existing `$LH` (anyone, supply-neutral). Invite is
  the *democratization* of the "hand someone a code that funds them" UX —
  RedeemFacet's UX for everyone, but spending your own balance instead of the
  platform's mint.
- **`send_lh` vs invite:** `send_lh` is a **fire-and-forget transfer** to a known
  party — no expiry, no claw-back, no onboarding link, settles instantly. Invite
  adds the **escrow + expiry + refund + shareable-link** machinery for the
  *uncertain* case ("I'll give this person `$LH` *if* they join"). If you already
  know the recipient and trust them, `send_lh` is simpler; if you're sending a
  link into the unknown and want your `$LH` back if it's ignored, invite.
- **Coexistence in `?invite=`:** both redeem and invite codes ride the **same
  `?invite=CODE` link UX** (§5.1), routed by code prefix. The owner keeps using
  `add-redeem-codes.sh` for minted-credit campaigns; users use the new invites
  panel for peer-to-peer onboarding. Both land a newcomer with `$LH`.

---

## 7. Phased plan + open questions + triage

### 7.1 Phasing

**MVP (escrow + accept + reclaim, bearer codes):**
- `InviteFacet` + `LibInviteStorage` + `script/AddInviteFacet.s.sol` (cut it; I
  run the cut — memory: I do all deploys/cuts, key in `./.env`).
- `createInvite` / `acceptInvite` / `reclaimInvite` + views, CEI throughout,
  TTL-bounded, `escrowedOf` sum.
- `registry.rs` helpers (`create_invite_sponsored`, `accept_invite_sponsored`,
  `reclaim_invite_sponsored`, `invite_of`, `escrowed_balance_of`) mirroring the
  redeem/deposit helpers; gas via `cast estimate`.
- Route `?invite=` auto-accept to InviteFacet by code prefix (`inv-`), keeping
  redeem (`lh-`) working — a ~10-line change in `try_redeem_pending_invite`.
- Studio **invites panel**: create (tier buttons + custom + TTL), copy-link,
  pending list with `[reclaim]`. All maud + swaps.
- A `scripts/` smoke test (E2E via the CLI + my on-chain identity, memory:
  be-the-e2e-tester): create → accept from a second identity → assert balance
  moved; create → wait past a short TTL → reclaim → assert refund.

**Phase 2 (front-run-proof + ergonomics):**
- **Bound vouchers** (optional `recipient` in `createInvite`, §4.2) — the real
  trust case, defeats mempool theft.
- `[reclaim all expired]` batch tx.
- Local "my invites" persistence so links survive a tab close (§7.3).
- Off-chain harvest of `InviteAccepted` to show *who* claimed (like
  `harvest-feedback`).

**Phase 3 / mainnet-gated:**
- Optional creation fee / `maxInviteAmount` / `maxEscrowPerFunder` knobs flipped
  on if abuse appears at value.
- Commit–reveal for bearer codes if front-running bites with real value (§4.2).

### 7.2 Trade-offs to be honest about

- **On-chain escrow gas vs an owner-subsidized model.** Every invite is ~3 txs
  over its life (create, accept, reclaim) the sponsor pays gas for. An
  alternative is **owner-subsidized invites** (the platform mints into a holding
  pool and the "invite" is just a redeem code the owner pre-funds) — cheaper per
  user, but it's *not* the user's `$LH`, breaks the "spend your own to invite +
  get it back" intent, and reintroduces a mint path. **The escrow model is the
  honest realization of the user's ask; accept its gas cost** (sponsor-paid,
  testnet-cheap). Named so it's a decision, not a surprise.
- **Bearer link vs bound voucher.** Bearer is shareable-anywhere but
  front-runnable; bound is theft-proof but needs the recipient's address up
  front. Supporting both (zero/non-zero `recipient`) is the right call but adds a
  branch to accept; MVP can ship bearer-only and add bound in Phase 2.
- **Storing plaintext locally.** On-chain has only the hash, so a closed tab
  loses the copy-able link. Persisting plaintext in OPFS is convenient but is a
  (low-value, self-owned) secret at rest — gate behind the planned at-rest
  encryption, or accept "copy the link now, we don't keep it."

### 7.3 Open questions the user should decide

1. **Bearer vs bound by default.** Ship MVP bearer-only (matches today's
   `?invite=`), or include bound vouchers from day one given the front-run risk
   is sharper for funded invites? *(Recommendation: MVP bearer + short TTL, bound
   in Phase 2 — but happy to pull bound into MVP if front-running worries you.)*
2. **Creation fee?** Skim a tiny platform fee on `createInvite` (revenue / spam
   tax) or 100%-refundable with zero leakage? *(Recommendation: no fee on
   testnet; revisit at mainnet value.)*
3. **Per-funder escrow cap default.** Unlimited (self-limiting) or a default cap
   (e.g. 10k `$LH` locked) as a circuit-breaker? *(Recommendation: unlimited;
   the knob exists to flip later.)*
4. **Code disambiguation.** Prefix-routing (`inv-` vs `lh-`) or
   try-accept-then-fallback-to-redeem? *(Recommendation: prefix — no wasted RPC.)*
5. **Local plaintext persistence** for the pending-invites links (§7.2) — keep in
   OPFS (convenience, secret-at-rest) or copy-now-discard?
6. **Does the recipient need a name first?** Auto-accept currently credits a
   freshly-generated identity. For bound vouchers the accepter must *be* the named
   address — confirm the onboarding lands them signing as the right identity.
7. **TTL bounds.** Confirm `[MIN_TTL, MAX_TTL]` (proposed 1h … 90d).

---

## 8. Summary of the recommended approach

A new permissionless **`InviteFacet`** (storage
`keccak256("localharness.invite.storage.v1")`) where any holder **escrows their
own `$LH`** (`approve` + `transferFrom` funder→diamond, the exact pattern
`deposit_credits_sponsored` already ships) to back an invite keyed by
`keccak256(code)` — the same bearer-code hashing as RedeemFacet, so the existing
`?invite=CODE` auto-onboard path carries over with a one-line prefix-route change.
The amount is a free `uint256` (tiers are UI buttons 10/100/1000 + custom, not a
chain constraint), every invite carries a bounded TTL, and `acceptInvite` pays the
escrow to the recipient while `reclaimInvite` refunds the funder 100% after expiry
— all CEI, all one-shot via a 3-state status flip, all sponsor-paid (user holds
zero gas, spends only the escrowed `$LH`). It is **supply-neutral** (redistributes,
never mints) so it does not reopen the sybil hole that disabled the daily
allowance, and it slots beside RedeemFacet (owner-mint) and `send_lh`
(instant-irreversible-transfer) as the **escrowed, refundable, link-shareable**
onboarding primitive.

**Top open questions for the user:** (1) bearer link vs front-run-proof bound
voucher as the MVP default; (2) zero creation fee vs a small skim; (3) prefix-route
or fallback for redeem-vs-invite code disambiguation; (4) whether to persist
plaintext codes locally so invite links survive a tab close.
