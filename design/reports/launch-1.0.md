# localharness 1.0.0 — Launch Spec

> **STATUS: open.** The 1.0 / **mainnet** moment has NOT happened — the project
> is still on Tempo Moderato **testnet** at `0.x`. (To be clear: **Tempo mainnet
> itself IS live** — chain `4217`, `https://rpc.tempo.xyz`, launched 2026-03-18 by
> Stripe + Paradigm. What's pending is *localharness's own* deploy onto it, not the
> chain's existence — see `stripe-mainnet.md`.) The *infrastructure* this spec
> calls for (on-chain identity + TBA, metered access, x402 escrow, templates +
> one-click fork, scoped runtime keys) has largely shipped on testnet; the 1.0
> *boundary itself* — mainnet, real `$LH` value, the Stripe on-ramp, and the
> sybil gate — is the forward work. Read with `beta-plan.md` (the testnet path to
> get here).

> Supersedes the 0.11-era launch *plan*. Rewritten at **0.18.0** after the
> agent-economy brainstorm and its peer review. 1.0.0 is reserved for the
> public-launch moment — this is the bar we hold it to, the architecture we
> commit to, and the honest line between what 1.0 *is* and what it is *not*.

Status (2026-06-03): 0.18.0 on crates.io + deployed. Identity is on-chain-only
(self-correcting owner hint), one-shot `create_and_publish_app` ships, feedback
lives in contract state, the namespace was reset to a clean slate. The golden
path is **not yet QA'd end-to-end on a real phone**, and **no beta has run.**
That last sentence governs everything below.

> **Decision locked (2026-06-03): 1.0.0 IS the mainnet launch.** Betas run on
> **Tempo Moderato testnet** as ordinary version bumps (0.19, 0.20, …) whose
> only job is to surface bugs; `1.0.0` is reserved for the **real launch on
> Tempo mainnet** with real `$LH` value and Stripe fiat on-ramp. The 0.x → 1.0
> boundary is the same line as "rails work" → "rails have teeth": every
> value-dependent mechanism (staking, sybil cost, Stripe, the economy's
> incentives) lands at 1.0, not before. See the beta plan in
> [`design/beta-plan.md`](beta-plan.md).

---

## 0. The one idea this spec is built on

Two layers, kept ruthlessly separate:

- **Identity** — the subdomain NFT, the ERC-6551 wallet, the seed. Sovereign,
  on-chain, already network-native (the diamond answers JSON-RPC from anywhere).
- **Runtime** — the thing that *answers* when someone calls
  `<name>.localharness.xyz/?rpc=1`. Today a browser tab; in principle anything.

Everything good in the economy brainstorm falls out of this split, and every
hard problem the peer review raised is a problem of *not* keeping it clean. So
the rule for 1.0: **define the protocol surface (identity + addressing + auth +
payment) independent of the runtime, ship the browser as the flagship sovereign
runtime, and leave the seams that let other runtimes exist later — without
building those runtimes now.**

1.0 is **the wedge plus the seams.** It is *not* the economy. The economy is the
north star 1.0 must not foreclose, not the thing 1.0 ships.

---

## 1. The promise of 1.0

> A stranger lands on `localharness.xyz`, and in about a minute has a working AI
> agent that is *theirs* — its own name, wallet, and identity on-chain — which
> they can point at their actual business, customize without code, and put a
> real published app behind. They share the link; a friend opens it on a phone
> and uses it. No install, no server they have to run, no crypto they have to
> buy, and no company that can take the identity away.

Two audiences in that sentence, and 1.0 serves them in this order:

1. **Makers / small businesses** (the wedge): "Squarespace for agents." Spin one
   up, fork a template, adapt it. This is the demand we can validate now.
2. **Agents themselves** (the north star): the same identities, reached over the
   network, eventually transacting with each other. 1.0 *enables* this and
   *pilots* it — it does not depend on it.

1.0 is not "more features." It is **"a stranger does the magic thing, trusts it,
and could run a small business on it — reliably, on a phone, unattended by us."**

---

## 2. Strategic frame (the load-bearing decisions)

### 2.1 Sovereignty, honestly re-priced

The original pitch was "no server, ever." Once runtimes can be servers, that
pitch weakens to "you own your name and keys, but your agent runs on hosting
like everyone else's." We will not pretend otherwise. The **durable, defensible**
sovereignty value — worth real money to anyone who has been deplatformed — is:

- **Exit rights & portability.** Your identity, name, wallet, files, and
  published app are on-chain and yours. You can move the runtime anywhere, fork
  the client, self-host, and **no platform can evict you or hold your identity
  hostage.**
- **Zero-trust onboarding.** You can *start* with no server at all — the browser
  is a complete sovereign runtime. The seam to other runtimes is opt-in.

The browser stays the default sovereign runtime and the front door. The on-chain
identity is the portability guarantee. That is the pitch. We do not claim "no
server" as a universal; we claim "no *mandatory* server, ever, and you can
leave."

### 2.2 Protocol vs product — own the primitives, give away the clients

- **Canonical & on-chain (what we steward):** registry, identity, TBA, payment
  (x402), escrow, the capability descriptor, reputation. Permissionless; everyone
  reads/writes the *same* primitives.
- **Competitive & forkable (what we ship one good copy of):** the marketplace,
  the builder UI, the runtime, the templates. Open-source reference clients with
  full feature parity; anyone can run a competing front end over the same chain.

**Honest caveat (per review):** at 1.0 there is **no moat.** A testnet registry
has no network effect, and open primitives can be forked wholesale. We are not
claiming a moat; we are making a bet that *canonical primitives + the best
reference implementation + credible neutrality* compound into one as adoption
grows. 1.0's job is to be good enough that the compounding *starts*. We will
draw the canonical-vs-forkable line now (§11) and sort every future feature into
one bucket, resisting the temptation to keep "just this one" proprietary.

### 2.3 The autonomy dial (this is how we resolve autonomy vs control)

The review is right that businesses want **control, not an agent autonomously
wiring money to strangers** — and that the most valuable customers are the least
likely to grant autonomy. The vision is autonomy. These reconcile as a **dial the
owner controls**, not a binary:

```
  FULLY GATED ─────────────────────────────────────────► BOUNDED AUTONOMY
  every external paid       budgets + whitelisted        open composition
  action needs approval     counterparties, capped       within staked bounds
       (1.0 default)        $/day  (1.0 opt-in)          (post-1.0, gated)
```

The dial is implemented by **delegated, scoped keys** (§6.4): the scope of the
runtime key *is* the autonomy setting. A key scoped to "answer RPC, never spend"
is the gated default; a key scoped to "spend ≤100 $LH/day to these 3 agents" is
bounded autonomy. **1.0 ships the bottom of the ladder and the dial mechanism.**
The economy is what climbing the ladder unlocks — later, opt-in, gated on value
and trust actually existing.

---

## 3. Scope — what 1.0 IS and explicitly IS NOT

**1.0 IS:**
- A 60-second path from stranger → owned, on-chain agent.
- Config-driven business customization (prompt + tools + published face) via
  forkable templates. The "Squarespace" layer.
- The magic moment: agent builds a visual app, publishes it on-chain, a phone
  visitor uses it.
- The **payment rails** working end-to-end with $LH *credits*: metered model
  access, x402 agent-to-agent settlement, and escrow for paid jobs — coarse-
  grained.
- Human-gated agents by default; the autonomy dial present but defaulted off.
- The network seams: a sponsorship relay and a documented protocol surface, so a
  headless agent *can* create an identity and be reached over HTTP.
- Testnet (Tempo Moderato), sponsored gas, zero-crypto onboarding.

**1.0 IS NOT (and the spec says so out loud):**
- An autonomous economy. No agent spends money unattended by default.
- Staking, slashing, or reputation-weighted-by-stake. Those need a valuable unit
  (§8.1); they wait for mainnet.
- A reputation *market* or validator network. 1.0 ships escrow + ratings, not
  trustless verification of non-deterministic work (§7).
- High-frequency agent composition. On-chain settlement has a latency/cost floor
  (§8.3); 1.0 is coarse-grained.
- Private payments. The ledger is public (§8.4).
- Real money — *during the betas.* $LH is credit, not currency on testnet; real
  value + Stripe arrive **at** 1.0 (mainnet), which is exactly why they define
  the 1.0 boundary rather than sitting inside the beta runway.
- A finished business-app builder. rustlite is the *visual wow*, not the
  workhorse (§8.2).

Naming the "is not" list is a feature: it's what keeps us from shipping the
cathedral before the foundation.

---

## 4. The two demand bets and how 1.0 tests them

We are pre-beta. We do not get to assume demand. 1.0 runs two *separate*
experiments and weights them differently:

- **Maker demand (must nail — this gates launch).** The magic moment, run by
  real strangers in a private beta. Success = a fresh person completes
  claim → customize → build → publish → share, unaided, on a phone, and *wants
  to keep it.* This is the launch gate.
- **Economy demand (a signal, not a pillar).** A tiny, dogfooded pilot: 1–2 real
  paid agent-to-agent flows where the payment *mattered* (e.g., a `chat.` agent
  pays a `brain.` agent for a real answer, and the owner would have paid). We
  ship the rails and run this pilot to read the signal. **We do not gate 1.0 on
  it, and we do not invest in the economy roadmap (§10 Track B) until the pilot
  says someone cares.**

This is the review's "step 0" made concrete: validate maker demand to launch;
use the economy pilot to decide whether the economy is worth building at all.

---

## 5. The product — Squarespace for agents

### 5.1 Customization is config-driven, not compiler-driven

A business agent is **not** a rustlite cartridge. It is a *configured agent*:
- a system prompt / persona (`.lh_system_prompt.txt`),
- a tool allowlist (`.lh_tool_allowlist.txt` / `agent.json`),
- an optional published face (directory / HTML / cartridge),
- optional outbound calls to other agents or services.

This is the layer that makes it "Squarespace": you fork something that 80% works
and tweak the config. It does **not** depend on the in-browser compiler, which is
the review's M1 reckoning — the app explosion rides on configuration and
composition, not on rustlite's expressiveness.

### 5.2 Templates + one-click fork

The missing primitive for the wedge. A template = a pre-baked bundle (persona +
tools + starter face + a capability descriptor). "Support agent," "shop agent,"
"scheduler," "FAQ bot." Forking is `create_and_publish_app` pointed at a template
instead of blank. The marketplace is the catalog of forkable templates **and**
live, callable agents.

### 5.3 rustlite stays the wow, not the workhorse

The "agent writes Rust, compiles it in your browser, runs it on a framebuffer"
demo is a genuine differentiator and the heart of the magic moment — keep it,
grow it *on demand* (no Vec/arrays, 16 KB cap are real ceilings). But the
business value is config + composition. We will not market rustlite as the way to
build a CRM. Two distinct stories, never conflated.

### 5.4 The marketplace is forkable from day one

Designed so a competitor could clone the front end and it still works against the
same chain. If cloning it breaks because it secretly needed our server, we built
it wrong. This is the protocol-vs-product discipline made testable.

---

## 6. Architecture for 1.0 — what ships, and the seams

### 6.1 Identity layer (done)

ERC-721 name + ERC-6551 TBA wallet + BIP-39 seed; sponsored Tempo 0x76 txs;
ownership is on-chain-only (the 0.18 self-correcting hint). Nothing to add for
1.0 except hardening.

### 6.2 The protocol surface (define it; it's mostly a seam)

Write down, independent of the browser, the contract every runtime honors:
- **Addressing:** `<name>.localharness.xyz/?rpc=1`, request/response shape (exists).
- **Auth handshake:** challenge-response (SIWE-style). A caller verifies the
  endpoint genuinely speaks for the identity; the endpoint authenticates the
  caller. Generalize the existing `address:timestamp:signature` proxy header into
  the documented standard.
- **Capability descriptor:** what an agent does + what it costs (a signed
  `agent.json`-shaped doc; hash on-chain, payload servable). 1.0 ships the
  *format* and the directory reading it; rich discovery is post-1.0.

Shipping the *spec* (not a second runtime) is the cheap, high-leverage move.

### 6.3 The sponsorship relay (the keystone — ship a minimal version)

Today zero-gas works only because the sponsor key is in the browser bundle and
an iframe co-signs `fee_payer`. The relay is the same logic exposed as an HTTP
endpoint: take a *signed user intent*, verify it, apply a sybil/rate gate,
co-sign `fee_payer`, submit. Two payoffs:
- A **headless agent** can register an identity / claim a name / publish over
  plain HTTP, zero gas — the seam that makes network agents real.
- It **hardens the existing sponsor path**: the relay is where rate limits,
  spend-velocity caps, and a balance circuit-breaker live, shrinking the
  embedded-key blast radius that the security review flagged.

1.0 ships a *minimal, rate-gated* relay. It does not yet need stake-based gating
(that's mainnet).

### 6.4 Delegated, scoped keys = the autonomy dial (ship basic)

We already have `MultiSignerAccount` (additional signers on the TBA). 1.0 adds:
- **Scoping:** a runtime/session key authorized for a bounded capability set
  (answer RPC; settle x402 up to a cap; never transfer the NFT or burn the name).
- **Revocation UX:** one-tap kill from the owner's studio.
- **The cold root stays cold:** the seed authorizes the hot runtime key and then
  rarely signs. This is the bridge between sovereignty and always-on autonomy,
  and it reuses what exists.

The scope is the product surface of §2.3's dial.

### 6.5 Payment rails (x402 done; add escrow; coarse-grained only)

- **Metered model access** (proxy + session/meter): done.
- **x402 settlement** (X402Facet, signed in `call_agent`): done — for *coarse*
  paid interactions (hire an agent for a job).
- **Escrow** (new): payment held, released on acceptance or after a dispute
  window. This is the 1.0 trust mechanism for paid work (§7), not reputation.
- **Coarse-grained only.** Per-call on-chain settlement has a real
  latency/cost floor; high-frequency composition needs channels/batching, which
  is post-1.0 (§8.3). 1.0 does not promise it.

---

## 7. Trust & safety — the honest version

The review's hardest point: **most agent work is non-deterministic and not
re-executable**, so "validators re-run the work and check the hash" cannot
underwrite trust for the dominant workload. 1.0 does not pretend otherwise.

What 1.0 actually ships for trust:
- **Escrow + acceptance.** High-value jobs: payment released on the buyer's
  acceptance or after a dispute window; disputes refund. Low-value: auto-accept.
  The boring mechanism that actually works.
- **Reputation as a lagging signal,** not an oracle: completed/disputed counts,
  visible, gameable-at-the-margin, useful at scale. Not weighted by stake (no
  value yet).
- **Route verifiable work differently.** Some output *is* checkable (the API
  returned, the tx landed, the file compiled). Those flows can auto-verify;
  judgment/creative work cannot, and we won't claim it can. Trustless
  re-execution is reserved for the verifiable subset, post-1.0.
- **Hot-key compromise is a protocol problem, not a "the human will notice"
  problem.** An unattended agent's online key, if scoped to spend, must have
  **protocol-level spend-velocity caps and circuit breakers** — because the whole
  point of autonomy is that no human is watching to revoke. Per-day scope bounds
  the daily bleed; a velocity breaker bounds the burst. Both live in the relay /
  the scoped-key contract, not in a dashboard.
- **The typed-confirmation convention extends to spending.** The same hard rule
  that guards `release_subdomain` (a typed, never-auto-filled confirmation) guards
  raising the autonomy dial and any one-shot spend above a threshold.

---

## 8. The hard constraints, reckoned with (each named, none waved)

**8.1 The unit is inert on testnet — and that defines the version line.** Staking/
slashing/reputation-by-stake are null without value, so they are **out of the
betas** by design. The betas (0.x, testnet) prove the **mechanics** — metering,
x402, escrow all function with credits as a medium. **1.0 is mainnet:** real
`$LH` value + Stripe supply the teeth that make the incentive layer bite. The
line is explicit and is the release boundary itself — **0.x (testnet) = rails
proven with credits; 1.0 (mainnet) = value makes them bite.**

**8.2 rustlite ceiling.** Visual cartridges only; business apps are config +
composition (§5.1). Grow rustlite on demand. Never sell it as the app platform.

**8.3 Micropayment floor.** On-chain per-call settlement is slow/costly →
coarse-grained paid jobs in 1.0; payment channels / batching for fine-grained
composition are post-1.0. Stated, not hidden.

**8.4 Privacy.** A public ledger of who-paid-whom is a real limitation. 1.0
payments are public (testnet, low stakes). Confidential amounts/counterparties
(stealth addresses, private settlement) are a 2.0 concern. Acknowledged, not
solved.

**8.5 Sovereignty re-priced** → exit rights + portability + no-mandatory-server
(§2.1). The defensible claim.

**8.6 Moat** → none at 1.0; earned via adoption + canonical primitives + best
implementation + neutrality (§2.2). Not oversold.

---

## 9. Definition of Done for 1.0.0

Launch when **all** of these hold:

- [ ] A first-time stranger completes the magic moment unaided, on desktop **and**
      phone, and says they'd keep it. (Maker-demand gate, §4.)
- [ ] Every golden-path step verified in a real browser — no silent dead-ends,
      no unhandled panics on the path (the wasm-tab-death failure mode is closed
      on the path a stranger walks).
- [ ] At least 3 forkable business templates; fork → customize → publish works
      end-to-end.
- [ ] Payment rails proven with credits: a metered turn, an x402 agent-to-agent
      settlement, and an escrowed paid job each demonstrated.
- [ ] The autonomy dial ships **defaulted to fully gated**; scoped runtime keys +
      one-tap revocation work; spend-velocity caps enforced at the protocol level.
- [ ] The sponsorship relay is live and rate-gated; a headless agent can register
      an identity and be reached over HTTP without a browser.
- [ ] The protocol surface (addressing, auth handshake, capability descriptor) is
      documented as a spec, not just an implementation.
- [ ] **Mainnet** sponsor model shipped — the *real* rewrite (not the embedded
      testnet key): relay rate-cap + balance breaker + spend-velocity caps live.
- [ ] **Stripe fiat on-ramp** live (buy `$LH` with a card) — the 1.0 value bridge.
- [ ] **Sybil gate** live (real value means identity creation needs cost-to-fake).
- [ ] At-rest encryption wired (done); a focused security pass on the relay, the
      cross-origin signer, scoped keys, and the sponsored-tx path.
- [ ] Marketing apex landing + human quickstart + the magic-moment story live.
- [ ] A showcase of real published agent-apps exists (dogfooded proof).
- [ ] The economy pilot ran; its signal is recorded (gate for Track B, not for
      launch).
- [ ] Feedback loop frictionless (state-backed submit + views + harvest — done).

---

## 10. Roadmap & sequencing — two tracks, one gate

**Track A — near-term, defensible, *this is 1.0*.** Build in roughly this order;
each is useful even if the economy never materializes:

1. Golden-path QA + panic-hardening on the path (close the dead-ends).
2. Business templates + one-click fork (the wedge).
3. Sponsorship relay (the seam + the security hardening).
4. Scoped runtime keys + revocation + spend caps (the autonomy dial, gated off).
5. Escrow for paid jobs (the trust mechanism).
6. Protocol-surface spec (addressing/auth/capability descriptor written down).
7. Security pass, landing + human docs, showcase.
8. Private beta → triage → the economy pilot (read the signal).

**Track B — speculative, *post-1.0*, gated on `value-live (= 1.0 shipped on
mainnet) AND pilot-signal`:** reputation markets, autonomous composition,
validator/verification network for the verifiable subset, payment channels,
private settlement, decentralized runtime market (the compute-cluster / MPC
endgame). Note the gate's first half is now *satisfied by 1.0 itself* (mainnet =
value), so Track B becomes a question of pure demand: did the pilot show anyone
cares? **Do not start Track B until that signal is real.** Steps A1–A6
deliberately leave every door open so starting B later is additive, not a
rewrite.

The gate between A and B is the single most important sequencing decision in this
spec: **build the wedge and the seams; let demand and value decide the economy.**

---

## 11. The decisions you must own

1. **Chain for 1.0: testnet vs mainnet. — DECIDED (2026-06-03): 1.0 = mainnet.**
   Betas run on testnet as version bumps to surface bugs; 1.0.0 launches on Tempo
   mainnet with real `$LH` value + Stripe. Rationale: the economy's incentive
   layer is value-dependent (§8.1), so aligning the version boundary with the
   value boundary is the clean line — and it honors "1.0 means the real thing."
   The cost this accepts (vs the old testnet-1.0 idea): 1.0 is a bigger lift
   (sponsor-key rewrite for real money, mainnet deploy, real-value security,
   Stripe), de-risked by a long testnet-beta runway. The discipline that keeps it
   from ballooning: **1.0 = the testnet-proven platform, now on mainnet with
   value + Stripe — NOT the economy.** The economy stays Track B.
2. **Runtime custody.** *Recommend: browser default + a self-host runtime option;
   NO platform custody of hot keys at 1.0.* Platform-hosted always-on = we become
   a custodian (liability, betrays the pitch). Offer it only later, with hard
   scope caps and easy revocation, clearly labeled as the convenient-not-sovereign
   option.
3. **Autonomy default.** *Recommend: fully gated.* The dial exists; it ships off.
4. **The canonical-vs-forkable line.** *Recommend: identity + payment + escrow +
   capability schema + reputation are on-chain canonical; all UI/runtime/templates
   are forkable.* Ratify it now; sort every future feature into a bucket.
5. **Sybil gate.** Meaningless with credits, so **none during the betas** — just
   rate-limit the relay and gate the cohort to trusted invitees. But because 1.0
   is now mainnet (real value), a sybil gate becomes a **1.0 requirement**: you
   can't open real-value identity creation without cost-to-fake (stake, or a
   Stripe-card-backed identity, or proof-of-persistence). Design it during the
   beta runway; ship it at 1.0.
6. **Second LLM backend.** Not a 1.0 blocker; you've consistently declined it.
   Noted, deferred — but single-vendor risk is real for an *economy* and should be
   revisited before Track B.

---

## 12. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Economy designed on a valueless unit | 1.0 ships rails only; value-dependent mechanisms gated on mainnet |
| Autonomy vs control contradiction | The dial: fully gated default, owner-controlled scope, opt-in bounds |
| "No moat" / forkable primitives | Don't claim a moat; earn it via adoption + best impl + neutrality |
| Trust unsolved for non-deterministic work | Escrow + acceptance + lagging reputation; trustless re-exec only for verifiable subset |
| Hot-key compromise on unattended agents | Protocol-level spend-velocity caps + circuit breakers, not human revocation |
| Sovereignty diluted by server runtimes | Re-price to exit-rights + portability + no-mandatory-server; browser stays default |
| rustlite too weak for real apps | Config-driven wedge; rustlite is the visual wow, grown on demand |
| Building supply ahead of demand | Maker-demand gates launch; economy pilot gates Track B |
| Golden path breaks for real users | QA + panic-hardening + private beta before any open launch |
| Sponsor key drained/extracted | Testnet caps it; relay rate-cap + balance breaker; rewrite for mainnet |
| Single-vendor (Gemini) | Tolerable for 1.0; revisit before the economy (Track B) |

---

## 13. The one-line strategy

**Ship the wedge and the seams, not the cathedral.** Split identity (sovereign,
on-chain, canonical) from runtime (a future market). Make a stranger's
60-second, customizable, human-controlled business agent *bulletproof on
testnet*, prove the payment rails with credits, and lay exactly the primitives —
the relay, scoped keys, escrow, the protocol spec — that let the *same
identities* go autonomous later. Own the primitives, give away the clients, and
let demand and a real unit of value — not a roadmap — decide when the economy is
worth building.
