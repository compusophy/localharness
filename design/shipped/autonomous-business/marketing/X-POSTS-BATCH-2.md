# X-POSTS-BATCH-2.md — evergreen standalone X/Twitter pool (batch 2)

> Ten **new** standalone X posts, distinct from the 10 in `CONTENT.md §1` and the three
> reply-chain threads (`CONTENT.md §2`, `READY-QUEUE.md #6`, `READY-QUEUE.md #8`). This is
> an **evergreen drip pool** — single posts to fire between the dated launch assets in
> `CALENDAR.md`, one at a time, spaced (never two in a day, never adjacent to a thread,
> never a near-duplicate of a recently-posted one — `RISKS.md` a.1 / guardrails #11/#12).
>
> Each post ≤280 chars (verified — see footer counts). Voice: technical, cypherpunk-indie,
> anti-bloat (`BRAND.md`). Every post fires as the AUTO lane (own `@localharness` account
> via the official API) and **must** carry the canonical disclosure as its immediate first
> reply + the X **Automated-account** native label.
>
> **Accuracy re-verified 2026-06-30 vs source:** crate **0.58.0**; the SDK backend list
> (Gemini/Claude/OpenAI/Mock) is a **backend trait** only — OpenAI/Mock/Gemma are SDK-only,
> NEVER framed as live in-app models (the hosted selector is Gemini Flash + Claude Opus,
> `src/app/model.rs`). Identity/mint is on **Tempo mainnet**; **no diamond/chain address is
> pinned**. x402/per-call settlement is described as a **mechanism** with no mainnet-live
> assertion (settlement is testnet-only). `$LH` is framed strictly as a flat usage credit
> (1 `$LH`/message), never a token to speculate on; **no earnings/self-funding claim** —
> self-funding is named as the OPEN problem. Pricing (1 `$LH`/msg) matches
> `src/docs_manifest.rs`. Demos `slither.`/`fractal.localharness.xyz` cited as URLs.

## Canonical disclosure (immediate first reply on every post — reuse verbatim)

```
AI-generated, human-reviewed. Posted by localharness's own automated account. #AI
```

Pair with the X **Automated-account** setting (the native bot label, linked to the human
operator) — a ToS requirement for automated posting, independent of the post text.

---

## The pool (10 posts)

**(B2-1) Sharp technical one-liner — native/wasm seam** · 216 chars
```
The same Rust binary runs your agent on tokio and in a browser tab. One crate, one backend seam (Gemini, Claude, OpenAI, Mock), zero rewrite between native and wasm32.

No Python graph to host. cargo add localharness
```

**(B2-2) "Every agent is a subdomain" hook — identity** · 235 chars
```
Every agent is a subdomain.

name.localharness.xyz isn't a profile page. It's an ERC-721 identity on Tempo mainnet with its own wallet, persona, and filesystem. The name IS the agent — and your key owns it.

Claim one: localharness.xyz
```

**(B2-3) Fair comparison — vs hosted-API-key frameworks** · 230 chars
```
Most agent frameworks are a Python dependency graph you host, wired to a vendor API key you rent.

localharness is one Rust crate that compiles to the browser and becomes an agent holding its own keys.

Not a framework. A network.
```

**(B2-4) Autonomous-business build story — what it is** · 270 chars
```
Build-in-public: we're standing up a company that's nothing but role-agents — exec, PM, coder, reviewer, accounting, HR, marketing. Each one a subdomain with its own wallet.

A "company" here = a guild + a shared treasury + role members. Composition, not a new contract.
```

**(B2-5) Autonomous-business build story — honest scope** · 253 chars
```
Building a company out of agents, in public — including the part that isn't solved.

The guild, treasury, bounties, and per-agent wallets all work. But it spends $LH on inference faster than it earns. Can agents pay their own way? That's the experiment.
```

**(B2-6) Demo CTA — slither (live URL)** · 220 chars
```
No app store. No install. Just a URL.

slither.localharness.xyz — a 512×512 multiplayer slither.io written in a Rust subset, compiled to wasm, drawn to a pixel framebuffer, peer-to-peer over WebRTC.

Open it in two tabs.
```

**(B2-7) Sharp technical one-liner — rustlite / cartridges** · 223 chars
```
rustlite: a Rust subset that compiles to wasm "cartridges" — no LLVM, no external toolchain, the compiler ships inside the crate.

Publish one and your subdomain serves it 24/7. No tab, no server. The loader is the runtime.
```

**(B2-8) Technical hook — pay-per-call / metered inference** · 237 chars
```
Agents that transact, not chatbots with a wallet plugin.

Each has an ERC-6551 account and an advertised per-call price in $LH. A call settles only after a successful reply — a failed reply never takes the money. Inference you can meter.
```

**(B2-9) CTA to localharness.xyz — no server, 24/7** · 251 chars
```
From cargo add to a live agent at your own subdomain, in one sitting.

No box to rent, no process to babysit. Publish once and name.localharness.xyz answers 24/7 — browser-resident (OPFS) plus an off-chain store, scheduled by a cron.

localharness.xyz
```

**(B2-10) Anti-grift — `$LH` is a credit, not a token** · 225 chars
```
$LH is a flat usage credit — 1 $LH per message, not a coin to pump. No presale, no governance-token theater.

Gas is sponsored on Tempo, so humans onboard with no wallet and no seed phrase. A real product, not a token launch.
```

---

## Length verification (X-weighted, ≤280)

All ten counted by code-point length (X weights em dash `—` and `×` as 1 each):

| Post | Chars | |
|------|-------|--|
| B2-1 | 216 | OK |
| B2-2 | 235 | OK |
| B2-3 | 230 | OK |
| B2-4 | 270 | OK |
| B2-5 | 253 | OK |
| B2-6 | 220 | OK |
| B2-7 | 223 | OK |
| B2-8 | 237 | OK |
| B2-9 | 251 | OK |
| B2-10 | 225 | OK |

Longest is B2-4 at 270. **All ten are ≤280.**

## Firing rules (operator quick-ref)

- **Drip one at a time** between the dated `CALENDAR.md` assets; never two pool posts in
  one day, never adjacent to an X thread (#6/#8) or single (#3/#4) — ≥1 day spacing, no
  bursts (`RISKS.md` a.1 / guardrail #11).
- **Rotate angles** so consecutive posts aren't both build-story or both demo-CTA; a
  similarity check against recently-posted copy gates the next one (guardrail #11).
- **Disclosure + Automated-account label on every one** (guardrail #9). Put any link in a
  reply, not the post body (links cost 13× and cut reach — `GROWTH.md §2.4`).
- **No cross-agent likes/RTs** on these (voting-ring/CIB → domain ban; guardrail #12).
- Each is AUTO (loop enqueues) but a **human flips it live** (`RISKS.md` a.4).
