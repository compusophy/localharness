# design/ab-testing — A/B testing with agentic personas

> **STATUS: v1 SHIPPED (CLI `abtest`); browser + on-chain rounds open.** The
> minimal win — run ONE prompt across N variants (models and/or on-chain
> personas) and print the answers side-by-side from a single command — is live
> as `localharness abtest` (`src/bin/localharness/abtest.rs`). It reuses the
> exact headless turn (`call::run_agent_turn`) every other agent-to-agent path
> uses, so it adds NO new on-chain surface, no new proxy route, and no new model
> plumbing. What remains open is the richer end of the spec below: a browser
> compare-view, a judge/scoring round, and persisting an A/B run on-chain.

## Why

"A/B testing with agentic personas" (#22) is the question every agent builder
hits the moment they have more than one option: *given a prompt, which
**variant** answers best?* A variant is one of two orthogonal axes the platform
already exposes:

- **Model** — the same persona answered by Gemini vs Claude vs GPT. The
  `--model` flag on `call`/`mcp-call` already routes a turn to any of the three
  through the credit proxy (one signed token, no provider key). The model
  selector and `localharness models` enumerate the valid ids.
- **Persona** — different on-chain system prompts (a `<name>.localharness.xyz`
  agent's published `persona`) answering the SAME question. `call <name>` already
  embodies a target's published persona for one headless turn.

Today you compare by running `call` (or the browser chat) N times by hand,
scrolling between answers, and eyeballing the difference. That is exactly the
manual A/B loop. The job is to make it ONE command / ONE view that fans the same
prompt across the variants and lays the answers out together.

This is deliberately NOT a new subsystem. Every primitive already exists:
- `call::run_agent_turn(key, target, message, history, model)` runs one headless
  turn as a target persona on a chosen model and returns the reply text. It is
  already shared by the CLI `call` and the MCP server.
- `colony.rs` already fans a prompt across a *panel* of agents (the judges) and
  aggregates — A/B is the same fan-out without the bounty/escrow lifecycle.
- The proxy already meters/charges each turn; an N-variant run is just N metered
  turns billed to the caller, no new billing path.

So the whole feature is an **orchestrator over `run_agent_turn`**: pick the
variant axis, run the turn per variant (sequentially, so the meter/x402 nonce
rules hold — each turn is its own one-shot request), capture `(variant, reply)`,
and present them. Pure plumbing; the hard parts (auth, billing, persona
embodiment, model routing) are reused verbatim.

## The model

An **A/B run** = one prompt × a set of variants → a set of labelled answers,
optionally scored.

```
abtest <prompt> --models <a,b,c>            # same persona, N models
abtest <prompt> --personas <x,y> [--model m] # N personas, one model
```

- **Variant** = `(persona, model)`. The two flags pick which axis varies; the
  other is held fixed (the default persona = the caller's own identity / a
  generic assistant; the default model = the platform default Gemini).
- **Fan-out** = run `run_agent_turn` once per variant, **sequentially**. This is a
  load-bearing constraint, not laziness: each turn carries a ONE-SHOT x402 nonce
  valid for exactly one request (see `call.rs` INVARIANT), so variants must not
  share a connection or replay a nonce. Sequential N-turns = N independent
  one-shot requests, each metered once. (A future parallel mode would need a
  fresh nonce per turn — out of scope for v1.)
- **Capture** = `Vec<(label, Result<reply, error>)>`. A failed variant (RPC
  flake, unfunded, a Claude id on a non-`anthropic` build) does NOT sink the run
  — it is reported in place so the other variants still produce a comparison.
- **Present** = the answers side-by-side under their variant labels. v1 is a
  terminal report (each variant a header + its reply); the browser view (open)
  is the same data in two/three columns.

### Cost + billing

An A/B run costs the caller **N metered turns** — exactly N ordinary `call`s.
There is no discount and no new escrow: A/B is a convenience orchestrator, and
each variant is billed by the proxy the same way a standalone `call` is. The CLI
prints the variant count up front so the spend is never a surprise (mirrors
`--pay auto` printing the resolved price before running).

### Fresh history per run

A/B comparison must be **apples-to-apples**: every variant answers the SAME
prompt from the SAME starting context. So an A/B run always uses a FRESH turn
(no persisted `call` history seeded in) — otherwise variant B would answer with
A's thread bleeding in, and the comparison would be meaningless. v1 passes
`prior_history = None` to every `run_agent_turn`; it does not persist the run's
threads back (a comparison is a one-shot experiment, not an ongoing
conversation).

## What shipped (v1, CLI)

`localharness abtest <prompt> [--as <me>] [--model <id>]... [--models a,b,c] [--persona <name>]... [--personas x,y]`

- **Model A/B** — `--models gemini-3.5-flash,claude-opus-4-8` (or repeated
  `--model`) runs the prompt under the caller's own persona on each model and
  prints the answers under model-id headers. The single-variant axis is the
  model; the persona is held at the caller's identity (or a generic assistant if
  the caller has no published persona).
- **Persona A/B** — `--personas alice,bob` (or repeated `--persona`) runs the
  prompt embodying each named agent's on-chain persona on ONE model (the
  `--model` default or override) and prints the answers under persona headers.
- **Capture-and-continue** — each variant's reply (or its error + the standard
  `hint_for_call_error` hint) is captured; one bad variant never aborts the run.
- **Pure, testable core** — the arg parse (`parse_abtest_args`), the variant
  expansion (`expand_variants`: the flags → the ordered `Vec<Variant>`), and the
  report rendering (`format_abtest_report`) are pure functions with unit tests,
  the same split `call`/`colony`/`models` use. Only the fan-out loop touches the
  network.

It reuses `run_agent_turn` (the headless turn), `model_backend_tag`-free routing
(the model id IS the variant label), `resolve_caller_key` (the caller's identity
signs + pays), and `hint_for_call_error`/`fmt_lh` (error hints + cost display).
No new on-chain calls, no new proxy route, no new module dependencies.

## Open (the richer end of #22)

- **Browser compare-view.** The same fan-out behind a chat affordance: type a
  prompt, pick 2-3 variants, see the answers in side-by-side columns in the
  unified stream. The data shape is identical to the CLI's `Vec<(label, reply)>`;
  the work is the maud template + the swap, NOT new orchestration. (No new DOM —
  an `inline_result_card`-style multi-column fragment.)
- **A judge/scoring round.** Reuse `colony::colony_judge_prompt` +
  `parse_judge_rating` + `median_rating` to have a NEUTRAL panel score each
  variant's answer 1-5, turning the eyeball comparison into a ranked verdict
  ("variant B wins, median 4 vs 3"). This is the colony's judge machinery pointed
  at A/B answers instead of a bounty result — the cores already exist and are
  tested.
- **Persisting an A/B run on-chain.** Record `(prompt, variants, winner)` so a
  comparison is auditable / shareable — a natural fit for the SessionRoom KV log
  (#22's sibling) rather than a new facet. Lets an agent A/B a persona change
  before committing it via `set_persona`.
- **A/B-driven self-improvement.** Close the loop: an agent A/Bs two candidate
  personas (or two phrasings of its own system prompt) against a battery of
  prompts, the judge round picks the winner, and the agent adopts it via the
  existing `set_persona` / lessons machinery. The A/B run becomes the evidence
  behind a persona edit instead of a blind rewrite.

None of these is required for the v1 win, and each is additive over the shipped
orchestrator — which is why this is a doc with a live minimal core, not a
forced full build.
