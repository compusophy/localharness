# MAIN identity + multi-device linking

> **STATUS: SHIPPED** — `MainIdentityFacet` (auto-MAIN on first claim) and the
> multi-signer ERC-6551 `MultiSignerAccount` are live on the diamond. Recommended
> Model 1 (TBA-of-TBA) is what shipped. NOTE: device linking is now QR
> **seed-adoption** (`?adopt=1#s=<ciphertext>`), not the on-chain add-signer flow
> sketched in "concrete next steps" — the seed IS the identity. Sybil-resistance
> mechanisms (cost-to-be-MAIN, reputation-bound MAIN) remain a later, mainnet-gated
> layer. Kept for the Key/Account/Identity layering + sybil analysis.

Updated 2026-05-25 with the multi-device + account-linking thread.
The original sybil-resistance framing is preserved at the bottom.

## The user's framing

> "I have different wallets and addresses and subdomains on my phone and
> on my computer. … I have like 40 different wallets, tons of fractured
> dust. That's why having a MAIN single identity or address will help
> against sybil and simplify."

Two goals, one design:
1. **Consolidation.** One identity that *is* the user. Assets aggregate
   under it. No "which wallet has the gas?" tax on every action.
2. **Sybil resistance.** The economy can address one user once, not
   N times as N puppet wallets.

These pull in the same direction: a single primary thing per person
solves both. The hard part is making "single" work across devices
without making the user a custodian of their own footgun.

## The three layers

Tease them apart before designing the merge:

| Layer | What it is | Plural OK? |
|------:|------------|------------|
| **Key** | secp256k1 keypair (an EOA) | Yes — one per device is healthy |
| **Account** | Where assets live (EOA itself, or a smart contract) | **No** — this is what we want unified |
| **Identity** | The thing other agents address (name, reputation handle) | **No** — that's the MAIN |

The current architecture conflates Key and Account: each device's
seed-phrase IS the account, because all assets sit at that EOA's
address. To unify Account across devices, the Account has to live
*somewhere other than any single device's EOA*. That's the whole
job.

## The candidate models

### Model 0 — what we have today (seed-import linking)

- One EOA per device by default.
- "Link" = export seed from device A, import on device B. Now both
  devices share that EOA. All assets at that EOA's address are
  visible to both devices.
- MAIN concept = the first subdomain claimed becomes the user's
  primary; subsequent subdomains are alts of the SAME EOA.
- Cross-device split = each device has its own EOA, owns its own
  separate set of subdomains. No relationship between them on-chain.

Pros: trivially implemented, already shipped. Self-sovereign.
Cons: seed export/import is the only join — clunky, dangerous,
asymmetric. No way to add/remove a device without rotating the
master key. No hierarchy.

### Model 1 — TBA-of-TBA (use what we already cut into the diamond)

The MAIN subdomain has a TBA (we already have ERC-6551 cut).
The MAIN's TBA owns the user's alt NFTs and holds the consolidated
balance. Device EOAs authorize the TBA via ERC-6551's
`isValidSignature`. Adding a device = the user (from any existing
authorized device) signs an attestation that the new device's EOA
can act for the MAIN's TBA.

Pros: zero new contracts beyond a richer ERC-6551 account impl.
Stays on-chain, stays self-sovereign. The MAIN NFT *literally*
represents the user.
Cons: the standard ERC-6551 account is single-owner via the NFT's
holder. Multi-device requires a custom account impl. Doable but
non-trivial — the account would need to maintain its own
authorized-signer registry.

### Model 2 — EIP-4337 smart-account at MAIN

The user's MAIN is held by a 4337 smart contract wallet. Device EOAs
are signers on the wallet. The MAIN NFT (and all alts, and the $LH
balance) lives at the wallet's address.

Pros: 4337 is the standard story for this exact problem. Paymaster
sponsorship is built in — Tempo has one, so device EOAs don't need
gas. UserOperation flow generalizes to other actions later.
Cons: needs a 4337 bundler on Tempo (or we run our own). Larger
surface; more contract code. More moving parts to audit.

### Model 3 — EIP-7702 (set-EOA-code)

User's EOA temporarily delegates code to a smart-account
implementation. Same multi-signer model as 4337, but applied to an
existing EOA in place — no new address.

Pros: no new "wallet address" to onboard. Keeps the EOA-as-identity
model intact. Pectra is mainnet now; Tempo support unclear.
Cons: depends on Tempo support. The "set code" auth is per-EOA per
chain — still requires the user to hold the original key.

### Model 4 — Off-chain attestations (ERC-8004 style)

Each device EOA stays independent. Cross-device linkage is a public
attestation: "EOA A and EOA B are controlled by the same person",
signed by both. Apps choose how much to trust the link.

Pros: zero protocol changes. Already compatible with everything.
Cons: doesn't solve consolidation — the user still has N wallets
with N separate balances. Solves identity, not assets.

## Recommendation

**Ship Model 1 (TBA-of-TBA) for v1, prepare for Model 2 (4337) as the
upgrade path.**

Why:
- We already have ERC-6551 cut into the diamond. The account impl is
  ours to evolve.
- Writing a multi-signer ERC-6551 account is straightforward Solidity
  — a `mapping(address => bool) authorizedSigners` + a custom
  `isValidSignature` that accepts any authorized signer.
- The user's mental model ("my MAIN owns my stuff") maps directly to
  "the MAIN NFT's TBA owns my stuff".
- Adding a device = one tx from an existing-authorized device that
  inserts the new device's EOA into the signer set. Removing = the
  inverse.
- Paymaster sponsorship: Tempo's paymaster can sponsor any tx, not
  just 4337 UserOps. Calls that come from the MAIN's TBA (via
  ERC-6551 `execute`) can be paymaster-sponsored at the entry-point
  level (the device EOA paying gas for the `execute` call, OR the
  paymaster paying for the device EOA's tx).

When we eventually want richer policy (session keys, spending limits,
recovery social-guardians, etc.), migrate the same MAIN NFT's
account-impl pointer to a 4337 contract. ERC-6551 lets the account
impl change without changing the TBA address — graceful upgrade.

## The hierarchy question

> "If we were to truly link accounts, it would be the same as kind of
> sending ownership right? But then how would my phone be able to
> control my main? I should be able to see my main then on my phone."

The honest answer: **once two keys can both sign for the same identity,
they're equivalent.** There is no "primary device" anymore. That's the
right answer for security — the moment hierarchy exists, every
discussion becomes "which device is more authoritative" and
recovery becomes a nightmare. Flat multi-signer is the security
property you actually want.

If the user wants a "this is my main device" UI cue, that lives in
the bundle, not in the protocol. The protocol sees a set of equal
signers; the bundle on a given device can mark itself as "this
device" with a local flag in OPFS. Cosmetic.

For the multi-MAIN-across-devices anxiety: with Model 1 in place,
that goes away. You don't have multi-MAIN. You have ONE MAIN whose
TBA you can sign for from multiple devices. The 40-wallets-of-dust
problem evaporates because all $LH lives at the MAIN's TBA, not
scattered across N device EOAs.

## The "MAIN per person" problem (sybil)

Adding devices is a JOIN operation. Splitting a MAIN is a FORK
operation that the protocol doesn't support — you can't "remove
yourself" from a MAIN that has multiple signers and end up with a
new MAIN of your own. That's by design: if forking were cheap, sybil
attacks would be cheap too.

A user who wants more identities just registers more subdomains.
Those are alts under the same MAIN. The protocol treats them as
expressions of the same person. Reputation accrues to the MAIN, not
the alt.

For the "but anyone can claim 100 MAINs by burning 100 separate
seeds" problem — that's where the economic + reputation layers
(below) come in.

## Sybil resistance (preserved from earlier discussion)

Mechanisms (none implemented yet; ranked by recommended ship order):

### A. Cost-to-be-MAIN

Charge a meaningful one-time fee in $LH to register a MAIN.
Funds locked in the MainIdentityFacet, slashed on misbehavior,
returned on graceful exit. Linear cost per identity is the classic
sybil deterrent.

Knob: cost scales with reputation. Cheap at zero rep, expensive at
high rep, so a sybil farm pays N×(low) to spin up but N×(high) to
grow them all in parallel — keeps the cost asymmetric in defenders'
favor.

### B. Reputation-bound MAIN

Subdomain doesn't get the MAIN flag until it accumulates X
reputation through ERC-8004 attestations from OTHER MAINs. Bootstrap
deadlock: where does the first non-zero reputation come from?
Pairs cleanly with (A) — pay cost to bootstrap, then reputation
keeps you (or doesn't).

### C. Social-graph anchoring

N existing MAINs vouch for a new one with skin in the game (their
own reputation). Rumour-network effects raise sybil farm seeding
cost.

### D. Continuous proof-of-personhood

WorldID, Idena, BrightID — meaningful but violates the
self-sovereign / Rust-native ethos. Flag, don't ship.

### E. Accept parallel MAINs explicitly

No enforcement; let downstream consumers decide. Lowest friction
but doesn't solve the user's stated worry. Falls out of the design
as the no-op baseline.

## What landed in this commit (2026-05-25 / v0.10.24)

- ✅ **`MainIdentityFacet.sol`** cut into the diamond. Surface:
  `registerMain(uint256) / clearMain() / mainOf(address) /
  mainNameOf(address) / isMain(uint256)`. NO fee/lock yet — sybil
  resistance is a later layer; this just establishes the primitive.
- ✅ **`registry::main_of`** / **`registry::register_main`** /
  **`registry::claim_and_maybe_set_main`** added to the bundle.
- ✅ **Auto-MAIN on first claim**: `run_apex_claim` and the signer
  iframe's `run_claim_name` both use the convenience helper so the
  user's very first subdomain becomes their MAIN automatically. No
  user action required.
- ✅ **MAIN badge in agents list**: `agents_list` template now takes
  a `main_token_id` and renders a small `[main]` chip on the row
  matching the holder's registered MAIN.
- ⏳ Custom multi-signer ERC-6551 account impl + add-device flow
  + paymaster integration: next commits. (Both later SHIPPED:
  `MultiSignerAccount` is live, and sponsorship is Tempo native AA —
  the old `paymaster.md` analysis was superseded and removed; see
  `../../CLAUDE.md` *Tempo Transactions + sponsorship*.)

## Concrete next steps

1. **Write `MultiSignerAccount.sol`** — a custom ERC-6551 account
   with `addSigner / removeSigner / isAuthorizedSigner`. Replaces
   the vanilla account impl currently configured on the diamond.
   Switching the impl will change the deterministic TBA address for
   future mints; existing TBAs would need to be migrated explicitly
   (testnet: acceptable; mainnet: design an address-stable proxy).
2. **`Sybil`-resistant MAIN** — add a `lockedFor` field to MAIN
   storage and start charging $LH on `registerMain`. Knob lives on
   the facet, owner-tunable.
2. **Custom ERC-6551 account impl** with `authorizedSigners` map and
   multi-signer `isValidSignature`. Replaces the vanilla
   `ERC6551Account` currently configured on the diamond. Migration:
   call `setTbaConfig` to point at the new impl. Existing TBAs
   continue to work because the address is derived from
   `(impl, salt, chain, registry, tokenId)` and we'd be changing
   impl — actually old TBAs would resolve to a different address.
   For testnet that's fine; for mainnet we'd want a registry-level
   upgrade path that preserves addresses (or accept the migration as
   a one-time hop).
3. **Bundle UX**:
   - "Add this device" flow at apex admin → security → add signer.
     Shows a QR code with a sign-challenge URL; the other device
     opens it, signs, submits. Other-device's EOA gets added.
   - "MAIN" badge on the user's primary subdomain in the apex
     agents list.
   - Paymaster integration so device EOAs don't need TMP balance —
     all txs sponsored by the MAIN's TBA's $LH allowance.
4. **Harvest scripts** for reputation/feedback events so we can
   actually see what users say (FeedbackFacet shipped in this
   commit; reputation facet is later).

## Open questions

- **Tempo paymaster integration.** Need to find the paymaster
  contract address and the sponsorship-policy interface. Mentioned
  by the user, undocumented in our codebase.
- **Address-stable ERC-6551 impl upgrade.** ERC-6551 derives TBA
  address from impl + salt + chain + registry + tokenId. Changing
  impl changes address. Options: (a) accept migration on testnet,
  (b) deploy a thin proxy as the registered impl + upgrade the
  proxy implementation behind it.
- **Recovery.** If a user loses all their devices, the MAIN is
  unreachable. Social recovery (M-of-N guardians) is the standard
  answer; needs design.
- **Alt → MAIN promotion.** What happens if a user has an alt that
  in retrospect should have been the MAIN? Can a MAIN be transferred
  to a different NFT in the same owner's set?

## Where this lives in the code

- `contracts/src/facets/MainIdentityFacet.sol` — new facet.
- `contracts/src/erc6551/MultiSignerAccount.sol` — new account impl
  with authorized-signer registry.
- `src/registry.rs` — `register_main`, `add_signer`,
  `remove_signer`, `is_authorized_signer` helpers.
- `src/app/templates.rs` — MAIN badge in agents list; "add device"
  flow inside admin → security.
- `src/app/events.rs` — actions for the new device-add flow.
