# design/architecture-analysis — what to learn from other agentic harnesses

> **STATUS: proposal / open.** Comparative analysis, not a roadmap. Each axis ends
> with a *candidate for localharness* — a concrete, code-tied improvement to
> consider, or an honest "not worth it / conflicts with our constraints." Nothing
> here is committed; the maintainer picks what (if anything) gets built.
>
> On-chain feedback #17: "maximally learn (not blindly copy)" from other harnesses
> — Hermes, Claude Code/CLI, Codex, OpenClawd (openclaude), and **antigravity**
> (the harness localharness originally formed from; not re-checked since the early
> port). Threads in feedback #13 (on-the-fly skills), #14 (meta-tools
> create/enable/fork), #16 (public lessons harvest).

## Confidence convention

Claims about *other* harnesses are marked **(inferred)** when I'm reasoning from
their public shape rather than from a checked source. localharness-side claims cite
real modules (`src/…`, `file:line`). Where I don't know, I say so — I do not invent
specifics. The value is the candidate column, not the comparison column.

The five reference harnesses, briefly:

- **Claude Code / Codex** — terminal coding agents. Hooks, slash-command/skill
  files, permission allowlists, subagents, MCP, an `AGENTS.md`/`CLAUDE.md` memory
  convention. Best-documented; most of the comparisons below are highest-confidence
  against these two.
- **antigravity** — the Python agent SDK localharness was originally ported from
  (`Connection`/`Content`/`Step`/`ToolCall` shapes, the policy precedence table in
  `src/policy.rs:6-15`, the `start_subagent` framing). We have NOT diffed against it
  since the port; everything attributed to it is **(inferred from our own port
  lineage)** and a re-check is itself a candidate (see §10).
- **Hermes, OpenClawd (openclaude)** — I have low confidence on internals. Treated
  as "open-source agent harnesses in the same space"; specifics marked **(inferred,
  low confidence)** and not leaned on.

---

## 1. Backend pluggability / model-agnosticism

| | approach |
|---|---|
| **localharness** | `Connection` / `ConnectionStrategy` trait seam (`src/connections/mod.rs:32-60`); Agent/Conversation depend only on the traits. Gemini + Anthropic + OpenAI + Mock + (local Gemma) backends. `$LH` proxy routes either provider. Seam proven by construction (`design/model-agnostic.md`). |
| **Claude Code** | Single-vendor (Anthropic), but reaches other models via MCP servers / proxy gateways rather than a first-class backend trait. **(inferred)** |
| **Codex** | Primarily OpenAI; model choice is config, not a pluggable transport. **(inferred)** |
| **antigravity** | Gemini-shaped; our seam was *added* during the Rust port — the Python SDK was less abstracted here. **(inferred from port lineage)** |

**Candidate for localharness:** mostly DONE and ahead of peers — the seam is the
crate's strongest asset. Small win: a **difficulty router** (cheap model for routine
turns, strong model for hard ones) is unbuilt and sits naturally at the
`ConnectionStrategy` layer — already named as Phase D in `model-agnostic.md`. Worth
it. Don't copy the single-vendor convenience the others get away with; our
multi-backend story is the differentiator.

## 2. Tool definition ergonomics (+ feedback #14 meta-tools)

| | approach |
|---|---|
| **localharness** | `Tool` trait (`src/tools.rs:33-42`): name/description/`input_schema`/`execute`. `ClosureTool` (`src/tools.rs:188`) for inline closures + `with_state`. Builtins are backend-neutral structs in `src/builtins/`. Schemas are hand-written JSON (Gemini rejects unions → guard test). |
| **Claude Code / Codex** | Tools mostly fixed/native + extended via **MCP**; users don't hand-author JSON schemas per tool — the server declares them. Skills/slash-commands are markdown, not code. **(inferred)** |
| **antigravity** | Same `Tool`/schema shape we ported. **(inferred)** |

Feedback #14 asked specifically about **meta-tools: create / enable / fork**.
Honest assessment of where we already are:

- **enable** — EXISTS. `configure_agent` (`src/builtins/configure_agent.rs`) lets an
  agent rewrite its own tool allowlist + system prompt into `agent.json`; takes
  effect next session. The allowlist machinery is real (`src/app/tool_allowlist.rs`,
  `GOLDEN` tools can't be disabled).
- **create** — does NOT exist as a runtime primitive. An agent cannot define a *new*
  tool from inside a turn. The nearest thing is `create_and_publish_app` (compile a
  rustlite cartridge), which is capability creation but not a callable tool.
- **fork** — does NOT exist. `start_subagent` (`src/builtins/start_subagent.rs`) is
  one-shot, text-only, *shares* the parent client/model, no tool dispatch — it
  delegates, it does not fork a configured agent.

**Candidate for localharness:** **enable is done; create/fork are the real gap.**
A `create` meta-tool would be a derive-schema-from-a-closure-or-rustlite-fn path
feeding the existing `Tool` trait — meaningful but bounded by the no-arbitrary-codegen
posture (a created tool must be a *cartridge* or a curated builtin, not eval'd JS).
A `fork` meta-tool (clone this agent's `agent.json` into a new subdomain identity) is
genuinely valuable and composes with on-chain identity (§9) + scheduling — recommend
it over `create`. Schema ergonomics (a derive-macro for `input_schema`) is a nice
SDK polish but low-urgency.

## 3. Safety / permission gates

| | approach |
|---|---|
| **localharness** | Two layers. (a) `DecideHook` / `PreToolCallDecideHook` (`src/hooks.rs:148`, first-deny-wins) + the declarative `Policy` precedence table (`src/policy.rs:6-15`, specific-DENY > specific-ASK > … > wildcard-APPROVE) + `workspace_only`. (b) the typed-confirm gate (`src/confirm.rs` + `chat::confirm_guard`) — DISPATCH-layer, denies the first call, issues a single-use code bound to exact args, retry only runs if the code appears in the latest USER message. |
| **Claude Code** | Permission modes + an allow/deny/ask rule list, persisted; hooks (`PreToolUse` etc.) can block. Conceptually the same shape as our policy table — we ported the precedence idea. **(inferred, high confidence)** |
| **Codex** | Sandboxed exec + approval prompts for risky actions. **(inferred)** |
| **antigravity** | Source of our policy precedence table. **(inferred from lineage)** |

**Candidate for localharness:** our confirm-gate (random code in the user message,
model-echo rejected) is arguably *stronger* than a plain allow/deny prompt because it
defeats the model self-approving — worth keeping, don't replace it. Two honest gaps
the peers cover that we don't: (1) **no execution sandbox** — we have no `run_command`
jail comparable to Codex's; on wasm this is moot (no native exec) and on native it's a
real gap if untrusted agents ever run locally. (2) coverage: per `design/README.md`,
`spend_treasury` / `execute_proposal` move funds gated only by the holder key, not the
confirm gate — closing that is cheap and already flagged. Recommend #2 now, #1 only if
native-multi-tenant becomes a real use case.

## 4. Context management / compaction

| | approach |
|---|---|
| **localharness** | Recency-weighted incremental **fold** (`src/backends/compaction.rs`): one rolling-summary turn + a raw keep-window; each compaction re-summarizes only `(prior summary + newly-aged delta)`, never the whole history; synthetic prefix stays ONE turn. Tagged (`COMPACTION_TAG`) so the next fold recognizes it. One generic engine, thin per-backend adapters. |
| **Claude Code** | Auto-compaction at a context threshold + a manual `/compact`; also leans on persistent memory files (`CLAUDE.md`) so the *durable* facts live outside the window. **(inferred)** |
| **Codex** | Similar threshold-triggered summarization. **(inferred)** |

**Candidate for localharness:** our fold is solid and the amortized-cost property is
better-engineered than a naive re-summarize. The thing peers do that we under-use:
**push durable facts OUT of the window into persistent memory** rather than
summarizing them repeatedly. We have the substrate (lessons on-chain, persona,
OPFS) but no "extract durable fact → memory, drop from window" path. Candidate:
a compaction hook that promotes recurring facts into `.lh_lessons`/persona before
they get summarized away. Medium value. A two-tier "deep summary" fold is documented
as deliberately deferred — agree, not worth it yet.

## 5. Multi-agent orchestration & subagents

| | approach |
|---|---|
| **localharness** | Two distinct mechanisms. (a) In-process: `start_subagent` (`src/builtins/start_subagent.rs`) — one-shot, text-only, shares client/model, NO tool dispatch, no recursion/fan-out (explicitly "0.4.x work"). (b) Cross-agent over the network: `call_agent` / `ask_agent` with x402 payment, `scheduleChildJob` recursion (depth-capped, budget from parent escrow), bounty/party/guild coordination on-chain. |
| **Claude Code** | First-class **subagents** with their own context, system prompt, tool allowlist, and model; parent delegates and gets a result back. Parallel fan-out is native. **(inferred, high confidence)** |
| **Codex** | Task delegation; less documented to me. **(inferred)** |

**Candidate for localharness:** **our weakest in-process axis.** `start_subagent` is
markedly thinner than Claude Code's subagents — text-only and tool-less means a
subagent can't actually *do* work, only reason. The high-value, low-controversy
upgrade: give a subagent its own tool allowlist + the existing dispatch pipeline
(reuse `backends::dispatch.rs` + `ToolRunner`), so it can run fs/builtin tools in an
isolated context and return a real result. This is the single most impactful "learn
from Claude Code" item in this doc. Note we already have something peers *lack* — the
cross-agent, paid, on-chain orchestration layer (§8) — so the recommendation is to
strengthen the *local* subagent, not rebuild the distributed one.

## 6. Memory / lessons (+ feedback #16 public lessons harvest)

| | approach |
|---|---|
| **localharness** | Self-recorded **lessons loop** (`src/lessons.rs`): one short lesson per real error/correction, dedup + last-10 × 240ch + 2000B cap, persisted on-chain under `keccak256("localharness.lessons")` + OPFS, folded into EVERY system prompt (browser, CLI, scheduler). Plus `set_persona` self-edit. "Dreaming" consolidation rewrites them. |
| **Claude Code** | `CLAUDE.md` / memory files — durable, human-editable, in-repo, loaded every session. Static-ish; not auto-distilled from errors. **(inferred, high confidence)** |
| **Codex** | `AGENTS.md` convention, similar. **(inferred)** |

Our lessons loop is *more* automated than the peer memory-file convention (it
distills from real failures and is bounded), and it's on-chain — which sets up #16.

**Candidate for localharness (feedback #16 = public lessons harvest):** lessons are
already on-chain per-identity under a known key, so a **public harvest is mostly a
read + aggregate**, not new infrastructure — enumerate identities, read each
`localharness.lessons` blob, surface a shared/curated lessons board (and optionally
let an agent opt-in-import another's lessons). High value, low cost, plays to our
unique on-chain substrate. Honest caveat: needs a quality/abuse filter (a public
lessons pool is a prompt-injection surface — never auto-import untrusted lessons into
a system prompt; same caution as `set_persona`). Recommend building it gated behind
explicit opt-in.

## 7. Skills / dynamic capability (feedback #13 = on-the-fly skills)

| | approach |
|---|---|
| **localharness** | No "skill" primitive. Closest analogs: rustlite cartridges (`create_and_publish_app`) = published capability; `configure_agent` = toggle existing tools; persona/lessons = behavioral steering. None are a *loadable instruction-bundle* the agent picks up mid-task. |
| **Claude Code** | **Skills** = markdown capability bundles (instructions + optional scripts/resources) discovered and loaded on demand; slash-commands similar. This is exactly the "on-the-fly skill" feedback #13 is pointing at. **(inferred, high confidence)** |
| **Codex** | Prompt/command files, comparable. **(inferred)** |

**Candidate for localharness:** **a genuine gap and a good fit.** A skill could be a
plain markdown/text blob stored in OPFS and/or on-chain under a
`keccak256("localharness.skill.<name>")` key, surfaced to the agent as a
loadable instruction section (the same mechanism `lessons::compose_section` already
uses to fold text into the prompt). No new wire types, no codegen — it's "named,
loadable prompt fragments," which our prompt-composition path (`session.rs`,
`system_prompt.rs`) already supports structurally. This composes with #16 (publish a
skill on-chain, others discover it). Recommend it — arguably higher-leverage than the
`create` meta-tool because it needs no execution model. Caveat: same injection caution
as lessons/persona for any *imported* skill.

## 8. Payment / economic model (localharness-unique)

| | approach |
|---|---|
| **localharness** | x402 EIP-712 settlement in `$LH` (`X402Facet`), agent-to-agent; per-message metering (`CreditMeterFacet`); sponsored writes via Tempo native AA so users hold nothing; bounty/party/guild/DAO economy rungs; scheduled jobs escrow `$LH`. The credit proxy is the one off-chain piece. |
| **Claude Code / Codex / antigravity / Hermes** | **None.** These are local developer tools — there is no inter-agent payment, no metering rail, no escrow. Cost is the user's API bill, full stop. **(high confidence — it's a category difference, not a feature gap)** |

**Candidate for localharness:** nothing to *learn* here — this axis is ours alone and
there's no peer to copy. The honest note is the inverse: it's also our biggest source
of *complexity and risk* (the security-audit and money-review docs exist for a
reason). The "learn from peers" lesson is one of restraint — peers stay simple by NOT
having an economy; we accept the complexity deliberately because the economy IS the
product (agents that pay each other). Keep it; don't let payment plumbing leak into
the SDK core (it correctly lives behind `feature="wallet"` today — preserve that).

## 9. Persistence & identity

| | approach |
|---|---|
| **localharness** | OPFS per-origin (8 fs builtins over the `Filesystem` trait, `EncryptedFilesystem` at rest), conversation history in OPFS, and — uniquely — **on-chain identity**: every subdomain is an NFT, MAIN identity + 6551 TBA, multi-device via QR seed-adoption, persona/lessons/skills as on-chain metadata. Identity is global truth, not a local file. |
| **Claude Code / Codex** | Local filesystem + project config; "identity" is the user's machine + API key. No portable, network-global agent identity. **(inferred, high confidence)** |
| **antigravity** | In-process session state; persistence was the app's job. **(inferred)** |

**Candidate for localharness:** like §8, this is a category difference in our favor —
nothing to copy. One genuinely transferable idea from the peers: their persistence is
**dead simple and human-inspectable** (a file you can `cat` and edit). Our on-chain +
OPFS + encrypted-at-rest stack is powerful but opaque to debug. Candidate: a single
"inspect my durable state" surface (persona + lessons + skills + allowlist in one
readable view) — a debuggability win, not an architecture change. Low cost, real QoL.

## 10. Re-check antigravity (the parent harness)

Cross-cutting, and the one item flagged explicitly in #17: localharness was ported
from antigravity and **we have not diffed against it since.** Everything attributed to
antigravity above is inferred from our own port lineage, which is exactly the blind
spot. The Python SDK has presumably moved (new tool/hook/multi-agent ergonomics,
possibly its own skills/memory conventions).

**Candidate for localharness:** a one-time **re-diff of the current antigravity SDK**
against our `Connection`/`Tool`/`Hook`/`Policy` surface — find what the upstream
learned that we froze at port time. This is research, not code; cheapest item here and
it de-risks every other inference in this doc. Recommend doing it first.

---

## Summary — candidates ranked by (value ÷ cost)

1. **On-the-fly skills (#13)** — loadable named prompt fragments in OPFS/on-chain,
   reusing `lessons::compose_section`-style prompt folding. No new execution model.
   High value, low cost. (§7)
2. **Real subagents** — give `start_subagent` its own tool allowlist + the existing
   dispatch pipeline so it can actually *do* work. Our weakest in-process axis vs
   Claude Code. (§5)
3. **Public lessons harvest (#16)** — read + aggregate already-on-chain lessons into
   an opt-in shared board; mostly a read path. (§6)
4. **`fork` meta-tool (#14)** — clone `agent.json` into a new on-chain identity;
   composes with identity + scheduling. (Skip `create` unless cartridge-backed.) (§2)
5. **Re-diff antigravity** — research, not code; de-risks every inference above; do
   it first. (§10)

Lower-tier / honest "not now": execution sandbox (wasm makes it moot; §3),
difficulty router (already Phase D; §1), durable-fact-promotion in compaction (§4),
inspect-state surface (QoL; §9), schema derive-macro (polish; §2). And explicitly
**don't copy** the peers' single-vendor simplicity (§1) or their lack of an economy
(§8) — those are our deliberate differentiators.
