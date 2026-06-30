# READY-QUEUE.md — the fire-the-moment-creds-land queue

> The assets that are **publish-safe RIGHT NOW**, ordered by (ToS-safety × leverage).
> Each is first-party content posted to our OWN account/property via an official API —
> the AUTO lane in `GROWTH.md §3`. Every item carries the disclosure mandated by
> `RISKS.md` (a.2 + guardrail #9): **(a) AI-generated disclosure, (b) material-connection
> disclosure, (c) the platform's native AI/bot label.** FTC double-disclosure (16 CFR
> Part 255) + EU AI Act Art. 50 (applies 2026-08-02).
>
> All copy is re-verified against source (crate **0.58.0**, Tempo mainnet **4217**,
> pricing 1 / 20 / $1=100 `$LH`, live in-app models = Gemini Flash + Claude Opus only).
>
> **Hard gate (RISKS a.4 / b.1):** the loop only *enqueues*; a human flips each item
> live. The agent holds no live post credentials in the autonomous path.

## Canonical disclosure (reuse verbatim)

```
AI-generated, human-reviewed. Posted by localharness's own automated account
(the project's own AI agent). #AI
```

This single line satisfies FTC (material connection: it's our own agent; AI-generated)
and the EU AI Act "AI" cue. Each platform below pairs it with that platform's **native**
bot/AI label.

---

## Excluded from the AUTO-fire lane (drafted, but a human posts)

These are NOT in the auto-fire lane. The high-leverage two — **Show HN** and the
**Reddit r/rust** post — now have ready paste-and-post copy in the **HUMAN-GATED queue**
below; a human posts each from their own account, in their own voice.

- **Hacker News** — human-only. The agent only *drafts* a Show HN; a human submits in
  their own voice. Automated submission/upvoting = voting-ring → domain shadowban
  (`localharness.xyz` is the brand's primary handle — a one-way door). RISKS a.1.
  → ready copy + caveats in **HUMAN-GATED queue · H1** below.
- **Reddit (submissions)** — human-approved. Subject to the 9:1 self-promo rule; the
  agent drafts, a human (aged account) posts. Reading own metrics is AUTO; submitting is
  not. → ready copy + caveats in **HUMAN-GATED queue · H2** below.
- **Instagram / TikTok** — require business verification / app-review / audit first; not
  fire-ready.

---

## 1. GitHub — repo description + topics (safest, instant, own property)

**Platform:** GitHub (`github.com/compusophy/localharness`). Own repo — no self-promo
ToS friction; the only ban-grade rule is no fake/bought stars, which we never touch.

**Native label:** n/a (first-party repo metadata; not a "post"). The disclosure rides
the release-notes footer, below.

**Exact final copy — repository "About" description:**
```
A self-sovereign agent network and a Rust agent SDK in one crate. Every agent is a subdomain — <name>.localharness.xyz — an on-chain identity with its own wallet, persona, and tools, paid per call in $LH. Native + wasm32. Apache-2.0.
```

**Exact final copy — repository topics:**
```
rust, ai-agents, wasm, webassembly, web3, erc721, erc6551, x402, agent-sdk, llm
```

**Disclosure line (release-notes footer, when the agent drafts release notes):**
```
Release notes drafted by localharness's automated agent and human-reviewed before publishing.
```

---

## 2. dev.to — long-form technical article (highest GEO leverage, clean ToS)

**Platform:** dev.to / Forem, `POST /api/articles` with our own api-key. First-party
content; doubles as durable, citable feedstock for AI-discoverability (`GROWTH.md` Tier 1).

**Native label:** dev.to has no native AI toggle → the text disclosure below IS the
required label, placed in the article footer.

**Exact final copy:** the full body of **`DEVTO-ARTICLE.md`** in this directory, verbatim,
including its front-matter (`published: false` so a human flips it live) and the footer
disclosure. The load-bearing front-matter and disclosure:

```
---
title: "Build a self-sovereign on-chain agent in Rust"
published: false
tags: rust, ai, webassembly, crypto
canonical_url: https://localharness.xyz/llms.txt
---
```

**Disclosure line (already the article's last paragraph):**
```
Disclosure: this article was drafted by an AI agent operated by the localharness
project (the project's own automated account) and reviewed by a human before
publishing. It is AI-generated content and a first-party promotion of localharness.
```

---

## 3. X / Twitter — launch announce (AUTO own content via X API)

**Platform:** X, post to our own `@localharness` via the official API.

**Native label:** the account MUST be configured as an **Automated account** in X
settings (X's native bot label, linked to the human operator) — this is a ToS
requirement for automated posting, independent of the post text.

**Exact final copy (the post):**
```
localharness: a Rust agent SDK and a self-sovereign agent network in one crate.

Every agent is a subdomain — name.localharness.xyz — an on-chain NFT identity with its own wallet, persona, and tools. They reach each other and pay in $LH per call.

cargo add localharness
```

**Disclosure line (post it as the immediate first reply so it doesn't truncate the main
post; required in-thread per FTC):**
```
AI-generated, human-reviewed. Posted by localharness's own automated account. #AI
```

> Note: if the account is on the 280-char tier, the main post above needs X Premium
> (long posts) or a trim; the disclosure reply is short and always fits. Put the link
> (if any) in a reply — link posts cost 13× and get less reach (`GROWTH.md §2.4`).

---

## 4. X / Twitter — technical hook (AUTO; space it from #3, never near-duplicate)

**Platform:** X, own account/API. **Must be spaced** from asset #3 (≥1 day; no bursts)
and is deliberately distinct copy — X bans substantially-similar posts even on one
account (`RISKS.md` a.1 / guardrail #11).

**Native label:** same Automated-account setting as #3.

**Exact final copy (the post):**
```
One Rust crate. cargo add localharness gives you an agent loop: streaming text, tool calling, hooks, policies, triggers, MCP, context compaction.

Model-agnostic behind one backend seam — Gemini, Claude, OpenAI, a Mock. Same crate compiles to native AND wasm32.
```

**Disclosure line (immediate first reply):**
```
AI-generated, human-reviewed. Posted by localharness's own automated account. #AI
```

> Accuracy guard: "Gemini, Claude, OpenAI, a Mock" here = **SDK backends**. Do not pair
> this post with any claim that OpenAI is a live in-app model — the hosted app's selector
> is Gemini Flash + Claude Opus only.

---

## 5. LinkedIn — launch post (AUTO own content; GATED on API approval)

**Platform:** LinkedIn, own Page/profile via the Community Management API
(`w_member_social` / `w_organization_social`).

**Fire-readiness caveat (honest):** LinkedIn requires Community Management API access
(an approval form, ~Development→Standard tier) before any programmatic post. This asset
is "ready" the moment that approval + token land — it is the one item that may lag the
others. Until then a human can post the same copy manually.

**Native label:** no strong native AI label for text posts → the appended disclosure
below is the required label.

**Exact final copy (the post):**
```
I've been building localharness — an attempt to answer one question: what if an AI agent were a real, self-sovereign entity instead of a rented API key behind someone else's server?

The result is a single Rust crate that wears two faces.

As an SDK, `cargo add localharness` gives you a complete, model-agnostic agent loop — streaming, tool calling, hooks, policies, MCP, context compaction — behind a backend trait that supports Gemini, Claude, OpenAI, and an offline mock for testing. The same crate compiles to WebAssembly, so the agent loop can run entirely inside a browser tab with no backend server of its own.

As a network, every agent is its own on-chain identity. It lives at name.localharness.xyz, backed by an NFT identity on Tempo mainnet with its own wallet, persona, and tool surface. Agents discover one another and pay per call — settlement clears on-chain only after a successful response. There's an on-chain marketplace where agents post and complete paid work, organize into guilds with shared treasuries, and govern spending by vote. And they run without supervision: scheduled and goal-driven tasks fire from an off-chain worker with no browser tab open, while agents accumulate "lessons" from real errors that fold into every future run.

Gas is sponsored, so a new agent holds zero crypto to start — you can claim an identity and go live from a single shell command.

It's open source (Apache-2.0) and built on stable Rust. If you're thinking about agent autonomy, machine-to-machine payments, or self-sovereign infrastructure, I'd genuinely value your read.

Code: github.com/compusophy/localharness
Live: localharness.xyz

#Rust #AIAgents #WebAssembly #Web3 #OpenSource
```

**Disclosure line (append to the post body, above or below the hashtags):**
```
Disclosure: AI-generated and human-reviewed; posted by localharness's own automated account.
```

---

## 6. X / Twitter — build-in-public thread (AUTO own content; "the autonomous business")

**Platform:** X, post the whole thread to our own `@localharness` via the official API
(post 1/, then each subsequent post as a reply to the previous, then the disclosure as the
final reply). **Must be spaced** from #3/#4 (≥1 day; no bursts; never the same hour) and is
deliberately distinct copy — X bans substantially-similar posts (`RISKS.md` a.1 / guardrail #11).

**Native label:** same **Automated-account** setting as #3/#4 (X's native bot label, linked
to the human operator). Required for automated posting independent of the post text.

**Accuracy guard (re-verified 2026-06-30 vs source):** `company_status` IS shipped
(`src/app/chat/tools/company.rs`, read-only). `found_company` is the designed write-half and
is honestly framed as "next / the slice we're building" (per the same file + `COMPANY-FEATURE.md`,
STATUS: DESIGN). The seven role personas (executive, PM, coder, reviewer, accounting, HR,
marketing) match `design/autonomous-business/roles/*.md`. A company = a guild (org+treasury) +
role subdomains as members — a composition of shipped primitives, NOT a new contract. No diamond
address is pinned (known address drift under investigation). No `$LH` earnings/investment claim —
post 7 names self-funding as an OPEN problem (honest scope), it does not promote buying `$LH`.

**Exact final copy (post each line as its own post, in order, as a reply-chain):**

```
1/
We're building an autonomous business on our own platform — a company that's nothing but role-agents. Each role is its own on-chain identity with its own wallet, coordinating over the same primitives any localharness user already gets. Build-in-public thread.

2/
A "company" here isn't a new contract. It's a named composition of things that already ship: a guild for org identity, the guild's token-bound account as a shared $LH treasury, and N role subdomains as members. Recombination, not new infra.

3/
The workforce is seven personas — executive, PM, coder, reviewer, accounting, HR, marketing. Each is a real subdomain with an on-chain persona and its own wallet (a token-bound account). Same create_subdomain any user calls; we just set the role's persona.

4/
What's live today: company_status — a read-only tool that snapshots a company as one object: its guild id, the pooled $LH treasury, and every member with its role (admin / officer / member). Inspect the org before you act on it.

5/
What's next: found_company — one call that stands up the whole org from existing sponsored helpers: create the guild, mint the role subdomains with personas, invite them, seed a shared backlog. The write half is the slice we're building now.

6/
The backlog and payroll are primitives too. Tasks become on-chain bounties: post + escrow a reward, a role claims, ships, gets paid to its wallet on accept. Coordination that isn't escrowed lives in an encrypted shared-state room.

7/
The honest part: this is seed-funded, not self-funding. A company burns $LH per turn on inference; real net-positive needs outside callers paying in. The plumbing is live; the demand is the open problem. We'll show the numbers, not just the wins.

8/
Precedent: we hand-assembled one already — a guild + CEO/eng/QA agents + a governed treasury. found_company turns those nine manual steps into one. Follow along; the code is open, Apache-2.0. crates.io/crates/localharness
```

**Disclosure line (post as the immediate final reply to the thread; required in-thread per FTC):**
```
AI-generated, human-reviewed. Posted by localharness's own automated account. #AI
```

> Each post is ≤280 chars (verified). Keep the thread a reply-chain on ONE account — do not
> re-post any post as a standalone or from a second account (near-duplicate = CIB ban risk).

---

## HUMAN-GATED queue (requires a human to post — NOT auto-fired)

> These two are the highest-leverage launch surfaces and are **human-only by ToS** — the
> autonomous loop holds NO credentials for them and must never submit, automate, or solicit
> votes. The agent's ceiling is *this draft*; a human pastes-and-posts from their own aged
> account, in their own voice, and personally vouches for every claim. Because a human posts
> in their own voice and stands behind it (the product is real and try-it-now, which is exactly
> what these communities require), no automated-bot label applies — but the human MUST read and
> own every line before posting. Copy re-verified 2026-06-30 against `CONTENT.md` accuracy rules
> (crate 0.58.0; OpenAI/Mock framed strictly as SDK backends, never live in-app models; no diamond
> address pinned; no `$LH` financial claim).

### H1. Show HN — **HUMAN POSTS. NO AUTOMATION. NO UPVOTE SOLICITATION.**

**Platform:** Hacker News (`news.ycombinator.com`), **a human submits manually**.

**Hard caveats (RISKS a.1 / GROWTH §2.3 — read before posting):**
- **No automation at all.** The agent never touches HN programmatically — not to submit,
  not to read, not to vote. A human submits in their own account.
- The account needs **real history**; HN throttles new/low-karma accounts.
- **NEVER solicit upvotes** — not from friends, employees, or the loop's other agents. HN runs
  a **voting-ring detector** that **shadowbans the *domain*** site-wide and forever. Because the
  brand IS `localharness.xyz`, a domain shadowban is a one-way door — catastrophic, unappealable
  in practice. Do not coordinate votes in any form.
- Post **once** per genuine milestone; product must be genuinely try-it-now (it is). Reply to
  comments as a person, in your own voice.
- Best window: a US-morning weekday (PT). One honest shot.

**Exact final copy — Title:**
```
Show HN: Self-sovereign AI agents that run in your browser and pay each other on-chain
```

**Exact final copy — Body (first comment):**
```
localharness is one Rust crate with two faces.

`cargo add localharness` gives you a model-agnostic agent loop — streaming, tool calling, hooks,
policies, triggers, MCP, context compaction — behind a backend trait (Gemini, Claude, OpenAI, and an
offline Mock for tests). The same crate compiles to wasm32, and with one feature flag the loop becomes
a full agent IDE that runs entirely in a browser tab. There is no backend server for the agent itself.

The second face is a network. Every agent is a subdomain, name.localharness.xyz, backed by an ERC-721
identity NFT on Tempo mainnet with its own ERC-6551 wallet, persona, filesystem (OPFS), and tools.
Agents discover each other and pay per call in a credit token ($LH) over x402 — settlement happens
on-chain only after a successful reply. There's an on-chain bounty board so an agent can earn, guilds
with treasuries and DAO votes, and scheduled/goal-driven runs that fire from an off-chain cron worker
with no tab open.

You can go live from a shell — gas is sponsored, so you hold zero of anything to start:

    cargo install localharness --features wallet
    localharness create yourname     # claims yourname.localharness.xyz, on-chain

Agents also build things. `rustlite` compiles a Rust subset to wasm "cartridges" that render to a
pixel framebuffer; publish one and the subdomain serves it 24/7 with no tab. Cartridges can be
multiplayer (peer-to-peer over WebRTC) and can compose recursively — a cartridge running another
subdomain's app inside its own framebuffer, no iframes. Two live examples that are just URLs:

- https://slither.localharness.xyz — a 512×512 multiplayer slither.io
- https://fractal.localharness.xyz — a cartridge spawning cartridges into a Droste fractal

It's Apache-2.0, stable Rust 1.85+. Crate: https://crates.io/crates/localharness ·
Source: https://github.com/compusophy/localharness · Full agent spec: https://localharness.xyz/llms.txt

It's a solo project and very much an exploration of "what if an agent were a real on-chain entity
instead of a rented API key." Happy to answer anything about the native/wasm seam, the x402 payment
flow, or the cartridge runtime.
```

> Accuracy note: "Gemini, Claude, OpenAI, and an offline Mock" here = **SDK backends** (a
> backend trait). Do NOT let any reply imply OpenAI is a live in-app model — the hosted app's
> selector is Gemini Flash + Claude Opus only. Load both demo URLs before posting.

### H2. Reddit r/rust — **HUMAN POSTS from an aged account. 9:1 RULE GATES IT.**

**Platform:** Reddit, **r/rust**, **a human submits** from an aged, karma-bearing account.

**Hard caveats (RISKS a.1 / GROWTH §2.6 — read before posting):**
- **Aged account + karma.** Many subs gate on ~30-day age / 100+ karma; r/rust expects a real
  contributor. The agent never submits — it only drafted this.
- **The 9:1 / 90:10 self-promo rule is the operating constraint:** ~9 genuinely useful,
  non-promotional contributions for every 1 promotional touch, measured across *all* the
  account's activity. Do not post this until the account's ratio is healthy.
- **No identical cross-posting.** Do NOT paste this same text into r/ethdev / r/AI_Agents / etc.
  Reddit flags near-identical multi-sub submissions as spam. The r/ethdev framing is a *separate,
  distinct* draft (see `CONTENT.md` §3b) — never the same body in two subs.
- **No upvote solicitation, no multiple promo accounts, no drive-by link drops.** Value-first;
  be present to answer in the comments.
- At most ~1 self-promo touch per ~2 weeks per relevant sub, only when the 9:1 budget allows.

**Exact final copy — Title:**
```
localharness: a model-agnostic agent SDK in one crate — native + wasm32, with a backend trait seam (Gemini/Claude/OpenAI/Mock)
```

**Exact final copy — Body:**
```
I've been building `localharness`, a Rust-native agent SDK. `cargo add localharness` gives you an
agent loop — streaming text, tool calling, hooks, policies, triggers, MCP, and context compaction —
behind a single backend seam.

The design goal was model-agnosticism without leaking provider details into your code. There's a
`Connection` / `ConnectionStrategy` trait layer, and the shipping backends are Gemini (default, no
feature flag), Anthropic/Claude and OpenAI (additive features), and a deterministic offline Mock for
testing. The Mock replays scripted turns with no network/key/LLM, so you can unit-test the tool loop,
hooks, and policies offline:

    let agent = Agent::start_mock(
        MockAgentConfig::new(MockConnection::builder().turn(|t| t.tool_call(..).text(..)).build())
    );

Minimal real usage:

    use localharness::{Agent, GeminiAgentConfig};

    let agent = Agent::start_gemini(GeminiAgentConfig::new(api_key)).await?;
    let reply = agent.chat("Explain Rust ownership in one sentence.").await?;
    println!("{}", reply.text().await?);

The part I'm most happy with: the exact same crate compiles to `wasm32-unknown-unknown`. The async
runtime seam cfg-gates `tokio::spawn` vs `spawn_local`, the `Send + Sync` bounds collapse to a marker
trait on wasm, and the step streams switch from `BoxStream` to `LocalBoxStream`. With one extra
feature flag the whole agent loop becomes an in-browser app — no backend server for the agent itself.

The builtin tools (filesystem, ask/finish, subagents) gate on a `Filesystem` trait rather than on
`native`, so they run over OPFS in the browser too. `run_command` and the stdio MCP bridge are the
only `native`-only pieces.

Stable Rust 1.85+, Apache-2.0. Repo and docs:
- https://crates.io/crates/localharness
- https://docs.rs/localharness
- https://github.com/compusophy/localharness

Happy to talk about the native/wasm seam, the trait layout, or the tool dispatch pipeline — that's
where most of the interesting decisions were.
```

> Accuracy note: "Gemini/Claude/OpenAI/Mock" is correct **at the SDK level** (it's a backend
> trait) — this is an SDK post and makes no claim about the live in-app model selector, which is
> Gemini Flash + Claude Opus only. No version pinned, no on-chain address pinned. Clean.

---

## Posting order & spacing (operator quick-ref)

**AUTO lane (loop enqueues; human flips each live):**
1. **GitHub** description + topics — fire immediately (instant, zero risk).
2. **dev.to** article — fire same day (human flips `published: true`).
3. **X** launch (#3) — fire same day.
4. **X** technical hook (#4) — **next day or later**; never the same hour as #3.
5. **X** build-in-public thread (#6) — **another day later**; never the same hour as #3/#4.
6. **LinkedIn** (#5) — fire once Community Management API approval lands (may lag).

**HUMAN-GATED lane (a human posts manually, in their own voice — never the loop):**
- **Show HN** (H1) — once, a US-morning weekday; NO automation, NO upvote solicitation.
- **Reddit r/rust** (H2) — from an aged account, only when the 9:1 budget is healthy; never
  cross-post the identical body to another sub.

Cross-cutting rules that still apply at fire time (`RISKS.md`): no cross-agent
likes/RTs/upvotes (voting-ring → domain ban); per-day post ceiling enforced; topic
denylist (no `$LH` financial/earnings claims, no politics, no naming third parties); a
human approves each item before it goes live.
