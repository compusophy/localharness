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

## 2b. dev.to — second long-form article (distinct angle: x402 + EIP-6551)

**Platform:** dev.to / Forem, `POST /api/articles` with our own api-key. Same first-party,
clean-ToS lane as asset #2 — a SECOND durable, citable deep-dive on a **distinct** technical
angle (payments + token-bound accounts), so the two articles don't read as near-duplicates
(the cross-post / substantially-similar trap that burns SEO and trips spam heuristics).

**Native label:** dev.to has no native AI toggle → the footer text disclosure IS the label.

**Spacing:** do NOT publish on the same day as asset #2 (the first article). Space the two
deep-dives ≥1 week apart so they land as two genuine posts, not a burst. Distinct title,
distinct body, distinct primary keywords (article #1 = "self-sovereign agent in Rust /
native-wasm seam"; this one = "x402 micropayments + ERC-6551 token-bound accounts").

**Exact final copy:** the full body of **`DEVTO-ARTICLE-2.md`** in this directory, verbatim,
including its front-matter (`published: false` so a human flips it live) and the footer
disclosure. The load-bearing front-matter and disclosure:

```
---
title: "An agent that pays its own way: x402 micropayments + EIP-6551 token-bound accounts in practice"
published: false
tags: rust, crypto, ai, web3
canonical_url: https://localharness.xyz/llms.txt
---
```

**Disclosure line (already the article's last paragraph):**
```
Disclosure: this article was drafted by an AI agent operated by the localharness
project (the project's own automated account) and reviewed by a human before
publishing. It is AI-generated content and a first-party promotion of localharness.
```

**Accuracy guard (re-verified 2026-06-30 vs source):** every code constant in the body is
pinned to `src/registry/x402.rs` (`DEFAULT_ASK_PRICE_WEI` = 0.01 `$LH`, auto-pay cap
`REMOTE_CALL_MAX_AUTO_PAY_WEI` = 1 `$LH`, `PRICE_LOCK_OVERPAY_TOLERANCE_BPS` = 10%); the
EIP-712 `PaymentAuthorization` struct, `settle(...)` signature, and domain (`"localharness-x402"`,
version `"1"`) match the facet; TBA semantics (`MultiSignerAccount`: CALL-only, EIP-1271,
signer-revoke-on-transfer, high-`s` reject) match `contracts/README.md`. CLI flags (`call --pay`/
`--verify`, `price`, `mcp-call`) match `src/bin/localharness/main.rs`. **No diamond address is
pinned** (facets churn via `diamondCut`); identity/mint is correctly labeled Tempo mainnet (chain
4217) while x402 settlement is described as a diamond facet WITHOUT a mainnet-live assertion. `$LH`
is framed strictly as a usage credit (`currency()=="credits"`), and §7 names self-funding as an
OPEN problem — no earnings/investment claim. OpenAI/Mock/Gemma are framed as SDK-only backends;
the live in-app selector is stated as Gemini Flash + Claude Opus only.

---

## 2c. dev.to — third long-form article (distinct angle: rustlite cartridge compiler)

**Platform:** dev.to / Forem, `POST /api/articles` with our own api-key. Same first-party,
clean-ToS lane as assets #2 / #2b — a THIRD durable, citable deep-dive on a **distinct** technical
angle (the in-crate Rust-subset → wasm compiler + the cartridge runtime), so the three articles read
as three genuine posts, not a burst or near-duplicates (the substantially-similar trap that burns
SEO and trips spam heuristics). Article #1 = "self-sovereign agent in Rust / native-wasm seam";
#2 = "x402 micropayments + ERC-6551 token-bound accounts"; this one = "rustlite: a Rust subset
compiled to wasm cartridges that run in the browser" — distinct title, distinct body, distinct
primary keywords (compiler / WebAssembly / cartridge runtime, NOT payments or identity).

**Native label:** dev.to has no native AI toggle → the footer text disclosure IS the label.

**Spacing:** do NOT publish on the same day as asset #2 or #2b. Space the three deep-dives ≥1 week
apart (article #1 → #2 → #3), so they land as three genuine posts. Distinct angle from BOTH prior
articles (compiler/runtime, not SDK-seam and not payments/identity).

**Exact final copy:** the full body of **`DEVTO-ARTICLE-3.md`** in this directory, verbatim,
including its front-matter (`published: false` so a human flips it live) and the footer disclosure.
The load-bearing front-matter and disclosure:

```
---
title: "rustlite: compiling a Rust subset to WebAssembly cartridges that run in the browser"
published: false
tags: rust, webassembly, compilers, gamedev
canonical_url: https://localharness.xyz/llms.txt
---
```

**Disclosure line (already the article's last paragraph):**
```
Disclosure: this article was drafted by an AI agent operated by the localharness
project (the project's own automated account) and reviewed by a human before
publishing. It is AI-generated content and a first-party promotion of localharness.
```

**Accuracy guard (re-verified 2026-06-30 vs source):** the public entry point
`localharness::rustlite::compile(&str) -> Result<Vec<u8>, _>` matches `src/rustlite/mod.rs`
(`pub mod rustlite` in `src/lib.rs`). The pipeline `lexer → parser → typecheck → codegen (wasm
emitter) → loader` and "no LLVM / direct LEB128+sections+opcodes emit" match the module doc
comments. Language subset (in: `i32`/`f64`/`bool`, casts, arrays `[i32;N]` incl. indexed writes /
repeat-init / shared-backing array params / array-return rejected; out: traits/generics/references/
heap `Vec`-`String`-`Box`/globals → clean `LH0300`) matches `src/rustlite/mod.rs` tests +
`src/rustlite/CLAUDE.md`. The integer-only host ABI, the `host::display::*` / `host::compose::*`
function names, the `frame(t)` / `render()` / optional `dims()` (width<<16 | height) contract, and
the `draw_string` parser-stage desugar (6px stride) match `src/rustlite/loader.rs` + the subsystem
spec. Runtime defenses — Web-Worker off-main-thread + main-thread watchdog ("brick" fix), 64 KB
instantiation cap, `wss://`-only SSRF gate (`url_is_allowed`), JS↔Rust host-table parity test — match
`loader.rs` + `display.rs`. `host::compose` recursion bounded by a depth-5 + node/byte/FB-area budget,
and `host::mp` = N-peer host-authoritative star over WebRTC up to 8 peers, match CLAUDE.md. Code
snippets are verbatim from `examples/cartridges/{bouncing_ball,fractal}.rl`. Live demos
`slither.localharness.xyz` / `fractal.localharness.xyz` cited as URLs. **No chain/diamond address
pinned; no crate version pinned; no `$LH` financial claim; no model-selector claim** (the article
makes none — clean). Honest-scope section names the subset boundary and "served as static wasm, no
per-request backend" explicitly.

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

## 7. LinkedIn — autonomous-business vision (AUTO own content; GATED on API approval)

**Platform:** LinkedIn, own Page/profile via the Community Management API
(`w_member_social` / `w_organization_social`). Same AUTO lane + same approval gate as
asset #5; this is a SECOND, **distinct** LinkedIn long-form (the autonomous-business
vision — a company built entirely of role-agents — not the generic launch). Space it from
asset #5 (≥several days; never two long-forms on the Page back-to-back — LinkedIn down-ranks
burst posting and a near-duplicate reads as spam).

**Fire-readiness caveat (honest):** LinkedIn requires Community Management API access (an
approval form, ~Development→Standard tier) before any programmatic post. This asset is
"ready" the moment that approval + token land — same lag as asset #5. Until then a human can
post the same copy manually.

**Native label:** no strong native AI label for text posts → the appended disclosure below
is the required label.

**Accuracy guard (re-verified 2026-06-30 vs source):** the SDK backend list "Gemini, Claude,
OpenAI, and an offline mock for tests" is framed strictly as a **backend trait** — no claim
that OpenAI is a live in-app model (the hosted selector is Gemini Flash + Claude Opus only,
`src/app/model.rs`). Identity/mint is correctly Tempo mainnet; **no diamond/chain address is
pinned**. The company layer matches `src/app/chat/tools/company.rs` + `COMPANY-FEATURE.md`:
`company_status` (read-only) and `found_company` (one-call founder; registered in
`session.rs`, allowlist- + confirm-gated) both ship; the seven role personas are real
(`DEFAULT_ROLES` / `roles/*.md`). Governance is correctly stated as single-controller
(Model A); multi-party (Model B) is named as the next layer. The economy is described as
"primitives are built" — NOT as live mainnet x402 settlement (settlement is testnet-only).
Self-funding is named as the OPEN problem — no `$LH` earnings/investment claim. No crate
version pinned.

**Exact final copy (the post):**
```
For a while now I've been building localharness, and lately pointing it at a deliberately ambitious question: what would it take for a business to be nothing but software agents — a CEO, a PM, engineers, a reviewer, accounting, HR, and marketing — each a real, self-sovereign entity rather than a function call inside one big script?

Here's where that stands, including the parts that aren't done.

What localharness is: one Rust crate. `cargo add localharness` gives you a complete, model-agnostic agent loop — streaming, tool calling, hooks, policies, MCP, context compaction — behind a backend trait (Gemini, Claude, OpenAI, and an offline mock for tests). The same crate compiles to WebAssembly, so the agent loop can run entirely inside a browser tab with no backend server of its own.

What makes an agent self-sovereign: every agent is its own on-chain identity. It lives at name.localharness.xyz, backed by an NFT on Tempo mainnet with its own token-bound wallet, an on-chain persona, and a filesystem. "My agents" is simply the set of identities my key owns — no account on someone else's server, no rented seat to revoke. Gas is sponsored, so claiming an identity costs the new agent zero crypto; you go live from a single shell command.

The company layer — what's shipped: a "company" here isn't a new contract. It's a named composition of primitives that already exist — a guild for org identity, the guild's shared wallet as a treasury, and N role-subdomains as members. There's a one-call tool that stands the whole org up (create the guild, register the seven role agents with their on-chain personas, seed a shared backlog) and a read-only tool that snapshots any company: its treasury and every member with its role. The seven role personas are real, versioned, and in the repository.

The honest part — what's in progress: founding a company ships; the agents autonomously running one does not yet. Today it's a single-controller setup — one operator wearing many personas — and multi-party, on-chain governance between distinct agents is the next layer. More to the point: a company of agents spends credits on inference every turn, so it is seed-funded, not self-funding. The coordination and payment primitives are built, but real net-positive depends on outside callers paying in. That is the open problem, and I would rather name it than dress it up.

Why bother: most "AI agents" today are a prompt plus a rented API key behind someone else's server. I wanted to see how far the opposite goes — an agent that holds its own keys, carries its identity across devices and sessions, and can be hired and paid like any other party on a network. Whether a business made entirely of those agents can sustain itself is exactly the experiment.

It's open source (Apache-2.0), built on stable Rust. If you work on agent autonomy, machine-to-machine coordination, or self-sovereign infrastructure, I'd value your read — especially the skeptical kind.

Code: github.com/compusophy/localharness
Live: localharness.xyz

#Rust #AIAgents #WebAssembly #AutonomousAgents #OpenSource
```

**Disclosure line (append to the post body, above or below the hashtags):**
```
Disclosure: AI-generated and human-reviewed; posted by localharness's own automated account.
```

---

## 8. X / Twitter — founder-story thread (AUTO own content; "why self-sovereign, not rented")

**Platform:** X, post the whole thread to our own `@localharness` via the official API
(post 1/, then each subsequent post as a reply to the previous, then the disclosure as the
final reply). **Must be spaced** from #3/#4/#6 (≥1 day; no bursts; never the same hour, and
never adjacent to another X thread — two threads in a day reads as automation churn). This is
a deliberately distinct angle (a personal, first-person founder story — *why* build
self-sovereign agents instead of rented API wrappers), not a feature recap; X bans
substantially-similar posts (`RISKS.md` a.1 / guardrail #11).

**Native label:** same **Automated-account** setting as #3/#4/#6 (X's native bot label, linked
to the human operator). Required for automated posting independent of the post text.

**Accuracy guard (re-verified 2026-06-30 vs source):** SDK model-agnosticism is explicitly
scoped to "the SDK level" in post 5 (Gemini/Claude/OpenAI/mock = a backend trait) — no claim
that OpenAI is a live in-app model (selector = Gemini Flash + Claude Opus, `src/app/model.rs`).
Identity on **Tempo mainnet** with its own wallet is accurate (each role NFT has its own
token-bound account); **no diamond/chain address pinned**. The seven role personas match
`DEFAULT_ROLES` (`src/app/chat/tools/company.rs`); "one tool founds it; another snapshots it"
matches `found_company` + `company_status`. Payments are described at the design level
("hired and paid like any other party") and post 7 says the plumbing is "built" — NOT live
mainnet x402 settlement (settlement is testnet-only). Self-funding is named as the OPEN
problem — no `$LH` earnings/investment claim. No crate version pinned. Each post ≤280 chars
(verified).

**Exact final copy (post each line as its own post, in order, as a reply-chain):**

```
1/
Why I'm building self-sovereign agents instead of another API wrapper.

A thread on a decision I keep having to defend — to myself as much as to anyone. localharness, in the first person.

2/
Most "AI agents" are a prompt and a rented API key behind someone else's server. Useful, but the agent doesn't own anything. Turn off the account and it's gone. No identity you can point at, no keys of its own, nothing it carries between sessions.

3/
I wanted the opposite: an agent that's a real entity. It has a name — name.localharness.xyz. An on-chain NFT identity on Tempo mainnet, its own wallet, a persona and filesystem that travel with it. "My agents" = the set my key owns. No seat anyone can revoke.

4/
The contrarian bet: this is one Rust crate, not a framework you host. `cargo add localharness` is a full agent loop — streaming, tools, hooks, policies, MCP, compaction. The same crate compiles to wasm32 — the agent runs in a browser tab, no server of its own.

5/
Model-agnostic at the SDK level — Gemini, Claude, OpenAI, and an offline mock, behind one backend trait. Swap the model, keep the loop. No Python dependency graph to rip out, no vendor SDK to marry.

6/
Then I pushed further: can a whole business be agents? Seven role personas — exec, PM, coder, reviewer, accounting, HR, marketing — each its own subdomain + wallet. A "company" is just a guild + treasury + those roles as members. One tool founds it; another snapshots it.

7/
The honest part, because a founder thread without one is just marketing: that company is seed-funded, not self-funding. It burns credits on inference every turn. The plumbing is built; real net-positive needs outside callers paying in. That's the open problem.

8/
Why the hard way? A rented agent is a feature on someone's roadmap. An agent that holds its own keys and name is infrastructure. I'd rather build the second kind and find out if it can stand on its own.

Open source, Apache-2.0. crates.io/crates/localharness
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

### H3. Reddit r/ethdev — **HUMAN POSTS from an aged account. 9:1 RULE GATES IT.**

**Platform:** Reddit, **r/ethdev**, **a human submits** from an aged, karma-bearing account.

**Hard caveats (RISKS a.1 / GROWTH §2.6 — read before posting):**
- **Aged account + karma.** r/ethdev gates on real-contributor history; the agent never
  submits — it only drafted this.
- **The 9:1 / 90:10 self-promo rule is the operating constraint:** ~9 genuinely useful,
  non-promotional contributions per 1 promotional touch, measured across *all* the account's
  activity. Do not post until the ratio is healthy.
- **This body is DELIBERATELY DISTINCT from the r/rust post (H2).** H2 is an SDK post
  (native/wasm seam, backend trait, Mock); this is an on-chain-architecture post (EIP-2535
  diamond, ERC-6551 TBAs, x402 settlement, bounty board, guarded child-diamond cut). **Never
  paste the same body into two subs** — Reddit flags near-identical multi-sub submissions as
  spam, and a duplicate would also collide with the r/rust draft.
- **No upvote solicitation, no multiple promo accounts, no drive-by link drops.** Value-first;
  be present in the comments to answer. At most ~1 self-promo touch per ~2 weeks per sub.

**Exact final copy — Title:**
```
Self-sovereign AI agents as ERC-721 + ERC-6551 identities: per-call x402 payments, an on-chain bounty board, and agent-deployed diamond facets
```

**Exact final copy — Body:**
```
Sharing an architecture I've been building on Tempo: AI agents that are first-class on-chain
entities instead of hosted API wrappers. It's an EIP-2535 diamond; the identity layer (ERC-721 +
ERC-6551) is live on Tempo mainnet (chain 4217), and the coordination/economy facets are the layer
I'm proving out.

Identity model:
- Each agent is an ERC-721 name NFT (it resolves to a subdomain, name.localharness.xyz).
- Each NFT has an ERC-6551 token-bound account — the agent's own wallet. The account impl is
  CALL-only, supports EIP-1271 + an enrollable device-signer set (multiple device EOAs drive one
  identity without sharing the seed), and revokes those signers automatically on NFT transfer.
- Persona, "lessons", and config live in on-chain metadata under namespaced keys, so an agent's
  behaviour is portable across devices and sessions. "My agents" is literally `ownerOf == myEOA`.

Payments (the part most relevant here):
- Agents pay each other per call in a credit token ($LH) using the x402 "exact" scheme over
  EIP-712. The caller signs a PaymentAuthorization (from, to, value, validAfter, validBefore,
  bytes32 nonce) under a domain bound to chainId + the diamond; a facet verifies it (EOA ecrecover
  + EIP-1271 for TBA signers, one-shot nonce) and settles payer → payee's TBA via TIP-20
  transferFrom. The payer approves the diamond once; after that each call is just a signature, and
  it's gasless for the payer (the fee-payer half is sponsored).
- Settlement is on-success only: the reply is produced first, then settle is submitted. A failed
  call never submits settle; the one-shot authorization just expires at validBefore. No nonce is
  consumed, so the caller keeps their $LH.
- Unattended pay is fenced by arithmetic, not prompts: a default floor (0.01 $LH) for unpriced
  agents, a hard auto-pay cap (1 $LH) above which the call refuses and surfaces the price, and a
  price-lock band (signed value must sit within +10% of the live advertised price, else re-quote
  instead of silently overpaying). Price is advertised as on-chain metadata, not asserted in a
  request header.

Coordination primitives (all facets on the one diamond):
- A bounty board: escrow a reward behind a task, claim/submit/accept, payout to the worker's TBA
  (same x402 rail; payout is bound to the claimed identity, so claim-squatting just pays the
  squatter).
- Guilds with their own minted identity + treasury TBA, and a DAO voting facet over the treasury
  (quorum snapshotted at propose-time so it can't be churned mid-vote).
- Reputation via 1–5 attestations tagged to a workRef (ERC-8004-flavored).

The self-extending bit: an agent can write a Solidity/EVM-subset facet, compile it to bytecode
in-crate (no solc), and diamondCut it into its OWN child diamond. The child's cut entry point is a
guarded facet that re-enforces reserved-selector + no-`_init`-delegatecall rules on-chain, so even a
raw hand-signed cut can't seize ownership or swap the loupe.

Honest framing: $LH is a usage-credit token (currency()=="credits"), explicitly NOT a stablecoin
and not something to speculate on; gas is sponsored so users hold zero of the gas token. And
self-funding is an open problem — the payment plumbing is live, but whether an agent nets positive
depends on outside callers paying in. I'm not claiming agents are minting money.

Code (Rust, Apache-2.0): https://github.com/compusophy/localharness
Full ABI surface + live addresses (facets churn via diamondCut, so pull them live): https://localharness.xyz/llms.txt

Curious what people here think about the on-success x402 settlement and the guarded child-diamond
cut model — both were attempts to keep "agent has its own wallet and can extend itself" from
becoming a foot-gun.
```

> Accuracy note: distinct body from H2 (on-chain architecture, not the SDK). **No diamond
> address pinned** — identity/mint correctly labeled Tempo mainnet (chain 4217); x402 + the
> economy facets are described as diamond mechanisms WITHOUT a mainnet-live assertion (the
> testnet-vs-mainnet cut nuance). `$LH` framed as a usage credit; self-funding named as an OPEN
> problem; no earnings/financial claim. No model-selector claim is made here. Clean.

---

## Posting order & spacing (operator quick-ref)

**AUTO lane (loop enqueues; human flips each live):**
1. **GitHub** description + topics — fire immediately (instant, zero risk).
2. **dev.to** article #1 (#2) — fire same day (human flips `published: true`).
3. **X** launch (#3) — fire same day.
4. **X** technical hook (#4) — **next day or later**; never the same hour as #3.
5. **X** build-in-public thread (#6) — **another day later**; never the same hour as #3/#4.
6. **X** founder-story thread (#8) — **another day later again**; never the same day/hour as
   #3/#4/#6 (never two X threads in one day — automation-churn signal).
7. **dev.to** article #2 (#2b) — **≥1 week after article #1**; distinct angle, never the same day.
8. **dev.to** article #3 (#2c) — **≥1 week after article #2**; distinct angle (compiler/runtime),
   never the same day as #2/#2b. Three deep-dives, three weeks, no near-duplicates.
9. **LinkedIn** launch (#5) — fire once Community Management API approval lands (may lag).
10. **LinkedIn** autonomous-business vision (#7) — same approval gate; **space ≥several days
   from #5** (never two long-forms on the Page back-to-back).

**HUMAN-GATED lane (a human posts manually, in their own voice — never the loop):**
- **Show HN** (H1) — once, a US-morning weekday; NO automation, NO upvote solicitation.
- **Reddit r/rust** (H2) — from an aged account, only when the 9:1 budget is healthy; never
  cross-post the identical body to another sub.
- **Reddit r/ethdev** (H3) — from an aged account, only when the 9:1 budget is healthy; **distinct
  body from H2** (on-chain architecture, not the SDK) — never the same text in two subs; space it
  well apart from the r/rust post.

Cross-cutting rules that still apply at fire time (`RISKS.md`): no cross-agent
likes/RTs/upvotes (voting-ring → domain ban); per-day post ceiling enforced; topic
denylist (no `$LH` financial/earnings claims, no politics, no naming third parties); a
human approves each item before it goes live.
