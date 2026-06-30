# AUDIENCE-INTEL.md — market intelligence (2026)

> Genuine market intelligence, not content. WHERE the three audience segments
> actually gather, WHAT lands vs. what falls flat, the objections to preempt, and a
> fair, current competitive map. Companion to `BRAND.md` (positioning), `GROWTH.md`
> (channel ops), `CONTENT.md` (drafts). Built from a 2026 web sweep (sources at the
> bottom); refresh quarterly — this is a perishable map.
>
> **Accuracy basis (don't drift):** crate **0.58.0**, Apache-2.0, Rust 1.85+.
> SDK backends Gemini/Anthropic/OpenAI/Mock + experimental in-browser Gemma are
> **SDK-only**; the live in-app model selector is just Gemini Flash + Claude Opus.
> The **x402 payment rail is on testnet for localharness** (the protocol itself is
> mainnet-live elsewhere — see §2). **Self-funding is an OPEN, unproven goal — make
> no earnings/autonomy claims.** No chain/diamond address pinned anywhere.

---

## 0. The one macro shift that frames all three segments

2026 is the year the **AI-agent token cycle broke and agent *payments* got
legitimized.** Two facts do most of the positioning work:

1. **Token-launch agent theater is now openly derided.** The flagship of the meta —
   ai16z / elizaOS — collapsed ~-99.9% from its Jan-2025 peak and is facing a
   **proposed class action (SDNY, filed ~Apr 20 2026)** alleging the "autonomous
   agent" was human-operated and investors were misled (~$2.6B alleged). On-chain
   investigator ZachXBT called ~99% of AI-agent tokens scams; Cointelegraph's framing
   is "memecoins that talk." Web3 AI-agents are ~3% of the real agent market.
2. **Agent payments became serious infrastructure.** Coinbase's **x402** protocol was
   donated to a **Linux Foundation–hosted x402 Foundation** with Stripe, Cloudflare,
   AWS, Google, Microsoft, Visa, Mastercard, Circle, Anthropic, and Vercel backing;
   Google's AP2 uses x402 as a default stablecoin rail; **Tempo** (the Stripe +
   Paradigm payments L1 localharness builds on) went mainnet **2026-03-18** with no
   native token and a Machine Payments Protocol. MCP went to the Linux Foundation's
   Agentic AI Foundation (97M+ monthly SDK downloads).

**The operative reframe for every channel:** lead with **payments rail + identity +
no-server runtime**, never with "token." localharness's "$LH is a metered usage
credit, not a tradeable token" and "self-funding is unproven, no earnings claims"
postures are not hedges — in 2026 they are the *credible* posture, and the single
best inoculation against the dismissal this category now earns by default.

---

## 1. Segment — Rust / SDK developers

**Who:** systems-minded engineers, allergic to dependency sprawl, want to own the
loop. Rust is *not* a common agent-building language — Python owns orchestration. The
honest, repeated 2026 line is **"Python is the scripting layer; Rust is the runtime
layer."** Rust's real agent wedge is infrastructure: wasm sandboxing of untrusted
skills, secure runtimes/CLIs, compile-time tool-schema guarantees, single-binary
local/edge inference. Position localharness as the *runtime/sandbox/portability*
layer, **not** a LangChain replacement.

### Where they actually gather
- **crates.io / lib.rs / blessed.rs** — primary discovery. crates.io now filters
  download counts to genuine Cargo requests (Jan 2026), so downloads are a trustworthy
  signal. Devs route around bare crates.io via **lib.rs** (faster browser) and
  **blessed.rs** (one hand-picked crate per need). Plus `awesome-rust` / `awesome-rust-llm`.
- **This Week in Rust (TWiR)** — highest-leverage editorial channel. "Crate of the
  Week" is community-nominated and **voted on users.rust-lang.org**; "Call for
  Participation" + links land via PR to the TWiR repo. This is the cleanest non-spammy
  on-ramp the project has.
- **r/rust** (~411K, growing ~16% YoY, ~35 showcase posts/mo) — largest single
  showcase venue, but crypto-allergic (below). **No large dedicated "Rust + AI" sub.**
- **Hacker News (Show HN) + Lobsters** — HN over-indexes Rust; time a Show HN for
  US-morning Pacific. Lobsters is smaller, higher-signal, invite-gated, has a `rust` tag.
- **Official spaces:** rust-lang **Zulip** + internals.rust-lang.org (dev/design),
  users.rust-lang.org + community Discord (help).
- **Social: the Rust project effectively moved to Bluesky + Mastodon; X is
  de-emphasized** (rust-lang.org/community lists Mastodon/Bluesky/YouTube/GitHub, not X).
  Bluesky has organized Rust starter packs/feeds.
- **Conferences (2026):** Rust Nation UK (Feb), RustWeek/RustNL Utrecht (May),
  **RustConf Montréal Sep 8–11**, Oxidize Berlin (Sep), **EuroRust Barcelona Oct 14–17**,
  TokioConf Portland (Apr). All CFPs already closed.
- **Newsletters/podcasts:** TWiR (weekly); Rust Trends (Bob Peters); **Rust in
  Production** (corrode / Matthias Endler) and Rustacean Station.

### What resonates vs. what falls flat
- **Resonates:** "one crate, small auditable dependency tree" — supply-chain anxiety
  is live and current (2025 `faster_log`/`async_println` malicious-crate incidents;
  "300-line tool, 141 deps" complaints). **Frame as discipline with concrete
  dep-count/SLOC numbers, not a "zero-deps" boast** — the community is selective, not
  eliminationist, and mocks reinventing trivial crates. Also: model-agnostic (answers
  framework fatigue); **wasm32 "runs client-side, no server"** (respected, but a
  demonstrated feature, not a revolution — browser-wasm lost its central banner when
  `rustwasm` archived); **Rust as the substrate for AI-generated code** (the compiler
  rejects hallucinated unsafe code — the one AI angle the crowd actually likes).
- **Falls flat / harmful:** superlatives ("blazingly fast" is a self-aware meme;
  "revolutionary"/"platform" invite eye-rolls); leading with "AI agent framework"
  (fatigue); and **any "web3 / token / decentralized" framing** — the dominant
  reputational risk here.

### Top objections to preempt
1. **"AI agent + crypto = grift."** Highest-probability dismissal. Preempt: lead with
   the working SDK/code; keep on-chain optional and subordinate; never say "token."
2. **"Crypto gives Rust a bad name."** Documented and acute — Rust devs are
   over-exposed to crypto recruiters (Solana/Polkadot/Parity are Rust); a Rust job
   board exists specifically to *exclude* crypto. **Cautionary precedent: Rig
   (0xPlaygrounds, ~7.8K stars)** — the most-starred Rust agent framework — is tethered
   to the ARC Solana token (down ~87% from ATH). The instructive split: the *framework*
   is treated as legit engineering; the *token* is lumped into the grift meta. Copy
   that posture exactly — framework on open-source merit, chain clearly optional.
3. **"Thin wrapper around an LLM API — I'd write the loop in an afternoon."** The most
   common framework critique. Preempt by showing the non-trivial parts: the
   Connection/strategy seam, hooks/policies, context compaction, native+wasm
   portability — and own that the core loop *is* simple rather than overclaiming.
4. **"Letting an agent control a wallet is reckless."** A real, documented risk class
   (key mgmt, prompt-injection → unauthorized transfers, liability). **This is where
   localharness flips an objection into a credibility win:** foreground the
   typed-confirmation gates on value-moving tools, spend caps, the policy layer,
   sponsored/relayed keys, and allowlists.
5. **Dependency bloat / supply-chain** (winnable, lead with one-crate) and **"not
   another agent framework"** (differentiate on boring-real Rust engineering).

> Signal: when a "Rust for AI agents" crate hit the official forum, reception was
> "cautious interest, not enthusiasm," and the one substantive reply asked for
> **local-model support** — so the experimental in-browser Gemma path (SDK-only) is a
> credibility asset to *mention*, not a headline to overclaim.

### Voices & communities worth genuine engagement (no scraping, no mass-DM)
- **Jeremy Chone** (`rust-genai`, rust10x, RustConf'25 speaker) — **the closest peer**
  (model-agnostic native-protocol multi-provider client) *and* the best Rust-AI content
  voice. Engage on engineering merit.
- **Laurent Mazare** (HF, `candle`), **Nathaniel Simard** (Tracel-AI, `burn` — already
  the engine behind localharness's local feature), **Eric L. Buehler** (`mistral.rs`),
  **Himanshu Neema** (`async-openai`).
- **Curated-list maintainers as PR targets:** `jondot/awesome-rust-llm`,
  `e-tornike/best-of-ml-rust`, `anowell/are-we-learning-yet`.

### → Best single channel for this segment
**This Week in Rust** — Crate-of-the-Week nomination (via users.rust-lang.org vote) +
a Call-for-Participation/links PR. Editorial, community-sanctioned, zero self-promo
friction, and it reaches exactly the buyer with none of the crypto-allergy blowback a
cold r/rust or X post risks.

---

## 2. Segment — Crypto / Tempo / web3 / agent-economy builders

**Who:** on-chain-native builders who want agents that *transact*, post-meme-cycle and
allergic to token theater. The credible side of this market in 2026 has rallied around
**payments-grade rails (x402), on-chain agent identity/reputation (ERC-8004), and
stablecoin/credit settlement** — exactly localharness's map, almost 1:1.

### Where they actually gather
- **The x402 ecosystem** — the single hottest gathering point for "agents that pay."
  Home is the **Linux Foundation x402 Foundation**; discovery via the **x402 Bazaar**
  (Coinbase CDP) and **x402scan** (Merit Systems). Recruiting venues: the x402
  hackathon circuit (SF "Agentic Commerce," Solana x402, Cronos PayTech $42K, Algorand
  Ideathon @ 42 Berlin). **Base** is the volume center of gravity.
- **ERC-8004 working group** — the most *philosophically aligned* standards community
  (on-chain agent identity + reputation; ERC-721 identity whose tokenURI points to a
  registration file — adjacent to localharness's name-NFT model). Live on Ethereum
  mainnet **2026-01-29**. Voices: **Davide Crapis** (EF "dAI" team), **Marco De Rossi**
  (MetaMask), **Jordan Ellis** (Google), **Erik Reppel** (Coinbase). Hub: the
  `erc-8004` GitHub org + `awesome-erc8004`. **Align with, don't compete.**
- **EIP-6551 (token-bound accounts)** — the standard localharness's agent wallet uses
  (Jayden Windle / Benny Giang, Tokenbound). Smaller, NFT-infra-flavored — good for
  *credibility* ("we use TBAs"), weaker as an acquisition channel.
- **Tempo / MPP builder community** — nascent but high-pedigree (Stripe + Paradigm,
  EVM-compatible, Foundry/Hardhat; design partners incl. OpenAI, Anthropic, Visa,
  Mastercard, Shopify). **Being a non-trivial real app on Tempo is rare and
  differentiating** — most agent projects are on Base/Solana.
- **Farcaster** — dominant social home for on-chain agent builders (auto-linked
  wallets; tooling via Neynar; the Farcaster Agentic Bootcamp). **Engage the
  builder/dev subset, not the Clanker/launchpad-degen scene** (which is the exact
  theater to position against).
- **ETHGlobal** — where agent×payments builders converge; Buenos Aires (Nov 2025) and
  Cannes (2026) finalists were heavily x402 + ERC-8004.
- **Account abstraction (ERC-4337/7702) circles** — care about gas sponsorship /
  paymasters; the sponsored-Tempo-tx + fee_payer relay story resonates, but it's
  plumbing, not a primary channel.

### What resonates vs. what falls flat
- **Falls flat:** token-launch theater (derided); **"autonomous / self-funding agent"
  claims without proof are radioactive** — that's literally the ai16z allegation. The
  task's "self-funding is OPEN, no earnings claims" rule is the correct, credible
  posture; overclaiming autonomy is the #1 way to get dismissed.
- **Resonates:** **"agents that transact, backed by credits/stablecoins not
  speculation"** (the mainstream-validated narrative — Stripe/Coinbase/Circle/AWS all
  shipped agentic-payment infra in early 2026). **"No token to pump, just a usage
  credit" lands — if framed as utility, not virtue.** The circulating litmus is the
  **Token-Utility test**: *if it would work just as well with a credit card, the token
  is a cash grab.* A pure metered $LH that is explicitly **not** fee-token-eligible and
  **not** tradeable passes cleanly — and **Tempo itself launched with no native
  token**, which you can cite as cover. Frame "no token" as *"we removed the thing you
  hate about crypto agents."*

### Top objections to preempt
1. **"Another AI × crypto grift / memecoin with extra steps."** Preempt: no tradeable
   token, no bonding curve, no launch; credit is a metered usage unit. Contrast
   directly with the ai16z suit and ZachXBT's "99% scams."
2. **"Why on-chain at all? Real agents don't need crypto."** The strongest objection,
   and *correct for most cases.* Answer narrowly: chain is the substrate **only** for
   the parts that need trustless settlement + portable identity + permissionless A2A
   payment — never for the inference itself.
3. **"Testnet, not mainnet — is this real?"** Be honest and turn it into rigor:
   x402-style payments are mainnet-proven *elsewhere* (Base/Solana, ~$600M annualized,
   Linux-Foundation-backed); **localharness runs its own payment rail on testnet
   deliberately while the self-funding economy is unproven.** This audience punishes
   overclaiming far harder than "early." (Note: never imply x402 *broadly* is
   pre-production — that reads as uninformed.)
4. **"'Autonomous economy' = the claim that got ai16z sued."** Never assert agents
   earn/self-fund; frame as an open research goal with the plumbing in place.

### Voices & communities worth genuine engagement
- **x402 builders** (the Foundation, Bazaar/x402scan crowd, the hackathon circuit) —
  localharness's per-call payment model *is* an x402 use case.
- **ERC-8004 working group** (Crapis, De Rossi, the `erc-8004` org) — contribute to /
  align the identity model publicly.
- **Tempo / MPP dev community** — Stripe/Paradigm-credible audience, rare to be a real
  app there.
- **Olas / Autonolas (Valory)** — the closest *philosophical* peer (crypto-native,
  API-free A2A micropayments since 2023, the Mech Marketplace, Pearl app integrated
  x402). Study / consider interop rather than dunk.

### → Best single channel for this segment
**The x402 ecosystem** (Foundation channels + Bazaar/x402scan listing + the hackathon
circuit). It is the neutral, institutionally-blessed standard whose entire premise —
agents paying per call over HTTP 402 — *is* localharness's thesis, and it concentrates
exactly the builders who already accept "agents that pay" without the token baggage.

---

## 3. Segment — AI-agent & indie-hacker builders

**Who:** solo devs / small teams shipping autonomous agents; want something live in an
afternoon with no infra to babysit; value self-hosting and no lock-in. In 2026 this
crowd has matured past hype into a **"show me production reliability and economic
payoff"** posture, with acute framework fatigue.

### Where they actually gather
- **The MCP ecosystem — the biggest gravity well.** 97M+ monthly SDK downloads,
  10,000+ production servers, donated to the **Linux Foundation Agentic AI Foundation**
  (founders incl. Block, OpenAI, AWS, Google, Microsoft); first **MCP Dev Summit NA**
  ran Apr 2–3 2026 (NYC). MCP is *the* interoperability standard and de-facto meeting
  ground. **localharness already ships stdio MCP — lead with it; it's table stakes for
  credibility here.**
- **Hacker News** — still the center of gravity for dev-tool launches, but matured:
  the conversation is now reliability, pricing, context behavior, orchestration of
  *bounded* workflows with a human supervisor — and **HN actively distrusts demos**, so
  a "real product, not a demo" defense is mandatory.
- **Indie Hackers** — alive but past peak (~140K members; energy diffused to WIP.co,
  Makerlog, niche Discords after Stripe cut the community team). One channel, not *the*
  channel.
- **Product Hunt** — still relevant for dev tools that "show value in seconds"; active
  LLM-dev-tools category. "A launch is a stress test, not proof of PMF" — wins come
  from clear messaging + live founder replies + fast setup.
- **X vs Bluesky for build-in-public** — X still has reach, but **Bluesky
  over-indexes** for dev-tool/open-source/values-aligned audiences (reportedly 2–4×
  engagement, chronological feed). Caveat: Bluesky rejects *uninvited* AI (the "Attie"
  backlash) — build-in-public there must be participatory, not broadcast.
- **Builder subreddits** — r/AI_Agents, r/LocalLLaMA, r/LLMDevs are large and active
  (couldn't pin 2026 metrics — present as active-but-unquantified). The local-first /
  self-host subculture is the segment most receptive to "no server, your keys, any
  model."
- **Newsletters/voices:** **Latent Space (swyx)** — the canonical "AI engineer"
  publication, most-aligned; **TLDR AI**, **Ben's Bites** (skeptical, indie-leaning).
  YouTube: Matt Wolfe, Nate Herk (agent build tutorials), AI Explained.
- **Hackathons/confs (2026):** MCP Dev Summit NA (Apr); Microsoft AI Agents / VSLive!
  Hackathon (Jul); lablab.ai + Devpost run continuous agent hackathons.

### What resonates vs. what falls flat
- **Falls flat:** generic "AI-powered" buzzword splash (YC's 2026 RFS wants
  AI-native-where-removing-AI-breaks-it); **abstract "self-sovereign" jargon on its own
  reads as crypto-coded**; token/coin/staking framing (devs are explicitly "frustrated
  by the requirement to stake tokens to deploy agents").
- **Resonates:** **concrete operational wins** ("less ops," "ship faster,"
  reliability); **"no server to operate / runs in the browser"** — localharness's
  strongest honest hook, mapping directly to the "less ops" demand and the local-first
  / data-sovereignty segment (decoupled, auditable, any-model). **The wallet angle —
  but framed as x402 payments, NOT a token** (see objection 2).

### Top objections to preempt
1. **"Why Rust, not Python/TS?"** Legit — most of this crowd is Python/TS. But the 2026
   thesis favors localharness for the *runtime* layer: the stack is splitting by layer
   and the execution/sandbox layers are flipping to Rust (memory safety, no-GIL
   concurrency, wasm sandboxing of untrusted code). Frame as the *execution + sandbox*
   layer where Rust genuinely wins — not a Python orchestration replacement.
2. **"Why on-chain / why crypto?"** Highest-risk objection here, and the most fixable.
   **Lead with "agents can pay and get paid over x402 — the rail Stripe/Google/Visa/
   Circle back," not "$LH token."** Downplay token/staking entirely. Pair the payments
   pitch with the safety story (confirm-gates, spend caps, sponsored relay) — 2026 saw
   real agent-wallet exploits and "who's liable" debates.
3. **"Yet another agent framework."** Fatigue is acute (viral "agents are killing the
   framework ecosystem" threads; Microsoft dinged for stack sprawl). **Defense: don't
   compete in the orchestration-framework bucket** — localharness is a *deployment +
   identity + runtime platform* (`cargo add` → an agent living at `name.localharness.xyz`,
   no server, identity + payments built in). Be explicit it's a different category, not
   orchestration-lib #38.
4. **"Real product or a demo?"** HN rewards reliability/economic-payoff and is allergic
   to demos. Bring a live, dogfooded, reliability-and-cost story (the autonomous-business
   dogfood is exactly this artifact — once it's run, not while it's still preview).

### Voices & communities worth genuine engagement
MCP working groups + MCP Dev Summit crowd (highest fit); the Latent Space / swyx orbit;
TLDR AI + Ben's Bites; Bluesky dev/open-source circles (participatory build-in-public);
Show HN (reliability-first framing); the local-first / self-host subculture.

### → Best single channel for this segment
**The MCP ecosystem** (a polished stdio + HTTP MCP server/client story, surfaced in the
MCP Dev Summit / server-directory crowd). It is the one venue where "no server,
browser-resident, any model, with an MCP surface" reads as *infrastructure* rather than
"another framework," and it routes around both the crypto-allergy and the framework
fatigue.

---

## 4. Competitive positioning refresh (2026)

**Honest framing first:** the competitors are **not dying** — most are healthier than
ever. The fair wedge is **language/runtime + leanness + category**, not market share.
Every competitor below is a **framework/SDK you still deploy and operate**, and all are
Python or TypeScript.

| Competitor | Category | 2026 state | Stronger than localharness at | Honest contrast localharness can draw |
|---|---|---|---|---|
| **LangChain / LangGraph** | Python/JS LLM-app framework + orchestration runtime + hosted platform (LangSmith) | **$1.25B unicorn** (Series B $125M, Oct 2025); ~90M monthly downloads; 35% of Fortune 500; LangGraph (~36K★) is the part that survived; LangChain ~141K★ | Ecosystem breadth; **LangSmith** observability/eval; hosted durable-agent platform; enterprise track record; community/hiring gravity | Abstraction overhead; dep bloat; version churn; Python latency; **2026 CVEs** (deserialization 9.3, path-traversal, SQLi in the checkpointer). Tiny auditable surface, native+wasm, no Python runtime |
| **CrewAI** | Python role-based multi-agent ("crews" + "flows") + hosted platform | ~$18M raised (Insight Series A); ~**20% of production teams** (#2); self-reported ~60% F500 + ~2B executions (marketing, not audited) | Mindshare/adoption; mature role/delegation patterns; Python ecosystem; enterprise sales motion | **You host & operate it**; noisy observability; debugging is "detective work"; ~80% production reliability; Python-only; **no identity, deployment surface, or payments** |
| **Vercel AI SDK** | Apache-2.0 TS toolkit — provider abstraction + streaming-UI primitives | **~16M weekly npm downloads** (~25K★); aggressive ~6-mo majors (v7, Jun 2026); 55+ providers | Frontend streaming/generative UI (its home turf); React/Next/Vue/Svelte parity; npm reach; fastest path to a web chat app | **JS/TS-only, Node runtime**; **Vercel hosting pull** (AI Gateway / Workflows / Sandbox — lock-in at the infra layer, not the lib); version churn. No native/single-binary/edge |
| **OpenAI Agents SDK** | Lightweight production agent SDK (Py + TS); the **Swarm successor** | v0.17.x (May 2026), ~26K★; **Assistants API permanently removed 2026-08-26**; multi-provider via a *beta* LiteLLM adapter | OpenAI backing + cadence; polished tracing/guardrails/sessions; hosted tools; mindshare; smoothest path for the largest model userbase | **Vendor gravity** ("board-level concern," tightly coupled to OpenAI specifics); multi-provider is best-effort beta; you run your own infra; OpenAI's own churn (Assistants→Responses) taxes builders. **Model-agnostic by design** here |
| **Eliza / ai16z (elizaOS)** | TypeScript/Node multi-agent framework; the original crypto-agent framework | Still shipping (v2, an "Agentic SME" deal), but token collapsed ~-99.9% and a **class action (Apr 2026, SDNY)** alleges faked autonomy | Crypto-agent **mindshare/brand**; large plugin + connector library (Discord/X/TG/Farcaster); TS reach; distribution | Token-speculation origin **+ active fraud litigation**; **character-file (off-chain JSON) identity** vs name-NFT + ERC-6551 TBA; **chain-as-plugin** vs chain-as-substrate. (Stay fair: Eliza has real adoption; localharness is testnet + unproven self-funding) |

**Broader landscape (so positioning is well-rounded):** LangChain/LangGraph leads
production (~45% of teams); **Microsoft Agent Framework 1.0** (Apr 2026, AutoGen +
Semantic Kernel unified — criticized as heavy/confusing); **Google ADK** (code-first,
now Go 1.0); **Pydantic AI** (the type-safety play); **Mastra** (TS-native, $13M seed,
~22K★, the leading TS option alongside Vercel AI SDK). On the crypto side:
**Virtuals/GAME** (token-launchpad origin, built ACP), **Olas/Autonolas** (the
utility-not-token peer), **Coinbase AgentKit** ("every agent deserves a wallet" — more
on-ramp than rival).

### The ONE wedge localharness owns

> **The same Rust crate is *both* the agent SDK and the deployed, self-owned agent —
> `cargo add` gives you the loop, one feature flag compiles it to wasm and it *is* the
> browser-resident agent at `<name>.localharness.xyz`, with identity and per-call
> payment as first-class primitives and no server to operate.**

No competitor collapses SDK-and-deployed-agent into one artifact: LangGraph, CrewAI,
Vercel AI SDK, OpenAI Agents SDK, and Mastra are all **frameworks you deploy and
operate** (Python or TS), and Eliza is a TS framework whose identity is a config file
and whose chain is a plugin. localharness's defensible whitespace is the *combination
almost nobody ships together* — **Rust-native single crate + native *and* wasm32 from
the same binary + no server (browser-resident) + self-owned on-chain identity (NFT +
TBA wallet) + native x402 payments + model-agnostic.** The two ways to lose this
position are well-understood and avoidable: (1) being mis-bucketed as "orchestration
framework #38" (fix: sell it as a *deployment/identity/runtime platform*, a different
category) and (2) being mis-read as "a crypto token project" (fix: payments-rail and
usage-credit framing, never "token," chain clearly optional and safety-gated).

---

## Sources

Web sweep, June 2026. Selected primary/strong sources (see segment text for inline context):

**Cross-cutting / 2026 macro:**
- ai16z class action: claimdepot.com/cases/ai16z-class-action…; ZachXBT "99% scams": banklesstimes.com (2025-01-06); "memecoins that talk": tradingview/Cointelegraph
- x402 → Linux Foundation + backers: blog.cloudflare.com/x402, thedefiant.io, coindesk.com (2026-04-02); network support: docs.cdp.coinbase.com/x402/network-support
- Tempo mainnet (2026-03-18, chain 4217, no native token): crypto.news, coindesk.com, docs.tempo.xyz; chainlist.wtf/chain/42431 (Moderato testnet)
- MCP → Agentic AI Foundation, 97M downloads, Dev Summit NA: blog.modelcontextprotocol.io, digitalapplied.com

**Rust segment:** lib.rs/stats; blog.rust-lang.org (2026-01-21 crates.io update); github.com/rust-lang/this-week-in-rust; rust-lang.org/community; corrode.dev/blog/rust-conferences-2026; supply-chain: cybersecurefox.com, aviatrix.ai; "Python scripting / Rust runtime": dev.to field guide, ossinsight.io/blog/rust-ai-agent-infrastructure-2026; crypto-allergy: news.ycombinator.com/item?id=37895227; Rig/ARC: gate.com, coinmarketcap.com/currencies/ai-rig-complex; peers: github.com/{jeremychone/rust-genai, huggingface/candle, tracel-ai/burn, EricLBuehler/mistral.rs}; forum signal: users.rust-lang.org/t/rust-for-ai-agents/136946

**Crypto segment:** eips.ethereum.org/EIPS/eip-8004 + eip-6551; github.com/erc-8004; x402.org/ecosystem, x402scan.com, docs.cdp.coinbase.com/x402/bazaar; ETHGlobal Buenos Aires/Cannes prizes; neynar.com (Farcaster agents); olas.network (Mech Marketplace), autonolas on X; elizaOS rebrand/token: theblock.co, cryptoslate.com, coinspeaker.com; Virtuals/GAME: whitepaper.virtuals.io; AgentKit: github.com/coinbase/agentkit

**Indie/MCP segment:** developersdigest.tech + epsilla.com (HN 2026 sentiment); indiehackers community roundups: letstalkshop.com, vibecontentcreation.com; producthunt.com/categories/llm-developer-tools; bluesky vs X: monolit.sh, posteverywhere.ai; CrewAI: pulse2.com, blog.crewai.com, getpanto.ai; OpenAI Agents SDK: openai.com/index/the-next-evolution-of-the-agents-sdk, community.openai.com (Assistants sunset), openai.github.io/openai-agents-python/models; landscape: yaitec.com, langchain.com/state-of-agent-engineering, learn.microsoft.com/agent-framework, adk.dev, generative.inc (Mastra)

**LangChain/Vercel (competitive):** langchain.com/blog/series-b + langchain-langgraph-1dot0; techcrunch.com (2025-10-21 unicorn); thehackernews.com (2026-03 CVEs); github.com/{langchain-ai/langchain, langchain-ai/langgraph, vercel/ai}; vercel.com/blog/ai-sdk-5/6/7; ai-sdk.dev/docs; api.npmjs.org/downloads/point/last-week/ai; truefoundry.com (Vercel lock-in analysis)

**Flagged / not independently verified:** CrewAI's "~60% F500 / ~2B executions" (self-reported); framework market-share %s (analyst/vendor estimates); elizaOS "AUM 300x" and Virtuals "$1M/mo" (promotional); exact x402 cumulative-txn figures (range 119M–165M across sources); r/AI_Agents·r/LocalLLaMA·r/LLMDevs 2026 metrics (search gap — active but unquantified); TWiR subscriber count (unpublished); Rust "left X" (inferred from channel listings).
