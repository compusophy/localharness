# localharness — Private Beta Plan

> **STATUS: open.** The structured private beta described here has not run; this
> is the operational instrument that gates the mainnet 1.0 launch. Some
> prerequisites it lists have since shipped (state-backed feedback, the silent
> wallet trap, the resolve loop), but the beta *process itself* is forward.

> Operational companion to [`launch-1.0.md`](launch-1.0.md). The betas run on
> **Tempo Moderato testnet** as ordinary version bumps (0.19, 0.20, …). Their
> single job is to **surface bugs and validate maker demand** before the 1.0.0
> **mainnet** launch (real `$LH` value + Stripe). A beta is not a launch; it is
> the instrument that tells us when we're allowed to start the launch.

---

## 0. What a beta is for (and what it is not)

- **For:** finding the bugs a real stranger hits, and answering one question —
  *does a fresh person complete the magic moment unaided, on a phone, and want
  to keep it?*
- **Not for:** polish, scale, marketing, or the economy. Those come later.

The whole value depends on testers hitting **readable, recoverable** failures.
A silent white-screen teaches nothing — you don't even learn the bug exists.
That is why the pre-beta gate below is non-negotiable.

---

## 1. Pre-beta gate — must ALL hold before the first invite

1. **Self golden-path walkthrough, done, twice — desktop AND phone.** You walk
   every step yourself, in a real browser, and log every break. This is the
   single cheapest bug-surfacing you will ever get and it has not been done. Do
   it first. Every break you find is a round a tester doesn't waste.
2. **Panic surface hardened *on the path*.** Not all ~200 `unwrap`/`expect` —
   just the ones a stranger walks. On the golden path, a failure must render *an
   error you can read*, never a dead tab. ("publish failed: out of gas" beats
   "it broke and I don't know where.")
3. **A recovery path for every failure mode on the path.** RPC down, claim
   fails, compile error, publish fails, key invalid — each shows a visible way
   forward, not a stall.
4. **Sponsor guardrails active.** Balance monitor + per-origin relay rate cap so
   one tester (or a loop) can't drain the sponsor and brick claims for everyone.
   (Partly shipped — `events.rs` rate cap + balance monitor; confirm it holds.)
5. **Feedback loop confirmed end-to-end.** A test submission → on-chain state →
   `harvest-feedback.sh` reads it back. (State-backed as of 0.18 — verify.)

If any of these is false, you are not gated for *a productive* beta yet — only
for a frustrating one.

---

## 2. The golden path (the exact walk a tester takes)

| # | Step | Success | Most likely failure |
|---|------|---------|---------------------|
| 1 | Land on `localharness.xyz` | Sees the pitch + "create identity" | Confusing first screen |
| 2 | Create identity | Seed generated, persisted | Silent wallet trap (fixed 0.17 — confirm) |
| 3 | Claim a subdomain | NFT minted, lands in studio | Resolve loop (fixed 0.18 — confirm), gas-out |
| 4 | Get model access | Credits or BYOK works first turn | Key invalid, no credits, proxy auth |
| 5 | Chat / give a task | Agent responds, streams | Model 400, tool misfire |
| 6 | Build an app | `create_and_publish_app` / cartridge runs | rustlite compile error, 16 KB cap |
| 7 | Publish | On-chain, public face set | setMetadata gas, tokenId lag |
| 8 | Share the link | Friend opens `<name>.localharness.xyz` | Public-face resolution, verify pill |
| 9 | Visitor uses it on a phone | App works, touch input | Mobile layout, framebuffer |
| 10 | Submit feedback | Lands in contract state | — (now state-backed) |

The bar for "this works": a fresh person gets from 1 → 9 **unaided**, on a phone.

---

## 3. The cohort

- **Small, invited, trusted: ~5–15.** Bounds sponsor drain and gives
  high-signal, reachable feedback. Resist going wider — an open beta on an
  unverified path just generates noise you can't act on.
- **Invite mechanism:** invite codes — `?invite=CODE` already auto-redeems
  `$LH`. One code per tester; rate-limited.
- **Mix deliberately:** some technical, some not; weight toward **phone-first**
  users, because the phone is where the magic moment lives or dies.

---

## 4. The loop — rounds map to version bumps

```
invite cohort ─► they use it ─► harvest feedback + direct contact ─►
   triage ─► fix ─► version bump (0.19, 0.20, …) ─► next round
```

- Each **round** = one cohort pass against one build.
- Each **version bump** ships that round's fixes (CHANGELOG per bump; the
  state-backed feedback log is the raw input).
- Reuse testers for regression rounds; rotate in fresh ones for unaided-
  first-run signal (fresh eyes are the only true magic-moment test).
- Keep bumping `0.x` — **never bump to 1.0 for a beta.** 1.0 is mainnet.

---

## 5. What to measure (no analytics backend — lean on these)

- **On-chain (free, real):** claims, publishes, feedback entries — all readable
  from contract state. `feedbackCount()` / `harvest-feedback.sh`, `nextId()`,
  `list_owned_tokens`.
- **Qualitative (the real signal):** did they finish unaided? where did they
  stall? did they come back the next day? *would they keep it?*
- **The one bar that matters:** a fresh person completes 1 → 9 on a phone,
  unaided, and says they'd keep it. Everything else is diagnostics.

Accept limited telemetry as the price of no-backend; make feedback frictionless
so people actually report (it is — one tool call, on-chain, harvestable).

---

## 6. Maker-demand exit criteria — when the beta is "done"

Move to **mainnet 1.0 prep** when ALL hold:

- [ ] **3+ consecutive fresh testers** complete the magic moment unaided, on a
      phone.
- [ ] Top blocking feedback resolved (triaged from contract state).
- [ ] No known silent dead-ends remaining on the golden path.
- [ ] You'd be comfortable a stranger does this with *real money* on the line.

That last bullet is the bridge to 1.0: the beta proves the *mechanics* on
testnet; clearing it is what earns the right to start the mainnet/Stripe/sybil
work.

---

## 7. The economy pilot (parallel, a signal — NOT a beta gate)

Separately from the maker beta, run a tiny dogfooded pilot:

- **1–2 real paid agent-to-agent flows** — e.g. a `chat.` agent pays a `brain.`
  agent (x402, escrowed) for an answer the owner genuinely wanted.
- **Read:** did it settle mechanically? would the owner have paid *real* money
  for it?
- **It gates Track B (the economy roadmap), not the beta and not 1.0.** If the
  pilot shows nobody cares, the economy waits — and 1.0 is still the maker
  platform on mainnet, which stands on its own.

---

## 8. Guardrails during the betas

- **Testnet = safe to break.** Bugs cost play money, not real money.
- **Sponsor:** balance monitor + abuse/rate cap active; trusted cohort bounds
  the drain; refill via `tempo_fundAddress` when low.
- **Scope:** no autonomy by default (the dial ships off — see launch-1.0 §2.3);
  no real-value flows; no Stripe. All of that is 1.0.

---

## 9. Versioning during the betas (locked)

- `0.19, 0.20, 0.21, …` — one per round-worth of fixes. Double-digit minors are
  the accepted house style past 0.10.
- **`1.0.0` is reserved for the mainnet launch.** Never spend it on a beta, a
  hotfix, or a "feels big" release. It means: real chain, real value, Stripe,
  sybil-gated, sponsor-rewritten, golden-path-proven.
