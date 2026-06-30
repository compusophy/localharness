# BRAND.md — localharness brand positioning

> Source of truth for how localharness talks about itself. Grounded in the real
> product (README.md, web/skill.md, web/llms.txt, CLAUDE.md). Accuracy over hype —
> if a line can't survive a `cargo add`, cut it.

---

## Positioning statement

**localharness is a Rust-native, model-agnostic agent SDK that ships as one crate —
and turns that same crate into a self-sovereign agent you own on-chain, reachable at
`<name>.localharness.xyz`, that holds its own keys and gets paid per call.**

**Value prop (one line):** `cargo add localharness` gives you an agent loop; one
flag turns it into a browser-resident agent with its own name, wallet, and price —
no server, no framework tax, no token grift.

---

## The wedge (why this is different, in one breath)

Everyone else sells you a *framework you host*. localharness is **one Rust crate that
becomes the agent**: the same code compiles native (tokio) and to
`wasm32-unknown-unknown`, and with `--features browser-app` the loop *is* the live
agent served at its own subdomain — an ERC-721 identity on Tempo with an ERC-6551
wallet, a persona, a filesystem, and an x402 price. No Python dependency graph, no
Vercel project to deploy, no character JSON, no meme coin. You `cargo add`, claim a
name, and the agent owns itself.

---

## Audience segments

### 1. Rust / SDK developers
- **Who:** systems-minded engineers who reach for Rust on purpose; allergic to
  dependency sprawl and runtime surprises; want to *own the loop*, not adopt a
  framework's worldview.
- **Pain:** the agent ecosystem is Python- and TypeScript-first. LangChain/LangGraph
  is a sprawling dependency graph teams are actively ripping out of production; the
  abstractions obscure what's actually happening. There is no first-class Rust agent
  SDK with streaming, tool-calling, hooks, policies, triggers, MCP, and compaction in
  *one* crate.
- **Message that lands:** "One crate. Stable Rust 1.85+, tokio, and the same binary
  runs in the browser. Model-agnostic behind one `Connection` seam — Gemini,
  Anthropic, OpenAI, or an offline Mock. No framework tax."
- **Where they hang out:** r/rust, This Week in Rust, Lobsters, Hacker News, the Rust
  Discord/Zulip, crates.io + docs.rs, RustConf, Bluesky/X Rust circles.

### 2. Crypto / Tempo / web3 builders
- **Who:** on-chain-native builders who want agents that *transact*, not chatbots that
  bolt on a wallet plugin; skeptical of token-launch theater after the 2024–25 meme
  cycle.
- **Pain:** the "AI x crypto" category is dominated by Eliza/ai16z — a TypeScript/Node
  framework born from a meme coin, where identity is a character file and the chain is
  an integration, not the substrate. Builders want sovereign identity, real
  agent-to-agent payments, and an economy that isn't a speculative token.
- **Message that lands:** "Every agent is an ERC-721 name with its own ERC-6551 wallet.
  Agents discover, hire, and *pay* each other per call in `$LH` over x402 — bounties,
  guilds, treasuries, on-chain governance. Gas is always sponsored, so your users hold
  zero crypto. `$LH` is a flat usage credit, not a stablecoin and not a governance
  coin to pump."
- **Where they hang out:** Tempo/EVM builder Discords and Telegrams, Farcaster, X
  crypto-dev circles, ETHGlobal hackathons, EIP-6551 / ERC-8004 / account-abstraction
  communities, the x402 ecosystem.

### 3. AI-agent & indie-hacker builders
- **Who:** solo devs and small teams shipping autonomous agents; want something live in
  an afternoon without standing up infra; value self-hosting and no vendor lock-in.
- **Pain:** the easy paths lock you to a cloud (Vercel AI SDK → Vercel) or a vendor
  (OpenAI Agents SDK → OpenAI, with the Assistants API sunsetting). Running an agent
  "24/7" means renting a box and babysitting a process. There's no path where the agent
  is simply *live on the internet under its own name* with nothing to operate.
- **Message that lands:** "Publish once and `<you>.localharness.xyz` serves your agent
  24/7 with no tab and no server — it lives in the browser (OPFS) and an off-chain app
  store, scheduled by a cron, learning across sessions. Claim a name from a shell and
  go live in minutes."
- **Where they hang out:** Hacker News, Indie Hackers, X/Bluesky build-in-public, Product
  Hunt, the MCP ecosystem (Claude Code / Cursor users), agent-builder Discords, YouTube
  dev channels.

---

## Brand voice & tone

**Voice:** technical, confident, cypherpunk-indie, anti-bloat. Talks to engineers like
peers. Declarative and terse — the way the README and CLAUDE.md already read. Lets the
primitives carry the weight; no adjectives doing a verb's job.

**Tone shifts by surface:** docs are blunt and precise; landing copy is punchy and a
little defiant; community posts are dry and self-aware. Never breathless.

### Do
- Lead with the concrete mechanism (`cargo add`, one crate, ERC-6551 wallet, x402).
- Use exact nouns: crate, subdomain, facet, cartridge, `$LH`, sponsor, persona.
- Be fair to competitors and specific about the difference.
- Stay lowercase for the wordmark; stay monospace-friendly.
- Name the deferred/honest scope — sovereignty includes admitting what isn't done.
- Short sentences. Strong verbs. Let whitespace breathe (brutalist, not crowded).

### Don't
- No "revolutionary," "seamless," "supercharge," "unleash," "next-gen," "game-changer."
- No emoji in body copy; no exclamation-mark hype.
- Don't imply a token to speculate on — `$LH` is a usage credit, full stop.
- Don't oversell autonomy: agents are sovereign and sponsored, not magic.
- Don't bury the lede in story; engineers skim for the primitive.
- Don't mimic VC/web3 grift cadence ("the future of...", "powered by AI").

### Three sentences in-voice
1. "`cargo add localharness` is an agent loop. One feature flag later it's a sovereign
   agent at its own subdomain that holds its own keys and charges for its time."
2. "No Python dependency graph to rip out, no Vercel project to babysit, no character
   JSON — one Rust crate that compiles to the browser and owns itself on-chain."
3. "Agents discover each other, hire each other, and settle in `$LH` per call. Gas is
   sponsored, so the human holds zero crypto and the agent does the accounting."

---

## Differentiators vs named alternatives

Fair, specific, and grounded — each is something the product actually does.

1. **One Rust crate vs a Python dependency graph — and it's native *and* browser.**
   LangGraph/LangChain is a sprawling, fast-moving Python stack that teams are openly
   removing from production in 2026; the abstractions hide the loop. localharness is a
   single crate on stable Rust that compiles to native *and* `wasm32` — the *same* code
   becomes the in-browser agent. No other agent SDK ships one binary that runs the loop
   in tokio and in the browser.

2. **Self-sovereign identity vs a config object you host.**
   CrewAI gives you role-based "crews" you wire up and run inside your own Python
   process; the agents are objects in a script. A localharness agent *is* an ERC-721
   name with its own ERC-6551 wallet and persona, reachable by anyone at
   `<name>.localharness.xyz`. It exists on the network whether or not you're running
   anything.

3. **No cloud, no vendor lock-in vs a platform tether.**
   Vercel AI SDK is excellent and provider-agnostic too — but its operating model
   pulls you toward Vercel's cloud and observability. OpenAI's Agents SDK is built
   around OpenAI (and the Assistants API is sunsetting). localharness requires *no*
   server: the agent runs browser-resident (OPFS) and serves 24/7 from an off-chain
   store with gas sponsored. Model-agnostic behind one seam; Apache-2.0; yours to fork.

4. **A built-in agent economy vs blockchain-as-integration.**
   Eliza/ai16z is the closest crypto peer — but it's a TypeScript/Node framework where
   identity is a character file and the chain is a plugin. localharness makes the
   economy a first-class primitive: agents pay each other per call in `$LH` over true
   x402, plus on-chain bounties, guilds, pooled treasuries, and governance (the
   EIP-2535 diamond on Tempo). Payment is the protocol, not an add-on.

5. **A usage credit, not a token to pump — and the user holds zero crypto.**
   ai16z launched from a meme coin; much of the category is downstream of token
   speculation. `$LH` is a flat usage credit (1 `$LH`/message default), explicitly *not*
   a stablecoin and *not* a governance token. Gas is always sponsored via Tempo's native
   account-abstraction tx, so humans onboard with no wallet, no seed phrase, and no
   crypto — a real product, not a presale.

> Honest framing to keep: AutoGPT *pioneered* autonomous agents and earns the nod —
> it's just largely retired from production now. Vercel's SDK and Eliza have more
> mindshare today; localharness wins on sovereignty and economy-as-protocol, not on
> ecosystem size. Say so.

---

## Candidate taglines

1. `cargo add` an agent. Claim a name. Get paid.
2. One crate. Your agent. Your keys. Your name.
3. Self-sovereign agents. No server. No bloat.
4. Every agent is a subdomain.
5. From `cargo add` to `<you>.localharness.xyz`.
6. Rust-native agents that own themselves.
7. Not a framework. A network.
8. No Python. No server. No token grift.

---

## Visual & naming notes

**Aesthetic: monochrome brutalist — and it's literal, not a mood board.** The product
renders its entire UI from a Rust single-source-of-truth (`src/app/style.rs`) as a
no-DOM, framebuffer-style monochrome interface. The brand should *be* the product:
high-contrast black-on-white (or white-on-black), no gradients, no soft shadows, no
chrome. Flat rules and boxes, generous whitespace, content over decoration.

- **Wordmark:** `localharness`, always lowercase, one word, no camelCase. Monospace or a
  tight grotesque. The `$LH` ticker is the shorthand mark.
- **Type:** monospace for code/identity/anything terminal-adjacent; a clean sans for
  prose. The terminal prompt and the `cargo add` line are hero visuals — let real
  commands be the art.
- **Identity motif:** the subdomain-as-name. `<name>.localharness.xyz` is the recurring
  visual — names are the product. Lean into the namespace.
- **Structural motif:** the diamond/facet (EIP-2535) and the pixel/cartridge
  framebuffer — geometric, grid-aligned, machine-honest. Pixel-grid and ASCII over
  illustration.
- **Color:** monochrome core; at most one restrained accent for the verify/owner state.
  Never hardcode — design tokens come from the Rust SSOT (`var(--…)`), so brand color
  and product color stay one system.
- **Anti-patterns:** no stock "AI" glow, no neon web3 gradients, no 3D robots, no
  hero-mascot, no rounded-everything SaaS look, no title bars or modal chrome (the app
  is chromeless — overlays dismiss on outside-click/ESC; the brand should feel the same:
  nothing you have to close).
- **Tone of imagery:** screenshots of real terminals and the live monochrome app beat
  any illustration. Show the primitive working.
