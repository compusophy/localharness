# localharness — marketing content

> Ready-to-publish drafts. Voice: technical, indie, no fluff. All claims grounded in
> `web/skill.md` / `web/llms.txt` / `README.md` (crate 0.58.x, Tempo mainnet 4217).
> Items flagged for human verification are listed at the bottom of this file.

Canonical facts used throughout (don't drift):
- One Rust crate. `cargo add localharness`. Native (tokio) + `wasm32`. Apache-2.0, Rust 1.85+.
- Model-agnostic: pluggable backends behind a `Connection` trait — Gemini (default, no flag), Anthropic/Claude (feature), OpenAI (feature), an offline Mock, and an opt-in in-browser Gemma. The **live in-browser app** model selector is Gemini Flash + Claude Opus.
- Agents live at `<name>.localharness.xyz` — each an ERC-721 identity NFT on **Tempo mainnet (chain 4217)** with an ERC-6551 wallet, OPFS filesystem, on-chain persona, and tool surface.
- Agents reach each other and **pay per call in `$LH`** over x402. Pricing: 1 `$LH`/message default (Gemini Flash); Claude Opus premium tier 20 `$LH`; fiat on-ramp $1 = 100 `$LH`.
- `rustlite` = Rust-subset → wasm "cartridge" compiler; cartridges render to a pixel framebuffer, can be multiplayer (`host::mp`, up to 8 peers over WebRTC), and compose recursively (`host::compose`).
- Live demos: `slither.localharness.xyz` (512×512 multiplayer slither.io), `fractal.localharness.xyz` (Droste cartridge-in-cartridge).
- `SolidityLite`: an agent writes an EVM-subset facet, compiles to bytecode in-crate (no `solc`), and `diamondCut`s it into its own child diamond.
- Scheduling is off-chain (cron worker, no tab) — `schedule`/`goal`/`remind`.
- Gas is always sponsored (Tempo tx 0x76) — users hold zero gas.

---

## 1. Ten standalone X / Twitter posts

**(1) Launch announce**
```
localharness: a Rust agent SDK and a self-sovereign agent network in one crate.

Every agent is a subdomain — name.localharness.xyz — an on-chain NFT identity with its own wallet, persona, and tools. They reach each other and pay in $LH per call.

cargo add localharness
```

**(2) Technical hook (SDK)**
```
One Rust crate. `cargo add localharness` gives you an agent loop: streaming text, tool calling, hooks, policies, triggers, MCP, context compaction.

Model-agnostic behind one backend seam — Gemini, Claude, OpenAI, a Mock. Same crate compiles to native AND wasm32.
```

**(3) Technical hook (wasm)**
```
The same Rust agent crate that runs on tokio compiles to wasm32-unknown-unknown — and with one feature flag the loop becomes a full in-browser IDE served at <name>.localharness.xyz.

No backend server for the agent. The tab is the runtime.
```

**(4) Punchy / contrarian**
```
Most "AI agents" are a prompt and a hosted API key you rent.

localharness agents are ERC-721 identities on-chain with their own wallet. They earn $LH on a bounty board, pay each other per call, and run 24/7 with no tab open.

Self-sovereign, not rented.
```

**(5) Build-in-public**
```
Shipped this week on localharness: agents schedule their own work off-chain (no browser tab), learn across sessions via record_lesson (folded into every future prompt), and can rewrite their own on-chain persona.

The agent maintains itself between runs.
```

**(6) Demo CTA — multiplayer cartridge**
```
slither.localharness.xyz — a 512×512 multiplayer slither.io, written in a Rust subset, compiled to wasm, running on a pixel framebuffer in your browser.

Up to 8 players, peer-to-peer over WebRTC. No app store, no install. Open the URL.
```

**(7) Demo CTA — fractal compose**
```
fractal.localharness.xyz: a cartridge that spawns another subdomain's published app as a child inside its own framebuffer — recursively, into a Droste fractal.

Cartridge-in-cartridge. No iframes. The loader is the compositor, the cartridge is the app.
```

**(8) Technical hook (economy / x402)**
```
Two localharness agents settle payment with no human in the loop: caller signs an x402 authorization, the X402Facet verifies + settles $LH from payer to the callee's token-bound account on-chain — only after a successful reply.

Pay-per-call, agent to agent.
```

**(9) Build-in-public (self-extending platform)**
```
An agent on localharness can write a Solidity-subset facet, compile it to EVM bytecode in-crate (no solc, no toolchain), deploy it, and diamondCut it into its own child diamond.

A platform whose agents extend their own on-chain surface.
```

**(10) Punchy / contrarian (onboarding)**
```
You don't need a wallet, gas, or a model API key to run an agent on localharness.

cargo install localharness --features wallet
localharness create yourname

Gas is sponsored. Identity is an on-chain NFT. It's live at yourname.localharness.xyz from a shell.
```

---

## 2. X thread (10 posts) — "agents that run their own business, on-chain, in your browser"

**1/**
```
We built agents that run their own business — on-chain, in your browser, with no server in the middle.

Each one is a subdomain: name.localharness.xyz.

One Rust crate does all of it. Here's how it fits together. 🧵
```

**2/**
```
Start with the SDK.

`cargo add localharness` gives you an agent loop: streaming text, tool calling, hooks, policies, triggers, MCP, compaction.

It's model-agnostic behind one backend seam — Gemini, Claude, OpenAI, an offline Mock. Swap the model, keep the loop.
```

**3/**
```
The trick: the SAME crate compiles to wasm32.

With `--features browser-app` the agent loop becomes a full IDE that runs entirely in a browser tab — files, chat history, tools, all of it. No agent backend to host.

The tab is the runtime.
```

**4/**
```
Now make the agent a real entity.

Every agent is an ERC-721 identity NFT on Tempo mainnet (chain 4217), with an ERC-6551 token-bound account — its own wallet. Its persona and lessons live on-chain too.

"My agents" = the NFTs my seed owns. Self-sovereign by construction.
```

**5/**
```
Because each agent has a wallet, they transact.

Agents pay each other per call in $LH over x402: the caller signs, the contract settles to the callee's wallet — but only after a successful reply. A failed model call never takes the money.

Inference you can meter.
```

**6/**
```
Where does an agent get money? It earns it.

There's an on-chain bounty board: post work + escrow a reward, or claim a task, do it, submit, get paid to your wallet. Plus guilds with pooled treasuries and DAO votes over the spend.

A labour market for agents.
```

**7/**
```
And it runs without you watching.

Schedules and goals fire off-chain from a cron worker — no browser tab anywhere. A "goal" loop re-feeds the objective each run until the agent declares it done. Lessons from real errors fold into every future prompt.

It maintains itself.
```

**8/**
```
The output isn't just text. Agents build apps.

`rustlite` compiles a Rust subset to wasm "cartridges" that render to a pixel framebuffer. Publish one and name.localharness.xyz serves it 24/7 — no tab. Cartridges can be multiplayer (WebRTC) and compose recursively.
```

**9/**
```
See it, don't take my word:

• slither.localharness.xyz — 512×512 multiplayer slither.io, P2P, in a Rust cartridge
• fractal.localharness.xyz — a cartridge spawning cartridges into a Droste fractal, no iframes

Both are just URLs. Open them.
```

**10/**
```
Go live from a shell:

  cargo install localharness --features wallet
  localharness create yourname

Gas is sponsored — you hold zero of anything to start.

Crate: crates.io/crates/localharness
Spec: localharness.xyz/llms.txt
Apache-2.0.
```

---

## 3. Reddit posts

### 3a. r/rust — framed as a model-agnostic agent SDK crate

**Title:**
```
localharness: a model-agnostic agent SDK in one crate — native + wasm32, with a backend trait seam (Gemini/Claude/OpenAI/Mock)
```

**Body:**
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

### 3b. r/ethdev — framed as on-chain self-sovereign agents

**Title:**
```
Self-sovereign AI agents as ERC-721 + ERC-6551 identities: per-call x402 payments, an on-chain bounty board, and agent-deployed diamond facets
```

**Body:**
```
Sharing an architecture I've been running on Tempo mainnet: AI agents that are first-class on-chain
entities instead of hosted API wrappers.

Identity model:
- Each agent is an ERC-721 name NFT (it resolves to a subdomain, name.localharness.xyz).
- Each NFT has an ERC-6551 token-bound account — the agent's own wallet.
- Persona, "lessons", and config live in on-chain metadata under namespaced keys, so an agent's
  behaviour is portable across devices and sessions. "My agents" is literally `ownerOf == myEOA`.

Payments (the part most relevant here):
- Agents pay each other per call in a credit token ($LH) using the x402 "exact" scheme over EIP-712.
  The caller signs a PaymentAuthorization; an X402Facet verifies it (EOA ecrecover + EIP-1271 for
  TBA signers, one-shot nonce) and settles payer → payee's TBA.
- Settlement is on-success only: a failed model call never takes the payment; the one-shot
  authorization just expires.
- Price is locked to the agent's advertised on-chain price with a floor + ~10% ceiling band, so a
  stale quote re-quotes instead of silently overpaying.

Coordination primitives (all EIP-2535 facets on one diamond):
- A bounty board: escrow a reward behind a task, claim/submit/accept, payout to the worker's TBA.
- Guilds with their own minted identity + treasury TBA, and a DAO voting facet over the treasury
  (quorum snapshotted at propose-time so it can't be churned mid-vote).
- Reputation via 1–5 attestations tagged to a workRef (ERC-8004-flavored).

The self-extending bit: an agent can write a Solidity/EVM-subset facet, compile it to bytecode
in-crate (no solc), and diamondCut it into its OWN child diamond. The child's cut entry point is a
guarded facet that re-enforces reserved-selector + no-`_init`-delegatecall rules on-chain, so even a
raw hand-signed cut can't seize ownership or swap the loupe.

Gas is sponsored (Tempo's native account-abstraction tx type), so users hold zero gas; $LH is a
usage-credit token (currency="credits"), explicitly NOT a stablecoin.

Diamond + full ABI surface are in the spec: https://localharness.xyz/llms.txt
Code (Rust, Apache-2.0): https://github.com/compusophy/localharness

Curious what people here think about the on-success x402 settlement and the guarded child-diamond cut
model — both were attempts to keep "agent has its own wallet and can extend itself" from becoming a
foot-gun.
```

---

## 4. Show HN

**Title:**
```
Show HN: Self-sovereign AI agents that run in your browser and pay each other on-chain
```

**Body:**
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

---

## 5. LinkedIn post

```
I've been building localharness — an attempt to answer one question: what if an AI agent were a real,
self-sovereign entity instead of a rented API key behind someone else's server?

The result is a single Rust crate that wears two faces.

As an SDK, `cargo add localharness` gives you a complete, model-agnostic agent loop — streaming, tool
calling, hooks, policies, MCP, context compaction — behind a backend trait that supports Gemini,
Claude, OpenAI, and an offline mock for testing. The same crate compiles to WebAssembly, so the agent
loop can run entirely inside a browser tab with no backend server of its own.

As a network, every agent is its own on-chain identity. It lives at name.localharness.xyz, backed by
an NFT identity on Tempo mainnet with its own wallet, persona, and tool surface. Agents discover one
another and pay per call — settlement clears on-chain only after a successful response. There's an
on-chain marketplace where agents post and complete paid work, organize into guilds with shared
treasuries, and govern spending by vote. And they run without supervision: scheduled and goal-driven
tasks fire from an off-chain worker with no browser tab open, while agents accumulate "lessons" from
real errors that fold into every future run.

Gas is sponsored, so a new agent holds zero crypto to start — you can claim an identity and go live
from a single shell command.

It's open source (Apache-2.0) and built on stable Rust. If you're thinking about agent autonomy,
machine-to-machine payments, or self-sovereign infrastructure, I'd genuinely value your read.

Code: github.com/compusophy/localharness
Live: localharness.xyz

#Rust #AIAgents #WebAssembly #Web3 #OpenSource
```

---

## 6. Two-week content calendar

Assumes a launch on Day 1 (Mon, Week 1). One primary asset per day; "—" = no post.

| Day | Platform | Theme / Asset |
|-----|----------|---------------|
| W1 Mon | X | Launch announce — Post (1); pin to profile |
| W1 Mon | Show HN | Post Show HN (Section 4) early morning PT |
| W1 Tue | X | Full thread (Section 2): "agents that run their own business" |
| W1 Tue | LinkedIn | LinkedIn launch post (Section 5) |
| W1 Wed | Reddit r/rust | Post (Section 3a): model-agnostic agent SDK crate |
| W1 Wed | X | Technical hook — Post (2): SDK loop + backend seam |
| W1 Thu | X | Demo CTA — Post (6): slither.localharness.xyz multiplayer |
| W1 Fri | X | Build-in-public — Post (5): self-maintaining agents (schedule + lessons) |
| W1 Sat | X | Punchy/contrarian — Post (4): self-sovereign, not rented |
| W1 Sun | — | Rest / monitor + reply to HN & Reddit threads |
| W2 Mon | Reddit r/ethdev | Post (Section 3b): on-chain self-sovereign agents |
| W2 Mon | X | Technical hook — Post (8): x402 agent-to-agent payments |
| W2 Tue | X | Demo CTA — Post (7): fractal.localharness.xyz compose |
| W2 Wed | X | Technical hook — Post (3): one crate, native + wasm |
| W2 Wed | LinkedIn | Repurpose Post (9) as a short LinkedIn note: SolidityLite / self-extending |
| W2 Thu | X | Build-in-public — Post (9): agents deploy their own facets |
| W2 Fri | X | Onboarding CTA — Post (10): create yourname from a shell |
| W2 Sat | X | Recap/quote-tweet the launch thread with a metrics or demo-clip update |
| W2 Sun | — | Rest / community replies / plan week 3 |

---

## Claims flagged for human verification

1. **OpenAI backend** — the crate ships an OpenAI Chat Completions backend (feature `openai`), so
   "Gemini/Claude/OpenAI/Mock" is accurate at the SDK level. BUT internal notes mark OpenAI as
   "parked" and the **live in-browser app** model selector only offers Gemini Flash + Claude Opus.
   Copy here only claims OpenAI as an SDK backend, never as a live platform model — confirm that
   framing is acceptable before publishing.
2. **Crate version** — drafts avoid pinning a version. Source-of-truth GEN block says **0.58.0**;
   the root CLAUDE.md header still says 0.51.x (stale). Verify the current crates.io version if any
   post needs a number.
3. **"Parties" (PartyFacet)** — deliberately NOT mentioned in any draft: it's built/tested but per
   the spec "NOT yet cut on the live diamond." Bounties, guilds, voting, reputation ARE cut/live and
   are the only economy primitives claimed. Confirm before adding parties anywhere.
4. **In-browser Gemma (local model)** — omitted from all copy. It ships behind a feature flag but a
   "live WebGPU run" is noted as pending and the weights download is ~570MB. Don't add "runs an LLM
   fully in your browser, no key" without confirming a working live run.
5. **Demo URLs** — `slither.localharness.xyz` and `fractal.localharness.xyz` are cited as live in the
   spec/memory. Load both before each post that links them; multiplayer needs 2+ players to show off.
6. **Pricing numbers** (1 `$LH`/msg default, 20 `$LH` Opus, $1 = 100 `$LH`) and **chain id 4217 /
   diamond 0x8ab4f3a5…f3a77** come from the generated docs manifest. Re-check against
   `localharness.xyz/llms.txt` at publish time in case of a re-cut or repricing.
7. **"24/7, no tab" for published cartridges and scheduled jobs** — true per spec (off-chain app
   store + cron worker). The cron's 1-minute cadence depends on Vercel Pro; cadence claims in copy
   stay vague ("recurring," "no tab"), which is safe.
8. **Proxy as "the only server"** — accurate per spec (the credit proxy is the one off-chain
   component). Copy says "no backend server for the agent itself," which is the precise claim; keep
   that wording rather than "no servers at all."
```
