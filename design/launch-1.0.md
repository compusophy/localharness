# localharness 1.0.0 — Launch Plan

The grand plan for the first official public launch. 1.0.0 is reserved
for the public-launch moment, not a routine bump — so this document is
the bar we hold it to and the path that gets us there.

> Status as of writing: **0.11.0** shipped (the display release —
> framebuffer cartridges, rustlite host-imports, app mode, cross-visitor
> on-chain publishing). Three beta blockers cleared (touch input, key
> onboarding link, bad-key recovery). No beta has run yet.

---

## 1. The promise of 1.0

localharness 1.0 is the public launch of a **browser-native,
self-sovereign agent platform**: anyone can claim a subdomain, get an
AI agent that owns a wallet and an identity, and have that agent **build
and publish real apps — written in Rust, compiled in the browser** — that
anyone can then visit. No server we run. No backend. Substrate is the
Tempo chain + the user's browser tab.

1.0 is not "more features." It's **"a stranger can land, do the magic
thing, and walk away impressed — reliably, on a phone, without us
hand-holding."**

## 2. The magic moment (the north star demo)

Everything in this plan serves making *this one flow* bulletproof and
shareable:

> Claim `calc.localharness.xyz` → tell the agent *"make me a
> calculator"* → it writes rustlite, compiles in your browser, runs it on
> the display, and publishes it on-chain → you send the link to a friend
> → they open it on their phone and **use the calculator**.

Self-sovereign, agent-authored, no install, no backend. If that flow is
crisp and trustworthy, we have a launch. If any step is janky, we don't.

## 3. Phases

| Phase | Goal | Exit criteria |
|------|------|---------------|
| **0. Now** | post-0.11 cleanup | beta blockers closed |
| **1. Beta-ready** | the golden path is verified + smooth | a fresh person can do the magic moment unaided |
| **2. Private beta** | real users, real walls | 5–20 invited users; feedback triaged; magic moment holds |
| **3. Hardening** | the things beta + 1.0 demand | security, second backend, chain decision resolved |
| **4. Launch-ready** | the front door + the story | landing page, docs, showcase, economics set |
| **5. 1.0.0** | public launch | DoD (§7) met; ship + announce |

Beta is the engine: it converts guesses into the real 1.0 punch list.
Don't pre-build Phase 3/4 before beta tells us what matters.

## 4. Workstreams

### A. Product & UX
- **Golden-path QA** — click through every step in a real browser
  (identity → claim → key → chat → build → run → publish → visitor view).
  Half the path is unverified post-0.11 code.
- **Onboarding** — the BYO Gemini key is the #1 friction. Link shipped;
  next: validate the key on save (one cheap call) so failure is caught
  before the first turn.
- **Mobile** — touch input shipped; QA the whole flow on a phone.
- **Dead-ends** — every failure (RPC down, claim fails, compile error,
  publish fails) needs a visible path forward, not silent stalling.
- **Polish** — the chrome, the empty states, the first-run feel.

### B. Platform & capability
- **Display/cartridge maturity** — more draw ops (line, text already in),
  whatever beta apps demand. Don't over-build ahead of demand.
- **rustlite** — agents will hit language walls (no arrays/Vec, limited
  stdlib). Grow it as real cartridges need it.
- **Publishing UX** — make "publish this app" a one-tap, obvious action;
  show the shareable link prominently after publish.
- **Showcase / directory** — a browsable gallery of published agent-apps
  (ties into composable subdomains + the marketplace idea). Strong launch
  asset: proof the platform produces real things.

### C. Security & trust  *(the hardest 1.0 gate)*
- **Sponsor key** — currently embedded in the wasm bundle, extractable
  (`src/app/sponsor.rs`). Acceptable on testnet (capped, refillable,
  play money); **cannot survive a public mainnet launch.** Options:
  (a) Tempo access keys with scoped fee_payer (if supported — TBD by
  live test); (b) an on-chain paymaster/policy account as fee_payer with
  per-identity rate limits; (c) per-user passkey signing (but then who
  pays gas?); (d) stay on testnet for 1.0 and defer this to mainnet/2.0.
- **At-rest encryption** — module is written (`src/app/encryption.rs`)
  but **completely unwired**. Wire it (wallet-derived key over OPFS) so
  an XSS-class bug can't trivially exfiltrate seeds/keys.
- **Security review** — a focused pass over the signer iframe, the
  cross-origin postMessage protocol, OPFS isolation, and the sponsored
  tx path before inviting the public.
- **Abuse** — no per-user cap on sponsored gas today; daily allowance
  only gates `$LH` mints. Add a guard.

### D. Economics & chain  *(the decision that dominates)*
- **LLM cost is already solved**: BYO Gemini key → the user pays Google
  directly. localharness pays $0 for inference. Keep this.
- **Gas is the cost center**: every tx is sponsored. Free on testnet;
  real money on mainnet. This is *why* the chain decision gates 1.0.
- **$LH economics** — daily allowance (100) + registration cost (50 LH)
  give linear sybil resistance. Fine for launch; revisit if abused.
- **Chain decision** — see §5.

### E. Reliability, QA & observability
- **Golden-path QA** (also A) — the gate before beta.
- **Regression safety** — codegen output is only validated by hand in
  Node (see `[[project-rustlite-codegen-validation]]`); consider a
  dev-only wasm validator so the compiler can't silently regress.
- **Observability under no-backend** — we can't run analytics. Lean on:
  the on-chain feedback viewer (shipped), `harvest-feedback`, and direct
  user contact. Accept limited telemetry as the price of no-backend;
  make the feedback path frictionless so users actually report.

### F. Launch & growth
- **Marketing apex** — `localharness.xyz` needs a real landing that tells
  the story + shows the magic moment + "claim your agent."
- **Human docs** — a dead-simple quickstart (CLAUDE.md is for Claude,
  llms.txt is for agents; humans need their own page).
- **Beta cohort → testimonials** — invited users become proof + quotes.
- **Distribution** — where the launch is announced (AI + crypto + indie
  builder communities). The pitch: "your AI agent builds apps that live
  on the open web, and you own them."

## 5. The four decisions that gate 1.0 (with recommendations)

1. **Chain for 1.0: testnet vs mainnet.**
   *Recommendation: launch 1.0 on Tempo Moderato (testnet).* It sidesteps
   the embedded-sponsor-key blocker (play money, capped, refillable),
   lets every user do everything for **free** (no crypto, no gas, lowest
   possible barrier — a *feature* for adoption), and the platform's value
   (agent builds + publishes apps) is fully demonstrable without real
   money. Mainnet — with the sponsor rewrite and real ownership — becomes
   the **2.0** milestone. Tradeoff: testnet identities aren't "real"
   ownership; we frame the launch as "free, no wallet funding needed,"
   not "own real assets."

2. **Sponsor model.** Follows from #1. If testnet-1.0: keep the embedded
   key, add a balance monitor + abuse cap, done. If mainnet-1.0: this
   becomes a multi-week security project (paymaster/access-keys/passkeys)
   and gates everything.

3. **Second LLM backend for 1.0?** *Recommendation: yes, lightweight.*
   Wiring one non-Gemini backend (Anthropic or OpenAI) validates the
   `Connection`/`ConnectionStrategy` abstraction, removes single-vendor
   risk, and lets users pick. Not beta-blocking; do it in Phase 3.

4. **Beta scope.** *Recommendation: invited, ~10 testnet users first.*
   Trusted cohort bounds the sponsor-drain risk and gives high-signal
   feedback before any open exposure.

## 6. Roadmap (milestones; releases when stacks warrant)

- **0.12 — Beta-ready.** Golden-path QA fixes, key-validate-on-save,
  sponsor balance monitor + abuse cap, mobile QA, dead-end recovery.
- **Private beta.** Invite ~10. Triage feedback (now have the viewer).
  Iterate in patch/minor bumps as needed.
- **0.13 — Beta hardening.** Top beta fixes + second LLM backend +
  publishing UX polish + the showcase/directory v1.
- **0.14 — Security pass.** Wire encryption, security review, abuse
  hardening. (If mainnet-1.0 was chosen, the sponsor rewrite lands here
  and this phase is much larger.)
- **0.15 — Launch-ready.** Landing page, human quickstart docs,
  showcase, final economics. Dress rehearsal of the magic moment.
- **1.0.0 — Public launch.** DoD met → release + announce.

(Version parts double-digit-by-now is an accepted exception; minor bumps
mark milestones, releases happen when changes stack up.)

## 7. Definition of Done for 1.0.0 (the launch checklist)

- [ ] A first-time visitor completes the magic moment unaided, on
      desktop **and** phone.
- [ ] Every step of the golden path verified in a real browser; no
      silent dead-ends.
- [ ] Chain decision made; sponsor model appropriate to it (monitor +
      cap for testnet, or the real rewrite for mainnet).
- [ ] At-rest encryption wired; a security review pass done on the
      signer / cross-origin / OPFS / sponsored-tx surface.
- [ ] Second LLM backend available (provider not single-vendor).
- [ ] Marketing apex landing + human quickstart docs live.
- [ ] A showcase of real published agent-apps exists (dogfooded proof).
- [ ] Private beta ran; its blocking feedback is resolved.
- [ ] Feedback loop frictionless (submit + viewer + harvest).

## 8. Risks & mitigations

| Risk | Mitigation |
|------|------------|
| Sponsor key drained/extracted | testnet-1.0 caps it to play money; monitor + abuse cap; rewrite for mainnet |
| Golden path breaks for real users | QA pass + private beta before any open launch |
| Gemini-key friction kills onboarding | inline "get a free key" (shipped) + validate-on-save + a 60s guide |
| Single-vendor (Gemini) outage/policy | second backend in Phase 3 |
| No telemetry (no backend) → flying blind | frictionless feedback + direct beta contact + on-chain harvest |
| rustlite too limited for real apps | grow it on demand from beta; the showcase reveals the ceiling |
| "Testnet = not real" perception | frame as "free, no crypto needed"; mainnet ownership = 2.0 |

## 9. The one-line strategy

**Make the magic moment bulletproof on testnet, prove it with a small
beta, wrap it in a real front door — and launch 1.0 free-and-frictionless,
saving real-money mainnet ownership for 2.0.**
