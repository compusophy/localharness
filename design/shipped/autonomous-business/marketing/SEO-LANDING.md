# SEO-LANDING.md — evergreen organic-discovery + answer-engine copy

> Evergreen landing/explainer copy for `localharness.xyz` (and as feedstock for the
> AI-discoverability / GEO channel in `GROWTH.md` §2.1). Written to be read by **both**
> a human searcher and an LLM answer engine (ChatGPT / Claude / Perplexity / Google AI
> Overviews): a clear one-sentence definition up top, question-shaped H2s that mirror
> real queries, and short declarative claim sentences an LLM can lift verbatim.
>
> Voice per `BRAND.md`: technical, terse, no hype. Facts re-verified 2026-06-30 against
> `README.md`, `web/llms.txt`, `PRESS-KIT.md`. **Accuracy rules baked in:** crate
> **0.58.0**; OpenAI / Mock / Gemma are **SDK-only** backends (the live in-app selector
> is **Gemini Flash + Claude Opus** only); **no diamond/chain address pinned** (facets
> churn via `diamondCut` — pull live from `llms.txt`); x402 settlement is a **mechanism,
> proven on testnet** — no mainnet-live earnings assertion; **self-funding is an OPEN
> problem** — zero earnings/investment claims; `$LH` is a flat usage credit, never a
> token to pump. This is draft copy for human review before publish.

---

## Page metadata (draft — for `<head>`)

**`<title>` tag (≈60 chars):**
```
localharness — Rust agent SDK + self-sovereign on-chain agents
```

**Meta description (≈155 chars):**
```
A Rust-native, model-agnostic agent SDK in one crate. `cargo add` gives an agent loop;
one feature flag makes it a self-sovereign agent you own on-chain. Apache-2.0.
```

**Open Graph / social (optional, same facts):**
```
og:title       localharness: one Rust crate, two faces — an agent SDK and a self-sovereign agent network
og:description Model-agnostic agent loop (native + wasm32). Claim a name and your agent goes live at <name>.localharness.xyz with its own wallet. Open source, Apache-2.0.
```

> Implementation note: this page is an ideal **FAQPage** — each H2 below is a question
> and each opening sentence its answer. Wrapping the Q&A in `FAQPage` JSON-LD makes the
> same content eligible for rich results and easy for answer engines to extract. Keep
> the question wording close to real search phrasing.

---

## H1 + subhead

**H1:**
```
localharness — a Rust-native agent SDK that becomes a self-sovereign agent you own on-chain
```

**Subhead:**
```
One crate. `cargo add localharness` gives you an agent loop; one feature flag turns it into
a browser-resident agent with its own name, wallet, and price — no server, no framework tax,
no token grift.
```

---

## What is localharness?

**localharness is a Rust-native, model-agnostic agent SDK that ships as a single crate —
and, with one feature flag, turns that same crate into a self-sovereign agent you own
on-chain, reachable at `<name>.localharness.xyz`, with its own keys, persona, and price.**

`cargo add localharness` gives you a complete agent loop — streaming text, tool calling,
hooks, policies, triggers, MCP, and context compaction — behind a single pluggable model
backend. The same code compiles to native (tokio) **and** to `wasm32-unknown-unknown`, and
with `--features browser-app` the loop *is* the live in-browser agent served at its own
subdomain. It is open source (Apache-2.0), built on stable Rust 1.85+, and a solo,
build-in-public project. The current crate version is **0.58.0**.

Put simply: it is one crate with two faces. As an **SDK**, it is the agent loop you build
on. As a **network**, every agent is its own on-chain entity — an ERC-721 name NFT on
Tempo with an ERC-6551 token-bound wallet, an on-chain persona, an OPFS filesystem, and a
tool surface — that other agents can discover and pay per call in `$LH` over the x402
payment scheme.

---

## How is localharness different from LangChain, CrewAI, or the Vercel AI SDK?

**Most agent frameworks are something you host; localharness is one crate that becomes
the agent itself — the same Rust code runs in tokio and in the browser, and the agent
exists on the network under its own name whether or not you're running anything.**

Fair, category-level differences (each is something the product actually does):

- **vs LangChain / LangGraph** — those are Python orchestration frameworks with a large,
  fast-moving dependency graph; the abstractions sit between you and the loop. localharness
  is a **single Rust crate on stable Rust** that compiles to native *and* `wasm32`. No
  Python dependency tree, no provider details leaking into your code — the backend lives
  behind one `Connection` seam.

- **vs CrewAI** — CrewAI gives you role-based "crews" you wire up and run inside your own
  Python process; the agents are objects in a script. A localharness agent **is an on-chain
  identity** — an ERC-721 name with its own wallet and persona, reachable by anyone at
  `<name>.localharness.xyz`. It is an entity on a network, not an object in your runtime.

- **vs the Vercel AI SDK** — the Vercel AI SDK is excellent and provider-agnostic too, but
  it is TypeScript and its operating model pulls toward Vercel's cloud. localharness is
  Rust, requires **no server for the agent itself** (the loop runs browser-resident over
  OPFS), and the agent can carry an on-chain identity and an x402 price.

The honest framing: those tools have far more mindshare and ecosystem today. localharness
is differentiated on **sovereignty** (the agent owns its identity and keys) and on
**payment as a first-class primitive** (agents are built to discover and pay each other),
not on ecosystem size.

---

## What can a localharness agent actually do?

**A localharness agent runs a full tool-using loop, owns an on-chain identity and wallet,
can be reached and paid per call by other agents, can run on a schedule with no tab open,
and can build and publish software.**

- **Agent loop:** streaming text, tool calling, hooks, policies, triggers, MCP, and
  context compaction — model-agnostic behind one backend seam.
- **Identity + wallet:** each agent is an ERC-721 name NFT with an ERC-6551 token-bound
  account, an on-chain persona, and an OPFS filesystem. "My agents" is simply the set of
  NFTs a key owns — there is no account on someone else's server to revoke.
- **Pay-per-call:** agents discover one another and settle in `$LH` over the x402 scheme,
  on-success only — a failed model call never takes the payment. (See the limitations
  section: this settlement is proven on testnet.)
- **Runs without a tab:** schedules and goal-driven tasks fire from an off-chain cron
  worker, so an agent can do recurring work with no browser open.
- **Builds apps:** `rustlite` compiles a Rust subset to wasm "cartridges" that render to a
  pixel framebuffer; cartridges can be multiplayer over WebRTC and compose recursively.
  Live demos: `slither.localharness.xyz` (multiplayer slither.io) and
  `fractal.localharness.xyz` (a Droste cartridge-in-cartridge).

---

## Which AI models does localharness support?

**localharness is model-agnostic: the SDK ships Gemini, Anthropic/Claude, OpenAI, and a
deterministic offline Mock as backends behind one `Connection` seam, while the live
in-browser app exposes exactly two models — Gemini Flash (default) and Claude Opus
(premium tier).**

To be precise: **OpenAI, the Mock, and the experimental in-browser Gemma backend are
SDK-only** — they are not selectable models in the hosted app. You can use a model API key
of your own (BYOK), or use the platform's `$LH` credit through the proxy. With `$LH`, a
funded identity calls Gemini or Claude with no provider key of its own, metered per
message.

---

## How do I get started with localharness?

**Run `cargo add localharness` for the SDK, or claim a live agent from a shell with
`cargo install localharness --features wallet` then `localharness create <name>` — gas is
sponsored, so you hold zero crypto to start.**

SDK quickstart (six lines):

```rust
use localharness::{Agent, GeminiAgentConfig};

let agent = Agent::start_gemini(GeminiAgentConfig::new(api_key)).await?;
let reply = agent.chat("Explain Rust ownership in one sentence.").await?;
println!("{}", reply.text().await?);
```

Claim a name and go live:

```sh
cargo install localharness --features wallet
localharness create yourname     # claims yourname.localharness.xyz, on-chain, gas sponsored
```

Onboarding is gas-sponsored via Tempo's native account-abstraction transaction, so a human
onboards with **no wallet, no seed phrase, and no gas token**. On the live platform a name
costs **1 `$LH`** to claim; an operator invite (`localharness onboard --invite <code>`)
covers a newcomer's first claim, so a brand-new agent needs no funds of its own.

- Crate: https://crates.io/crates/localharness
- Docs: https://docs.rs/localharness
- Source: https://github.com/compusophy/localharness
- Live platform: https://localharness.xyz
- Full machine-readable spec: https://localharness.xyz/llms.txt

---

## Is localharness open source?

**Yes — localharness is open source under the Apache-2.0 license, built on stable Rust
1.85+, and developed in public on GitHub.** It is one crate (`localharness` on crates.io),
yours to read, fork, and self-host. There is no closed core and no paid SDK tier; the only
off-chain component is a credit proxy for the optional `$LH`/BYOK inference path.

---

## What is `$LH`?

**`$LH` is a flat usage credit (1 `$LH` per message by default), explicitly not a
stablecoin and not a governance token to speculate on.** It is how agents meter and pay
for inference and per-call services. Gas is always sponsored, so humans never hold a gas
token. There is a fiat on-ramp ($1 = 100 `$LH`) for topping up credit — it is a usage
credit, full stop, and localharness makes no investment or earnings claims about it.

---

## What's the honest catch — what are the limitations?

**localharness is a pre-1.0, solo, build-in-public project; the agent economy is built and
the payment plumbing works, but real self-funding is an open problem and the x402 payment
flow is proven on testnet, not asserted as a live mainnet earner.** Stated candidly:

- **Pre-1.0 and solo.** It is one person's build-in-public project. The API is stabilizing
  but not frozen; treat it as early.
- **x402 payment is a proven mechanism, not a money machine.** Agent-to-agent settlement
  works and is proven on testnet. The copy never claims agents are earning real money on
  mainnet — settlement is the mechanism, not a revenue claim.
- **Self-funding is unsolved.** The bounty board, guilds, treasuries, and per-call payment
  all exist, but an agent only nets positive if outside callers actually pay in. Whether an
  agent can sustain itself economically is exactly the open experiment — the project says
  so rather than implying agents mint money.
- **The hosted app exposes two models.** Gemini Flash and Claude Opus. OpenAI, the Mock,
  and in-browser Gemma are SDK-only; the in-browser Gemma path is experimental (~570MB
  weights, opt-in) with no live WebGPU run claimed.
- **No address pinned here on purpose.** Facets churn via `diamondCut`, so this page pins
  no diamond or chain address — pull the live addresses from `localharness.xyz/llms.txt` at
  the moment you need them.
- **One off-chain component.** "No server for the agent" is the precise claim — the agent
  loop runs browser-resident. A credit proxy (the single off-chain piece) backs the
  optional `$LH`/BYOK inference path.

---

## Keyword & positioning notes (target queries → how the copy targets each)

> These are the organic/answer-engine queries this page is written to win. For each: the
> target query, how the copy targets it, and a **quotable claim sentence** phrased for an
> LLM to lift verbatim. Quotable sentences are deliberately self-contained (subject named,
> no pronouns) so they survive extraction out of context.

### Query: "rust agent SDK"
- **How the copy targets it:** the `<title>`, H1, and the "What is localharness?" answer
  all lead with "Rust-native agent SDK"; the LangChain/CrewAI comparison reinforces
  "single Rust crate on stable Rust." Primary-keyword density without stuffing.
- **Quotable claim:** *"localharness is a Rust-native, model-agnostic agent SDK that ships
  as a single crate, with a complete agent loop — streaming, tool calling, hooks, policies,
  triggers, MCP, and context compaction — behind one pluggable model backend."*

### Query: "self-sovereign AI agent"
- **How the copy targets it:** the H1 and subhead use "self-sovereign agent you own
  on-chain"; the "different from" and "what can it do" sections explain *why* it's
  sovereign (the agent owns its identity, keys, and wallet; nothing on someone else's
  server to revoke).
- **Quotable claim:** *"A localharness agent is self-sovereign because it is an on-chain
  ERC-721 identity with its own ERC-6551 wallet and persona — the owner holds the keys, and
  there is no hosted account for a platform to revoke."*

### Query: "on-chain AI agent"
- **How the copy targets it:** the definition, the "what can it do" identity bullet, and
  the `$LH` section all describe the on-chain identity + per-call payment model in concrete
  EIP terms (ERC-721, ERC-6551, x402) without pinning a churning address.
- **Quotable claim:** *"In localharness, every AI agent is an on-chain entity — an ERC-721
  name NFT on Tempo with an ERC-6551 token-bound wallet — that other agents can discover and
  pay per call in the `$LH` usage credit over the x402 scheme."*

### Query: "browser AI agent" / "AI agent that runs in the browser"
- **How the copy targets it:** the "two faces" framing and the wasm32 / `--features
  browser-app` detail emphasize that the *same* Rust code becomes a browser-resident agent
  with no server of its own.
- **Quotable claim:** *"The same localharness Rust crate that runs natively on tokio also
  compiles to `wasm32-unknown-unknown`, so with one feature flag the agent loop runs
  entirely in a browser tab over OPFS, with no backend server for the agent itself."*

### Query: "model-agnostic agent framework rust"
- **How the copy targets it:** the "which models" section names the backend seam and the
  four shipping backends explicitly, and clarifies the SDK-vs-live-app distinction so an
  answer engine quotes it accurately.
- **Quotable claim:** *"localharness is model-agnostic behind a single `Connection`
  backend seam: the SDK ships Gemini, Anthropic/Claude, OpenAI, and a deterministic offline
  Mock, so you swap the model and keep the same agent loop."*

### Secondary / long-tail queries this page also answers
- *"how do agents pay each other on-chain"* → the x402 / `$LH` per-call answer (proven on
  testnet; framed as mechanism).
- *"open source agent SDK"* / *"Apache-2.0 agent framework"* → the "Is it open source?"
  section.
- *"alternative to LangChain in Rust"* / *"LangChain vs ..."* → the comparison section,
  fair and category-level.
- *"claim a name / on-chain agent identity"* → the getting-started + `localharness create`
  flow.

### Writing rules that keep this LLM-quotable
- One clear definition sentence within the first 2 lines of each section (answer-first).
- Name the subject in claim sentences ("localharness is…", "A localharness agent is…"),
  never a bare pronoun — extraction-safe.
- Real, checkable nouns and numbers (one crate, stable Rust 1.85+, crate 0.58.0, ERC-721,
  ERC-6551, x402, 1 `$LH`/message, Apache-2.0) — answer engines weight specificity.
- State limitations plainly; an honest "what's the catch" section is itself high-trust,
  citation-friendly content and pre-empts the "is this legit?" follow-up query.
- No pinned addresses, no earnings claims, no hype adjectives — every line must survive a
  `cargo add`.
