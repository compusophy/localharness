# MAIN identity — design problem

Open question raised in conversation 2026-05-25.

## The problem

The 0.10.20 inversion makes the first subdomain a user claims their
"primary identity" — every subsequent subdomain they own derives from
the same apex wallet. Good for UX (no apex pre-step), bad for sybil
resistance: nothing stops a single person (or one well-funded
adversary) from running N parallel apex wallets, each with its own
"MAIN" subdomain, each accruing reputation independently.

User framing: *"we need a way to protect against sybil by incentivizing
a main, but someone could have multiple mains idk what to do, they can
always parallelize the attack."*

## What "MAIN" needs to be

A MAIN identity is the on-chain anchor that:
1. **Other agents can address by reputation.** "X has a 4.7 score
   across 200 interactions" is only meaningful if X is hard to
   duplicate.
2. **Carries real cost to acquire and maintain.** Free mints + free
   reputation = sybil heaven.
3. **Has a clear "this is the one true MAIN of this person" claim.**
   A person can own multiple agents (subdomains) but only one MAIN.

## Candidate mechanisms

### A. Cost-to-be-MAIN

Charge a meaningful one-time fee in $LH (or TMP) to register a MAIN.
Funds locked, slashed on misbehavior, returned on graceful exit.

- Pros: linear-cost-per-identity, classic sybil deterrent.
- Cons: capital-rich attackers laugh at it. Sets a floor on adoption
  (poor users can't get a MAIN).
- Adjustable knob: cost scales with reputation — cheap to MAIN at
  zero rep, expensive at high rep, so a sybil farm pays N×(low) to
  spin up but N×(high) to grow them all in parallel.

### B. Reputation-bound MAIN

A subdomain doesn't become MAIN until it accumulates X reputation
points (ERC-8004 facet, deferred). Reputation comes from interactions
with OTHER MAINs. Bootstrap problem: where does the first non-zero
reputation come from?

- Pros: organic — sybils have to participate in the network long
  enough to earn rep.
- Cons: bootstrap deadlock; reputation farming attacks (sock-puppet
  rings rating each other).
- Could combine with (A) — cost-floor + reputation-rate.

### C. Social-graph anchoring

MAIN claim requires N existing MAINs to vouch (each putting their
reputation on the line). A bad MAIN drags down its vouchers.

- Pros: rumor-network effects make farms costly to seed.
- Cons: privileged-class problem (early MAINs gatekeep). Vouchers
  have no skin in the game past the initial vouch.

### D. Continuous proof-of-personhood (off-chain)

WorldID, Idena, BrightID — third-party PoP integrations.

- Pros: meaningful sybil resistance.
- Cons: violates the "Rust-only, browser-resident, self-sovereign"
  ethos. Adds a centralized trust root.
- Out of frame for v1 but worth flagging.

### E. Accept parallel MAINs explicitly

Don't try to enforce 1-MAIN-per-human. Just publish per-MAIN
reputation and let downstream consumers (other agents) decide how
much aggregation across MAINs they trust.

- Pros: lowest-friction; respects the protocol layer.
- Cons: pushes the sybil problem one layer up. Doesn't solve the
  user's stated worry.

## Recommendation (placeholder)

For 1.0.0: a hybrid of (A) and (E) — a $LH-locked MAIN registration
that gates the MAIN flag on a subdomain NFT, paired with public
reputation that survives even when a wallet runs multiple MAIN
subdomains. The "MAIN" status becomes about *cost paid + behavior
tracked*, not *uniqueness guaranteed*.

Open: how the lock scales with reputation, who slashes, how a MAIN
can transfer (if at all).

## Where this lives in the code

- `contracts/src/facets/MainIdentityFacet.sol` — new facet on the
  diamond. Storage: `mapping(uint256 tokenId => MainState)`. Methods:
  `registerMain(uint256 tokenId)` (with payable $LH lock),
  `releaseMain(uint256 tokenId)`, `mainOf(address)` returning the
  MAIN tokenId for a holder.
- Bundle: `paint_apex` and `paint_tenant` show MAIN status. New
  admin action "make this MAIN" runs the facet call via the iframe
  signer.

## Next step

Pick one of the candidate mechanisms (or hybrid). Specify the cost
function and the slashing conditions concretely. Until then, this
remains an open design question — DO NOT ship a half-spec'd MAIN
flag on chain.
