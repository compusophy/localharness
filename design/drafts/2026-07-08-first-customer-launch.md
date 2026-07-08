# First-customer launch — channel drafts (2026-07-08)

LHCO's first-customer push. **Nostr already FIRED** by the loop
(event `f7d6a7fb6a7f98e2e8f0318bf1e9df055d9177abd1bb77226f2524ccecaf9324`,
npub1ctevx4s…, live on primal.net + nos.lol). The rest below are **human-fired**
— your accounts, your call on timing.

## The offer mechanics (reply-gated, not embedded)
- 10 onboarding invite codes minted (3 $LH each, 90-day TTL, escrowed by `claude`).
  A live `?invite=` code in a public post gets scraped + drained by bots in
  minutes, so **do NOT paste a code publicly** — offer it, hand it out on reply/DM.
- To see / send codes: `localharness invite list --as claude`; give a requester
  `https://localharness.xyz/?invite=<code>` (browser onboard) or tell them
  `localharness onboard --invite <code> --as <name>` (CLI). Reclaim unredeemed
  later with `localharness invite reclaim --as claude <code>`.
- Quickstart asset: <https://github.com/compusophy-bot/localharness-quickstart>
- Source: <https://github.com/compusophy/localharness> · crate:
  <https://crates.io/crates/localharness>

---

## Farcaster — @localharness (cast, ≤320)
> LOCALHARNESS — a Rust, model-agnostic agent SDK + a browser platform built on it.
>
> `cargo add localharness`: agent loop, tool-calling, hooks, MCP. Gemini/Claude/GPT.
>
> browser-app → your agent at <name>.localharness.xyz. Wallet = identity, on-chain, no servers.
>
> Free starter code, first agent — reply. github.com/compusophy-bot/localharness-quickstart

## X / Twitter — thread
> 1/ LOCALHARNESS is two things in one Rust crate: a model-agnostic agent SDK, and a self-sovereign, browser-resident agent platform built on it. Open source. Early. You own the keys.
>
> 2/ `cargo add localharness` gives you an agent loop: streaming text, tool-calling, hooks, policies, triggers, MCP, context compaction. Backends: Gemini, Anthropic, OpenAI, and a deterministic Mock for tests. Native or wasm.
>
> 3/ Build with the browser-app feature and every user gets a live agent at <name>.localharness.xyz. Identity is a wallet — on-chain on Tempo. No signup, no servers, no database. The agent is yours.
>
> 4/ A free starter code funds a newcomer's first agent — claim your own <name>.localharness.xyz and run it, no card. It's early; treat it that way. Reply and I'll send a code. github.com/compusophy-bot/localharness-quickstart

## Reddit — r/rust (first choice), r/AI_Agents (second)
Etiquette (from the copy): pick the right flair, disclose you're the author,
keep the offer at the bottom (never in the title), don't paste identical copy
across subs, and stick around to answer comments.

**Title:** LOCALHARNESS: a Rust-native, model-agnostic agent SDK (+ a browser agent platform built on it)

**Body:**
> I've been building LOCALHARNESS, an open-source agent SDK in Rust. `cargo add localharness` gives you an agent loop — streaming, tool-calling, hooks, policies, triggers, MCP, and context compaction — behind a Connection/ConnectionStrategy seam so the backend is swappable. Gemini, Anthropic, OpenAI, and a deterministic Mock backend ship in the crate. It compiles native (tokio) and to wasm32.
>
> The same crate has a second half: build with the `browser-app` feature and it becomes a browser-resident agent platform. Every user gets a live agent at <name>.localharness.xyz; identity is a wallet, state is on-chain (Tempo), and there's no server or database in the request path.
>
> Honest status: the SDK is the stable part. The platform half is early and moves fast, it's self-sovereign — you hold the keys, lose them and you lose the agent — and it leans on one off-chain component (a credit proxy) for LLM billing. Rather you know that going in.
>
> Repo: github.com/compusophy/localharness · crates.io/crates/localharness · quickstart: github.com/compusophy-bot/localharness-quickstart
>
> If you want to try the platform side without wallet/gas friction, I have free starter codes that fund a first agent (no signup, no card) — reply or DM and I'll send you one. I'm the author — happy to answer anything, especially on the SDK's trait boundaries.

## Show HN
**Title:** Show HN: LOCALHARNESS – Rust agent SDK + self-sovereign browser agent platform

**First comment:**
> Author here. LOCALHARNESS is one Rust crate that does two things.
>
> (1) An agent SDK. `cargo add localharness` gives you an agent loop with streaming text, tool-calling, hooks, policies, triggers, MCP, and context compaction. The backend sits behind a Connection/ConnectionStrategy trait, so it's model-agnostic — Gemini, Anthropic, OpenAI, and a deterministic Mock backend ship in-crate. It builds native (tokio) and to wasm32.
>
> (2) A platform. Compile the same crate with the `browser-app` feature and you get a browser-resident agent: every user gets a live agent at <name>.localharness.xyz. Identity is a wallet, state is on-chain (Tempo), and there's no server or database in the request path — the browser is the runtime.
>
> What's honest about the state: the SDK is the solid part. The platform half is early and moves fast, it's self-sovereign so you hold the keys (lose them, lose the agent), and it leans on one off-chain component (a credit proxy) for LLM billing. I'd rather you know that going in.
>
> Repo: github.com/compusophy/localharness — crates.io/crates/localharness. Free starter codes for a first agent (no signup/card) — ask and I'll send one. Feedback welcome, especially on the SDK's trait boundaries.

## Email (warm / opt-in recipients only — NOT cold spam)
**Subject:** LOCALHARNESS is open — claim your first agent

> Hi,
>
> Quick note since you asked to hear when this was ready.
>
> LOCALHARNESS is live. It's two things in one Rust crate:
>
> - An agent SDK: `cargo add localharness` gives you an agent loop (streaming, tool-calling, hooks, policies, MCP, compaction) that's model-agnostic — Gemini, Anthropic, OpenAI, or a Mock backend.
> - A platform: build with the browser-app feature and you get a live agent at <name>.localharness.xyz. Identity is a wallet, on-chain on Tempo, no servers.
>
> I have a free starter code that funds your first agent — claim a <name>.localharness.xyz and run it, no signup and no card. Reply and I'll send you one.
>
> Fair warning: it's early and self-sovereign, so you own the keys and the responsibility that comes with that. Open source if you want to read the code first: github.com/compusophy/localharness
>
> Would genuinely value your read on it.
>
> —
