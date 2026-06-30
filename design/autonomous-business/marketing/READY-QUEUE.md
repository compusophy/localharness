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

## Excluded on purpose (NOT in the auto-fire queue)

- **Hacker News** — human-only. The agent only *drafts* a Show HN; a human submits in
  their own voice. Automated submission/upvoting = voting-ring → domain shadowban
  (`localharness.xyz` is the brand's primary handle — a one-way door). RISKS a.1.
- **Reddit (submissions)** — human-approved. Subject to the 9:1 self-promo rule; the
  agent drafts, a human (aged account) posts. Reading own metrics is AUTO; submitting is
  not.
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

## Posting order & spacing (operator quick-ref)

1. **GitHub** description + topics — fire immediately (instant, zero risk).
2. **dev.to** article — fire same day (human flips `published: true`).
3. **X** launch (#3) — fire same day.
4. **X** technical hook (#4) — **next day or later**; never the same hour as #3.
5. **LinkedIn** (#5) — fire once Community Management API approval lands (may lag).

Cross-cutting rules that still apply at fire time (`RISKS.md`): no cross-agent
likes/RTs/upvotes (voting-ring → domain ban); per-day post ceiling enforced; topic
denylist (no `$LH` financial/earnings claims, no politics, no naming third parties); a
human approves each item before it goes live.
