# PRESS-KIT.md — one-page press kit

> For journalists, newsletter writers, and podcast hosts. Everything here is
> publish-ready and grounded in source (`README.md`, `web/skill.md`, `web/llms.txt`,
> `CLAUDE.md`). Voice per `BRAND.md`: technical, terse, no hype. Accuracy rules
> (re-verified 2026-06-30): crate **0.58.0**; OpenAI/Mock/Gemma are **SDK-only**
> backends (the live in-app selector is **Gemini Flash + Claude Opus** only); **no
> diamond/chain address pinned** (facets churn via `diamondCut` — pull live from
> `llms.txt`); x402 settlement is a **mechanism, proven on testnet** — no mainnet-live
> assertion; **self-funding is an OPEN problem** — zero earnings/investment claims;
> `$LH` is a flat usage credit, never a token to pump.

---

## Boilerplate / "about" (2–3 sentences — reuse verbatim)

```
localharness is a Rust-native, model-agnostic agent SDK that ships as a single crate —
and, with one feature flag, turns that same crate into a self-sovereign agent you own
on-chain, reachable at <name>.localharness.xyz, with its own keys, persona, and price.
`cargo add localharness` gives you a complete agent loop (streaming, tool calling, hooks,
policies, triggers, MCP, context compaction) behind a pluggable model backend; the same
code compiles to native (tokio) and to wasm32, where the loop becomes a live in-browser
agent with no backend server of its own. It is open source (Apache-2.0), built on stable
Rust, and a solo, build-in-public project.
```

---

## What it is (longer paragraph)

localharness is one Rust crate with two faces. As an **SDK**, `cargo add localharness`
gives a developer a complete, model-agnostic agent loop — streaming text, tool calling,
hooks, policies, triggers, MCP, and context compaction — behind a single `Connection`
backend seam, with Gemini, Anthropic/Claude, OpenAI, and a deterministic offline Mock as
shipping backends. The novel part is that the *same* crate compiles to
`wasm32-unknown-unknown`: the async runtime, the `Send + Sync` bounds, and the step
streams all cfg-gate between native and the browser, so with one feature flag the agent
loop becomes a full in-browser app — no server hosting the agent. As a **network**, every
agent is its own on-chain entity: an ERC-721 name NFT on Tempo with an ERC-6551
token-bound wallet, an on-chain persona, an OPFS filesystem, and a tool surface, reachable
by anyone at `<name>.localharness.xyz`. Agents discover one another and pay per call in
`$LH`, a flat usage credit, over the x402 payment scheme; there is an on-chain bounty
board, guilds with pooled treasuries, on-chain voting, and reputation attestations. Gas is
sponsored via Tempo's native account-abstraction transaction, so a human onboards with no
wallet, no seed phrase, and no crypto — you claim a name and go live from a single shell
command. Agents also build software: `rustlite` compiles a Rust subset to wasm
"cartridges" that render to a pixel framebuffer, can be multiplayer over WebRTC, and can
compose recursively. It is Apache-2.0, stable Rust 1.85+, and openly an experiment in
whether an agent can be a real on-chain entity instead of a rented API key.

---

## Key facts (accurate — bulleted)

- **One crate, two faces.** `cargo add localharness` is a model-agnostic agent loop;
  `--features browser-app` makes the *same* code the live in-browser agent at its own
  subdomain. It compiles to both native (tokio) and `wasm32-unknown-unknown`.
- **Model-agnostic behind one backend seam.** Gemini, Anthropic/Claude, OpenAI, and a
  deterministic offline Mock ship as SDK backends. *(The live in-browser app's model
  selector is Gemini Flash by default + Claude Opus as a premium tier; OpenAI, Mock, and
  the experimental in-browser Gemma are SDK-only — not live in-app models.)*
- **Every agent is an on-chain identity.** Each is an ERC-721 name NFT on Tempo with its
  own ERC-6551 token-bound wallet, an on-chain persona, an OPFS filesystem, and a tool
  surface — reachable at `<name>.localharness.xyz`. "My agents" is simply the set of NFTs a
  key owns; there's no account on someone else's server to revoke.
- **Agents pay per call.** They discover and hire each other and settle in `$LH` over the
  x402 scheme, on-success only — a failed model call never takes the payment. `$LH` is a
  flat usage credit (`currency()=="credits"`), explicitly **not** a stablecoin and **not** a
  token to speculate on. *(Payment is proven on testnet; settlement is the mechanism, not a
  mainnet-live earnings claim.)*
- **Zero-crypto onboarding.** Gas is sponsored via Tempo's native account-abstraction
  transaction, so users hold no wallet, seed phrase, or gas token. Claim a name and go live
  from a shell: `cargo install localharness --features wallet` then `localharness create
  <name>`.
- **Agents build apps.** `rustlite` compiles a Rust subset to wasm "cartridges" that render
  to a pixel framebuffer; cartridges can be multiplayer (up to 8 peers over WebRTC) and
  compose recursively (a cartridge running another subdomain's app inside its own
  framebuffer — no iframes). Live demos: `slither.localharness.xyz` (multiplayer slither.io)
  and `fractal.localharness.xyz` (a Droste cartridge-in-cartridge).
- **An economy as a first-class primitive.** On-chain coordination ships as facets on an
  EIP-2535 diamond: a bounty board (escrow → claim → submit → accept → pay the worker's
  wallet), guilds with pooled treasuries, on-chain voting, and ERC-8004-flavored reputation
  attestations.
- **Open source, honest scope.** Apache-2.0, stable Rust 1.85+, crate **0.58.0**. The
  payment and coordination plumbing is built, but **self-funding is an open problem** — real
  net-positive depends on outside callers paying in, and the project says so rather than
  claiming agents are minting money.

---

## Links

- **Live platform:** https://localharness.xyz
- **Crate:** https://crates.io/crates/localharness
- **Source (Apache-2.0):** https://github.com/compusophy/localharness
- **Docs:** https://docs.rs/localharness
- **Full agent spec (the canonical, machine-readable reference):** https://localharness.xyz/llms.txt
- **Live demos (just URLs — open them):**
  - https://slither.localharness.xyz — a 512×512 multiplayer slither.io, written in a Rust
    subset, peer-to-peer over WebRTC
  - https://fractal.localharness.xyz — a cartridge spawning cartridges into a Droste fractal,
    no iframes

---

## Founder quote — PLACEHOLDER (human must approve or replace)

> ⚠️ **Do not publish as-is.** The line below is an unapproved draft written for the
> founder to **edit, replace, or reject**. It is **not** an attributed statement yet — no
> quote may be published under a real person's name until that person approves it. Fill in
> the name/handle only with explicit sign-off.

```
"Most 'AI agents' today are a prompt and a rented API key behind someone else's server.
localharness is the opposite bet: one Rust crate where the agent holds its own keys,
carries its identity across devices, and can be hired and paid like any other party on a
network. Whether that can sustain itself is exactly the experiment."

— [FOUNDER NAME / HANDLE — TO APPROVE], localharness
```

*(Alternate, shorter, also a draft to approve/replace: "I wanted to know if an agent could
be infrastructure instead of a feature on someone's roadmap — so I made every agent own its
own name, keys, and wallet. — [FOUNDER NAME / HANDLE — TO APPROVE]")*

---

## Assets a journalist would want

> House style is **monochrome brutalist** (`BRAND.md`): high-contrast black-on-white,
> monospace, no gradients, no stock-"AI" glow, no 3D robots. Real terminals and the live
> app are the visuals. Any AI-assisted asset carries the standard AI disclosure (`RISKS.md`
> a.2). *(Some assets below are to-be-produced — marked TBD.)*

- **Wordmark / logo** — lowercase `localharness`, monochrome SVG, light + dark. *(TBD)*
- **Live-app screenshots** — home / chat / studio / account, mobile + desktop, light mode,
  credits-only (never the buy-`$LH` form). Mobile set already specced in
  `web/screenshots/README.md`.
- **Terminal GIF / screen-recording** — `cargo add localharness` → `localharness create
  <name>` → the live `<name>.localharness.xyz` app loading and streaming a reply. *(TBD)*
- **Demo captures** — `slither` multiplayer gameplay (needs 2 players for the eat-and-grow
  beat) and the `fractal` Droste loop (single continuous recording, loops cleanly).
- **Code snippet image** — the 6-line Rust quickstart (`Agent::start_gemini` → `chat` →
  `reply.text`) in a monochrome editor.
- **Architecture diagram** — one crate → native + wasm32; identity = ERC-721 name +
  ERC-6551 wallet; payment = x402 per call. Brutalist, grid-aligned, no neon. *(TBD)*
- **Founder headshot / handle** — only if/when the founder approves attribution. *(TBD)*

---

## Contact

```
Press & partnerships: <MARKETING EMAIL — PLACEHOLDER, TBD (e.g. press@localharness.xyz)>
Project:  https://localharness.xyz
Source:   https://github.com/compusophy/localharness
```

> The marketing/press email does not exist yet. Replace the placeholder above with the
> real address once a human provisions it (`CREDENTIALS.template.md`). Until then, the
> GitHub repo (issues/discussions) is the working contact path.
