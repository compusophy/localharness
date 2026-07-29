# The agent-harness landscape — research brief, 2026-07-29

> Aggregated learnings from every major harness we could reach, gathered live on
> 2026-07-29 (12 parallel researchers + an adversarial completeness pass + three
> strategy lenses). We forked our thinking from Google Antigravity early and have
> not tracked the field since; this is the catch-up, plus what it means for us.
>
> **Verification standard.** Every dated claim below was read from a live source
> this session. Claims that could not be confirmed are quarantined in
> §10 — do not promote them out of that section without re-checking. Benchmark
> leaderboard numbers are aggregator-sourced and are flagged as such throughout.

---

## 1. Four corrections to our working map

We held four beliefs going in. Two were wrong, two were half right. Recording the
corrections first because everything downstream depends on them.

**(a) "Hermes is the successor to OpenClaw" — REFUTED.**
`NousResearch/hermes-agent` was created **2025-07-22**; `openclaw/openclaw` was
created **2025-11-24**. Hermes predates OpenClaw by four months, so it cannot be
its successor. They are different codebases in different languages (Hermes:
Python, MIT, Nous Research; OpenClaw: TypeScript/Swift, MIT, Peter Steinberger),
both alive, both pushed within minutes of each other on 2026-07-29, and **each
ships an importer for the other**. Neither project's own docs use the word
"successor" — that framing exists only in third-party comparison blogs.
What the intuition is actually tracking is **displacement**: Hermes overtook
OpenClaw at #1 on OpenRouter's daily app rankings around 2026-05-10.
OpenClaw's name lineage is real though: Warelay (2025-11-24) → Clawdbot
(2025-12-03) → Moltbot (2026-01-27, forced by an Anthropic trademark complaint)
→ OpenClaw (2026-01-30). It is now stewarded by a 501(c)(3) OpenClaw Foundation
(2026-07-08) and is **the largest agent repo on GitHub: 384,450 stars**.

**(b) "Antonio from Paradigm has nanocodex" — WRONG NAME, right affiliation.**
nanocodex is **Georgios Konstantopoulos** (`gakonst`), CTO & General Partner at
Paradigm. Nobody named Antonio appears anywhere in the repo. The project is
**14 days old** (created 2026-07-15, crates.io 0.1.0 on 07-21, 0.3.0 on 07-28,
296 stars, Apache-2.0).

**(c) "Everything will become Rust" — TRUE OF EXACTLY ONE LAYER.**
Rust decisively won *the harness binary*: Codex is a ~99-crate Rust workspace
(43.8 MB Rust vs 86 KB TypeScript by GitHub language bytes, `rust-v0.146.0`
published 2026-07-29), goose is Rust core + Electron shell, Zed is Rust and
governs ACP, xAI's `grok-build` (2026-07-14) is a Rust TUI that took 23.3k stars
in two weeks. It has won **nothing above that layer**: the biggest harnesses by
stars are TypeScript (OpenClaw 384k, opencode ~190k, OpenHands 82k, Cline ~63k)
or Python (Hermes 222k); the entire commercial tier (Amp, Cursor, Factory, Devin)
is TypeScript-centric; the official Rust MCP SDK is **Tier 2/beta** against the
new spec while TS/Python/Go/C# are Tier 1; and neither Anthropic nor OpenAI
ships an official Rust API SDK (Anthropic's request issue #1559 has sat open with
no maintainer reply since 2026-05-17). The sharpest structural counter: of the
five harnesses in the ALE ablation, **exactly one (Codex) is Rust**.
⚠️ One consequence in our favour that is easy to miss: **neither Codex nor goose
publishes a single crate to crates.io.** A real, `cargo add`-able agent crate is
rarer than the Rust-everywhere narrative suggests. That is our slot.

**(d) "Agentic meta → swarm/colony meta" — HALF RIGHT, and the second half is
contested right now.**
Swarm/colony was a genuine mindshare meta (OpenAI Swarm, CrewAI/AutoGen era,
peaked mid-2025) but never became the production meta. The pivotal week was
2025-06-12/13, when Cognition published "Don't Build Multi-Agents" and Anthropic
published its multi-agent research system one day apart; the dispute was settled
empirically by MAST (NeurIPS 2025, 14 failure modes over 1600+ traces) and by
Anthropic's own **2026-01-23 walk-back to "start single-agent, decompose by
*context*, not by problem."** What succeeded the agentic meta is
**harness engineering** on the scaffolding side and **model-native
internalization via RL** on the weights side.
BUT the "swarm is dead" conclusion is too clean. In the seven days to 2026-07-23
Amp shipped agent-to-agent spawning and cross-thread file exchange (07-17), a
meta-agent called Puck (07-20), **agent self-scheduling** (07-21), and
event-driven remote execution Orbs (07-23). Factory's whole enterprise product is
a coordinator dispatching to role-specialized droids. And Claude Code visibly
oscillated in one week: v2.1.217 (07-21) capped concurrent subagents at 20 and
**banned nesting entirely**; v2.1.219 (07-24) **reinstated nesting at depth 3**.
The honest read: *swarm-as-architecture is dead; **bounded recursion with explicit
depth and concurrency budgets** is live and being actively tuned by every major
vendor.* That is precisely the shape of our `ComposeBudget` — which makes depth
caps a first-class product decision, not a defensive footnote.

---

## 2. Who's who, as of 2026-07-29

| Harness | Owner | Language | Note |
|---|---|---|---|
| OpenClaw | OpenClaw Foundation (Steinberger) | TS/Swift | 384k★, largest agent repo; v2026.7.2-beta.5 on 07-28 |
| Hermes Agent | Nous Research | Python | 222k★; the self-improvement reference implementation |
| Codex | OpenAI | Rust (~99 crates) | 102k★; ships daily; JSON-RPC "App Server" behind every surface |
| opencode | — | TS | ~190k★ |
| OpenHands | All-Hands-AI | TS | 82k★ |
| Cline | Cline | TS | ~63k★, CLI 2.0 + SDK |
| Antigravity | Google | closed (Go CLI, Python SDK) | 2.0 is a standalone app, **no code lineage to 1.0** |
| Amp | Sourcegraph | TS | the most architecturally aggressive shipper right now |
| Cursor | Anysphere | TS | Router (07-22), side chats, cloud-agent hooks |
| Factory (Droid) | Factory | TS | $150M Series C at $1.5B (2026-04-16); role-split droids |
| goose | **Linux Foundation (AAIF)** | Rust + Electron | moved `block/goose` → `aaif-goose/goose` ~2026-03 |
| Buzz | Block | Rust (Axum) | Apache-2.0, launched **2026-07-21** |
| grok-build | xAI | Rust | 2026-07-14, 23.3k★ in two weeks |
| nanocodex | gakonst (Paradigm) | Rust | 14 days old; library-first; **pays for inference on Tempo** |
| Roo Code | — | TS | **DEAD** — final VS Code release 2026-05-15, repo read-only |

---

## 3. What changed while we weren't looking

**Google Antigravity went closed and consolidated.** 1.0 launched 2025-11-18 as a
VS Code fork. At I/O 2026 (**2026-05-19**) Google rebuilt it as **Antigravity
2.0** — a standalone desktop app with *no code lineage to 1.0* — plus three
sibling surfaces: the **`agy` CLI (Go, closed source)**, which killed the
100k-star Apache-2.0 Gemini CLI for consumer tiers on **2026-06-18**; an
**SDK (Python, Apache-2.0) that wraps a closed compiled harness binary**; and
**Managed Agents** in the Gemini API. All four run "the same shared agent harness
co-trained with Gemini models" — that phrase is the entire product thesis. The
unit of work moved from repo → **project**, and single-agent → **dynamic
subagents** (`invoke_subagent`, clean context, nesting depth cap 10, workspace
mode `inherit | branch (git worktree) | share`). Extensions are now four
concentric things — Skills (`SKILL.md`), Rules, MCP servers, Hooks — bundled into
**Plugins**, plus **Sidecars** (supervised long-running background processes with
`restart_policy` and cron). No BYOK, no BYO-endpoint, and credit unit economics
are undisclosed (The Register asked and got no answer). Kilpatrick frames
Antigravity as "the connective tissue across Search, the Gemini app, Cloud, and
AI Studio" — it is Google's internal harness-consolidation play.
**Our read: the thing we forked from is now a closed, vertically-integrated,
credit-metered product. Divergence was correct and is now irreversible.**

**Codex is a protocol, not a CLI.** The CLI is one consumer of a JSON-RPC 2.0
**App Server** shared by every Codex surface (CLI, VS Code/Xcode/JetBrains,
desktop, web); its schema is emitted per-build (`codex app-server
generate-ts` / `generate-json-schema`). Sandboxing is real OS enforcement — a
Chrome-derived Seatbelt `.sbpl` on macOS, bundled bubblewrap (plus a
`linux-sandbox` crate) on Linux, a hand-rolled `windows-sandbox-rs` (restricted
SIDs, ACLs, desktop isolation, DPAPI, ConPTY) on Windows — kept **orthogonal to
`approval_policy`, which only decides when to pause**. The 2026 architectural
move is **Code Mode**: tools projected as typed JavaScript functions executed in
a vendored V8, now with **remote Code Mode hosts over WebSocket** (07-29). Also
notable: `/import` migrates **competitors'** config (Cursor, Claude Code) —
harness config is commoditizing.

**Hermes is the self-improvement reference design, and it decomposes cleanly.**
Four separable layers with different risk profiles:
1. an **after-every-turn background review** that patches memory and skills;
2. **skills as procedural memory** with progressive disclosure;
3. a **curator** that garbage-collects agent-authored skills with *deterministic
   decay* plus rollback snapshots;
4. a **separate offline DSPy+GEPA optimizer that emits pull requests** rather
   than mutating anything live.
Memory is deliberately tiny and cache-preserving: two hard-capped markdown files
frozen into the prompt at session start, plus a zero-token-cost SQLite FTS5
session search as the unbounded tier. **It has no identity, payment, or
verification layer of any kind** — that exists only as a community proposal and
an experimental third-party x402 wrapper.

**Buzz is the Nostr-native workspace, and its "compatible harness list" is a
hardcoded const.** A Rust/Axum relay over Postgres + Redis + S3/MinIO where every
message, reaction, workflow step, git patch and CI status is a Schnorr-signed
Nostr event. Central design claim is **human-agent parity**: an agent gets its own
keypair and its own authorship and never impersonates its owner. It ships
`buzz-acp`, a bridge from relay events to any ACP agent over stdio, with a
`KNOWN_ACP_RUNTIMES` catalog covering goose, Claude Code, Codex and `buzz-agent`.
Two genuinely unusual pieces of engineering: **git repos on plain object storage
with a compare-and-swap manifest pointer, formally verified in TLA+**, and
**encrypted peer-to-peer model inference over QUIC/iroh (MeshLLM)**.
There is **no published harness-compatibility matrix** anywhere — what exists is
the ACP agent registry (~38 entries) and Buzz's four-entry const.

---

## 4. The standards layer — and the three we are not on

Interop split into three non-competing slots plus a convention:

- **MCP** — agent → tool/context. We ship this.
- **ACP** (Agent Client Protocol) — editor/UI → agent. **We do not.**
- **A2A** — agent → agent. LF-hosted, v1.0, 150+ orgs.
- **AGENTS.md** — zero-ceremony repo instructions (OpenAI's AAIF contribution;
  the claim that it originated at Anthropic is simply wrong).

**ACP is the one that matters most to us commercially.** JSON-RPC 2.0,
newline-delimited over stdio, created by Zed, now **jointly governed by Zed and
JetBrains**; v1 stable (Rust crate v1.6.0 / schema-v1.20.0, both 2026-07-21), v2
in alpha. Official SDKs in **Rust**, TS, Python, Java, Kotlin. Agent-side methods
are a small surface: `initialize`, `authenticate`, `session/{new,load,resume,
prompt,cancel,close,delete,list,set_mode,set_config_option}`. Client-side:
`fs/{read,write}_text_file`, `terminal/{create,output,wait_for_exit,kill,release}`,
`session/{update,request_permission}`, `elicitation/*`.
The ~38-agent registry includes **Cursor, Cline, Goose, Gemini CLI, GitHub
Copilot, OpenHands, JetBrains Junie, Factory Droid, Kimi CLI, Qwen Code, Mistral
Vibe, Docker cagent, OpenClaw and Hermes Agent**; Claude Code and Codex are on it
**via Zed-authored adapters**, not natively. Buzz's compatible-harness list is
effectively this list. *Speaking ACP is how a harness stops being invisible.*

**Agent Skills is the other one.** A skill is a folder with a `SKILL.md`
(`name` + `description` minimum) plus optional `scripts/`, `references/`,
`assets/`, loaded by **progressive disclosure** — discovery (name+description
only) → activation (full instructions) → execution. Originally Anthropic,
released as an open standard at agentskills.io on 2025-12-18, and now adopted by
~40–45 products including Claude Code, Codex, Cursor, Copilot, VS Code, Gemini
CLI, goose, OpenHands, Junie, Amp, Factory, Roo, Kiro, Letta, Mistral Vibe, Trae,
Spring AI, Tabnine, Snowflake, Databricks and Pulumi. **A skill written for one
agent runs unmodified in a competitor's** — the clearest sign a vendor feature
became shared infrastructure. Our skills are an on-chain JSON blob
(`[{name, instructions}]`, 16 max, 4000 B) — semantically the same object, wire-
incompatible, and unable to carry scripts or references.

**MCP became stateless on 2026-07-28 — one day before this brief.** The revision
removes `initialize`, `Mcp-Session-Id`, `ping`, and SSE resumability; every
request self-describes protocol version and capabilities in `_meta`; servers must
implement `server/discover`; server-initiated calls are replaced by a retry-based
Multi Round-Trip pattern (`resultType: "input_required"`); Roots, Sampling and
Logging are formally deprecated under a new 12-month deprecation policy.
**This is the first version of MCP a browser-resident, serverless agent can
actually implement** — and the official Rust SDK is Tier 2/beta with no wasm
support, every transport tokio/hyper.

**x402 has moved and we may have drifted.** v2 (2025-12-11) standardizes CAIP-2
network IDs and **renames the headers** to `PAYMENT-REQUIRED` /
`PAYMENT-SIGNATURE` / `PAYMENT-RESPONSE` (replacing the `X-` names), and adds an
extensions framework carrying SIWx sessions and the Bazaar discovery layer. The
**x402 Foundation reached operational launch under the Linux Foundation on
2026-07-14 with 40 members including Visa, Mastercard, Stripe, Google, AWS,
Cloudflare and Circle.** → ACTION: audit our header names and CAIP-2 handling
against v2 before claiming x402 compatibility.

**ERC-8004 is registration-heavy and operationally hollow.** Still formally
**Draft** on eips.ethereum.org as of 2026-07-29 despite canonical registries
being live at deterministic `0x8004…` addresses on 30+ chains since 2026-01-29.
A June 2026 measurement of the first 10,000 mainnet agents found **0.67% declared
any service endpoint and 6.28% had any feedback, with one client authoring 66% of
all feedback**. The lesson for us is not "avoid 8004" — it is that **asserted
reputation is worthless; only derived reputation is worth building.**

---

## 5. The evidence on "the model will eat the harness"

This is the strategic question, and the evidence is genuinely split — but it
splits along a line nobody states clearly, so state it:

**What is being eaten is the PROMPT. What is growing is STATE, ISOLATION, BUDGET
and ROUTING.**

For the thesis, and stronger than any podcast opinion: **ALE-Claw** (Agents' Last
Exam harness ablation, 2026-06-11) is a deliberately minimal harness derived from
OpenClaw with the product machinery stripped and the **system prompt cut ~65%**.
Result: same accuracy band (mean **0.485 vs OpenClaw's 0.464**) at **44% fewer
input tokens, 41% lower cost, 60% less wall-clock**. Verbatim: *"A richer harness
is not automatically a better one."* That independently reproduces Boris Cherny's
report that Anthropic **removed ~80% of Claude Code's system prompt** for its
newest models — with, in the careful phrasing, *no measurable loss on coding
evaluations* (the "the model got smarter" version is an aggregator's
embellishment). Logan Kilpatrick put a **~12-month clock** on today's scaffolding
on Sequoia's Training Data (**2026-06-11**), so the falsifiable window closes
**June 2027**.

Against the thesis: in the same eight weeks that Anthropic deleted 80% of the
prompt it also shipped subagent concurrency caps, nesting-depth controls,
background-agent hooks and sandbox changes; Amp shipped Orbs, a meta-agent and
agent-to-agent messaging; Cursor shipped a routing layer and three new lifecycle
hooks. Both labs are also **buying** harness talent (OpenAI hired OpenClaw's
Steinberger, 2026-02-15) — you don't buy what you expect to be worthless. And
nanocodex's stated thesis is the exact opposite framing: **"the model and harness
are co-designed."**

Also load-bearing, and the strongest ALE-adjacent signal: across the benchmark,
**model choice produces roughly 3× the performance spread that harness choice
does.** If the harness is a third-order variable, then the *implementation
language of the harness* is fourth-order — which is the real reason (c) above is
only one-layer true.

**Two research results that should change what we build:**

- **Metaprogramming (arXiv 2606.10933, 2026-06-09).** Frontier models faced with
  unfamiliar languages (Brainfuck, Befunge-98) do **not** write the target
  language directly — they write **generators** in a language they already know
  and debug those locally. Restricting metaprogramming hurts performance;
  handing weaker models the helpers improves it. **Direct implication for
  rustlite:** the correct affordance is a scratch workspace, a fast and honest
  compile-error loop, and a corpus of published cartridges to read — *not* more
  syntax documentation in the system prompt. More prose about our bespoke
  language is the exact failure mode our own telemetry already diagnosed.
- **METR is retiring the time-horizon metric.** It published **"expenditure
  horizon" on 2026-07-21**, denominating agent capability in **dollars** rather
  than minutes, finding frontier agents deliver ~1–1.5% NanoGPT speedups for
  $2,300–$3,300 against a ~$2,500-per-1% human baseline. The field's own unit of
  account is becoming money. We are the only harness in this brief with money
  natively in the substrate.

---

## 6. What to steal

Ranked by value-to-effort, each with its source and its catch.

1. **Hermes's four-layer self-improvement split.** We have lessons/skills/persona
   but no *curator* and no *offline optimizer*. Steal specifically: deterministic
   decay + rollback snapshots for agent-authored skills, and an optimizer that
   **emits a proposal rather than mutating live state**. Catch: their DSPy+GEPA
   phase 1 is the only phase verified implemented.
2. **Hermes's memory shape.** Hard-capped files frozen at session start (so the
   prompt cache survives) + an unbounded, zero-token search tier over past
   sessions. We fold lessons into every prompt with no search tier at all.
3. **Codex's orthogonality rule: sandbox ≠ approval.** OS enforcement decides what
   is *possible*; `approval_policy` only decides *when to pause*. Our
   `confirm_guard` and our wasm/bashlite sandboxes should be explicitly separated
   the same way, and documented as such.
4. **Bounded recursion as a tuned product surface.** Everyone is converging on
   explicit depth + concurrency caps (Antigravity depth 10; Claude Code 20
   concurrent, nesting depth 3 after a one-week reversal). Our `ComposeBudget`
   already has this shape — surface the numbers as configuration and publish
   them, rather than burying them as constants.
5. **Antigravity's artifact-mediated verification.** Plans, diffs, diagrams,
   screenshots and browser action-videos as reviewable artifacts the running
   agent ingests *without a restart*. Our framebuffer + telemetry already produce
   most of these; we do not treat them as a reviewable artifact stream.
6. **Sidecars** (supervised long-running background processes with a restart
   policy) — a cleaner primitive than our scheduler for "keep this running".
7. **nanocodex's fork-by-checkpoint.** Treat the provider's response ID as an
   opaque checkpoint and fork by sending only the new user delta with
   `previous_response_id`, keeping ONE lineage cache key across descendants.
   Their measured medians: branch latency 1.224 s vs 5.082 s; branch input 8.5 K
   vs 24.7–26.9 K tokens; 99.6% cached-input ratio vs 19.3%. Catch: this is
   provider-specific (OpenAI Responses) and they label it live-service
   observation, not a controlled benchmark.
8. **Buzz's CAS-manifest git-on-object-storage** (TLA+ verified) — the right
   shape if we ever need repo state without a git server.

---

## 7. The lane that is actually open

Everything above is table stakes or catch-up. This is the part that is ours, and
it came out of the research rather than out of ambition:

**The skill supply chain is on fire, and the named unbuilt fixes are our shipped
primitives.**

The evidence, all from this session:
- **ClawHavoc: ~1,200 malicious OpenClaw skills** on ClawHub, exfiltrating API
  keys, crypto wallets and browser credentials at scale.
- **Bitdefender found ~17% of early skills malicious** (Feb 2026); Koi Security
  disclosed 341; Unit 42 found five that evaded detection entirely, one padding a
  README with **22 MB** to defeat scanner thresholds, with **C2 infrastructure
  still live more than three months after public disclosure**.
- Unit 42's list of what is missing from the distribution model, verbatim in
  substance: **no code signing, no sandboxing, no granular permissions, limited
  review.** "Lack of isolation between skill logic and agent authority means
  installation results in complete control over the agent's identity."
- **SoK: Agentic Skills** names the fixes and says nobody has built them:
  trust tiers with progressive disclosure backed by **provenance verification**,
  sandboxing and permission boundaries, signing, and **outcome-weighted ranking**
  — of which it states plainly that **no production system employs outcome-
  weighted ranking of skills**, and flags **"governance economics and liability"**
  as entirely unaddressed.
- The registry layer went from one registry (Dec 2025) to **eight competing
  marketplaces** by Q2 2026, and the ecosystem's own stated bottleneck is
  *"discovery is no longer the bottleneck — judgment is."*

Now map that against what localharness already runs in production:

| What the literature says is missing | What we already ship |
|---|---|
| Code signing / provenance | Every agent is an on-chain ERC-721 identity with a wallet; published artifacts are owned and signed |
| Sandboxing of third-party code | Cartridges are untrusted wasm off-main-thread in a Web Worker with a watchdog; bashlite is fuel-bounded; `Rooted` FS confines |
| Granular permissions | Tool allowlist + policy predicates + dispatch-layer confirm gate |
| **Outcome-weighted reputation** | `ReputationFacet.attest(subject, 1..5, workRef)` with per-work dedup; ERC-8004-style stake/challenge/resolve in `ValidationFacet` |
| **Governance economics / liability** | x402 settlement, escrowed bounties, validation staking — skin in the game |

That is not a coincidence we should be smug about; it is a roadmap. A `SKILL.md`
today is an unsigned folder of instructions and **executable `scripts/` that run
with the full authority of the agent**. We are the only system in this brief
where that artifact could instead be *owned, signed, priced, sandboxed, and
carry an attested track record of whether it actually worked*.

**The novel primitive that falls out of it — and that nothing in the sweep has —
is the execution receipt.** Because rustlite cartridges are deterministic wasm
and their bytes are content-addressed, a call can emit
`keccak(lib_hash ‖ fn ‖ canonical_args ‖ result ‖ fuel ‖ status)` signed by the
caller. Buzz signs *messages*, ERC-8004 signs *opinions*, x402 signs *payments*,
A2A signs *capability claims* — **nothing in the field signs a computation.**
It is the only reputation primitive here that a sybil cannot manufacture, because
manufacturing it means actually executing the code and paying for it. And it is
exactly the "outcome-weighted ranking" the SoK paper says no one has.

**The discipline that keeps this from becoming crypto theater** — and the
research is blunt that this is the default failure mode (0.67% of 10,000 ERC-8004
agents have a service endpoint; one client wrote 66% of all feedback) — is a
single test: **does the primitive pay rent at N=1, on one machine, with no
counterparty?** Receipts at N=1 are a deterministic regression-test corpus for
our own cartridges. Per-export pricing at N=1 is cost accounting we don't have.
Graders-as-cartridges at N=1 are the eval harness that every prompt-ablation
above requires. Build those. **Do not** build the discovery marketplace, the
reputation leaderboard, the governance surface, or the agent directory — those
need counterparties, and those are the ones that stay hollow.

---

## 8. What to stop

- **Stop adding rustlite syntax prose to the system prompt.** arXiv 2606.10933
  says the model's coping strategy for an unfamiliar language is to write a
  generator; give it a scratch workspace and honest compile errors instead.
- **Ablate the prompt at every model pin.** ALE-Claw and Cherny independently
  measured that most of it is dead weight. We have a 12-persona QA fleet and a
  metered budget — that is a controlled ablation rig; use it and measure.
- **Don't build MCP Roots, Sampling, or Logging.** Formally deprecated 2026-07-28.
- **Don't chase A2A/AP2/UCP/ACP-commerce.** The card rails require a merchant of
  record. x402 under the LF is the one we are already on.
- **Don't migrate off the diamond onto ERC-8004 registries.** Publish an
  8004-shaped registration as a cheap hedge; the registries are hollow.
- **Reconsider ACP-over-stdio in the browser** — there are no subprocesses in a
  tab. ACP belongs in the CLI, where we already have a binary.

---

## 9. The one concrete commercial move

**Implement ACP in the `localharness` CLI.** It is a small, well-specified
JSON-RPC surface (a dozen agent-side methods), there is an **official Rust SDK**,
and it is the single mechanism by which a harness becomes visible: it puts us in
the ~38-entry ACP agent registry alongside Cursor, Cline, Goose, Copilot,
OpenHands, Junie, **OpenClaw and Hermes** — and it is exactly what Buzz's
`buzz-acp` bridge consumes, which is the "compatible harness list" in practice.
It is also the cheapest possible distribution: we stop competing for stars
against a 384k-star repo and instead become reachable from every editor that
already speaks the protocol.

Pair it with the second reachability move — **expose the cartridge store as a
stateless MCP server** under the 2026-07-28 revision, so every `spawn_lib`-callable
published cartridge becomes a tool in Codex, Cursor, Zed and Amp, gated by the
x402 endpoint we already run.

---

## 10. Quarantine — claims we could NOT verify

Do not promote any of these without re-checking. They are here so nobody
re-derives them and assumes they were vetted.

- **Every benchmark leaderboard number in this space.** Terminal-Bench 2.1 and
  SWE-bench Pro figures (incl. "Kimi K3 88.3% vs GPT-5.6 Sol 88.8%") are all
  aggregator-sourced. Terminal-Bench's structure (Stanford × Laude; 1.0 = 80
  tasks, 2.0 = 89, 2.1 current) is confirmed; the scores are not.
- **x402 volume.** Chainalysis's "100M+ cumulative transactions on Base through
  Q1 2026" and x402.org's live 30-day counters (75.41M tx / $24.24M) are not
  reconcilable with the widely-quoted "165M transactions / 69,000 agents". Do not
  cite a single figure with confidence.
- **Cursor Composer 2.5** — no primary source exists that we could find; the
  cursor.com/blog/composer page is the **October 2025** Composer 1 post.
- **Noam Brown's "next model makes harnesses obsolete"** — The Information,
  paywalled, secondary only.
- **The "Claude Code is 98% harness" study** — the citing article 403s; no
  underlying study identified.
- **Star/fork counts** are point-in-time and were mutually inconsistent across
  sources within the same week. GitHub stars are a degraded signal in 2026 —
  cross-check against crates.io downloads and push recency.
- **Antigravity internals**: Knowledge Items format, the agent manager's
  scheduling algorithm (there is *no* public writeup), artifact on-disk layout,
  and credit unit economics are all undocumented.
- **Hermes's public launch date** (2026-02-25 is secondary; only the repo
  creation date 2025-07-22 is hard) and its self-evolution phases 2–4 (documented
  as planned, not verified as implemented).
- **Buzz**: the ACP `PROTOCOL_VERSION` integer, 11 of its 14 custom NIPs, and
  whether `buzz-agent` is built on goose.
- **"67% of respondents run Wasm in production"** — widely repeated, absent from
  webassembly.org's own 2026 post, which cites far smaller Web Almanac numbers.

---

## Primary sources

ACP: [agentclientprotocol.com](https://agentclientprotocol.com/) ·
[registry](https://agentclientprotocol.com/overview/agents) ·
[repo](https://github.com/agentclientprotocol/agent-client-protocol) —
Agent Skills: [agentskills.io](https://agentskills.io) —
Codex: [github.com/openai/codex](https://github.com/openai/codex) —
nanocodex: [github.com/gakonst/nanocodex](https://github.com/gakonst/nanocodex) —
Hermes: [github.com/nousresearch/hermes-agent](https://github.com/nousresearch/hermes-agent) ·
[self-evolution](https://github.com/NousResearch/hermes-agent-self-evolution) —
Buzz: [block.xyz/inside/introducing-buzz-where-humans-and-agents-work-together](https://block.xyz/inside/introducing-buzz-where-humans-and-agents-work-together) —
ALE: [arxiv.org/abs/2606.05405](https://arxiv.org/abs/2606.05405) ·
[harness ablation](https://agents-last-exam.org/blogs/harness-matters) —
Metaprogramming: [arxiv.org/abs/2606.10933](https://arxiv.org/abs/2606.10933) —
SoK Agentic Skills: [arxiv.org/html/2602.20867v1](https://arxiv.org/html/2602.20867v1) —
Skill supply chain: [unit42.paloaltonetworks.com/openclaw-ai-supply-chain-risk](https://unit42.paloaltonetworks.com/openclaw-ai-supply-chain-risk/) —
Kilpatrick: [sequoiacap.com/podcast/google-deepminds-logan-kilpatrick-why-the-model-eats-the-harness](https://sequoiacap.com/podcast/google-deepminds-logan-kilpatrick-why-the-model-eats-the-harness/) —
Amp: [ampcode.com/news](https://ampcode.com/news) —
Cursor: [cursor.com/changelog](https://cursor.com/changelog)
