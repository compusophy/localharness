# Autonomous loop — the localharness agent immune system

Status: design. Nothing here is built yet; this names the gap and the smallest
buildable first increment. The vision is a feedback loop that runs **without a
human bridging it**.

## The loop today (human in the middle)

The pieces already exist; a human is the wire between them.

```
human  ──drives──▶  test-agent (localharness call / browser chat)
human  ──reads──▶   its findings
human  ──runs──▶    localharness feedback "<finding>"   (on-chain write)
human  ──reads──▶   localharness feedback               (on-chain read)
human  ──triages──▶ decides what to fix
human  ──writes──▶  the code change
```

Concretely, every leg is already a real code path:

- **Exercise.** `localharness call <target> <msg>` (`src/bin/localharness.rs`)
  runs a headless agent turn through the credit proxy under the target's
  on-chain persona. The browser equivalent is `chat.rs::run_send` driving
  `stream_turn` with the full tool surface (`create_subdomain`,
  `create_and_publish_app`, `run_cartridge`, filesystem, …).
- **Report.** `feedback_submit` → `registry::submit_feedback_sponsored` writes
  the finding to the on-chain append-only `Entry[]` (FeedbackFacet). Sponsored,
  so the agent holds zero funds.
- **Read.** `feedback_read` → `registry::list_feedback()` returns
  `Vec<FeedbackEntry { sender, timestamp, text }>`, newest first.
- **Triage / fix.** Human only.

The loop is *already closed through the chain* — the on-chain feedback log is
the durable message bus. What is missing is the **autonomous drivers** at each
end: something that fires the exercise on a trigger, something that exercises the
platform with *real execution tools* (not a pure conversational turn), and
something that reads the log and synthesizes work.

## The honest gap: the headless call path has NO tools

This is the load-bearing constraint and the reason an autonomous QA agent is not
just "run `call` on a timer".

`localharness call` deliberately sets:

```rust
// src/bin/localharness.rs — call()
let caps = CapabilitiesConfig {
    enabled_tools: Some(Vec::new()),   // ← zero builtins
    enable_subagents: false,
    ..Default::default()
};
```

with a comment that says exactly why: *"a remote prompt must not read the
CALLER's filesystem"*. The headless `call` is a **conversational** turn — it can
think and reply but cannot *do* anything. Likewise `agent_rpc.rs` routes an
incoming `lh-agent-call` straight into `agent.chat(message)` — again, only
whatever tools that subdomain's session was started with, and a remote caller
must not get to drive the *callee's* destructive tools.

So an autonomous QA fleet cannot reuse the `call` path as-is: a turn with no
tools can describe a bug but cannot reproduce one. It can't register a subdomain,
publish a cartridge, hit the proxy under a fresh identity, or fuzz the rustlite
compiler. **An autonomous QA agent needs a real, scoped execution tool surface
that the conversational `call` path intentionally withholds — and a sandbox that
makes granting it safe.**

The rest of this doc designs that surface, the fleet that uses it, the triage
agent that consumes the log, the safety model, and the finding→fix arc.

---

## 1. A test-agent on a trigger that actually exercises the platform

### 1a. The trigger

Reuse `triggers.rs` verbatim — it already gives us interval and (via a custom
`Trigger` impl) event-driven firing, on both native and wasm:

```rust
use localharness::triggers::{every, Trigger, TriggerContext};
use std::time::Duration;

// Interval: probe every 30 min.
let qa = every(Duration::from_secs(1800), "qa-smoke", |ctx: TriggerContext| async move {
    ctx.send_when_idle("Run the smoke suite and report findings.").await
});
```

`send_when_idle` already respects the one-turn-at-a-time discipline that
`run_send`/`TURN_ACTIVE` enforce, so a QA tick never races a live turn. For
event-driven probes (e.g. "a new facet was just cut", "a new subdomain was just
registered") implement `Trigger` directly and poll the chain in `run()`:

```rust
struct OnNewFacet { last_seen: Mutex<u64> }
#[async_trait] impl Trigger for OnNewFacet {
    fn name(&self) -> &str { "on-new-facet" }
    async fn run(&self, ctx: TriggerContext) -> Result<()> {
        loop {
            sleep_ms(60_000).await;
            let n = registry::facet_count().await?;        // DiamondLoupe
            if n != *self.last_seen.lock() {
                *self.last_seen.lock() = n;
                ctx.send("A facet changed on the diamond. Re-run the on-chain probe.").await?;
            }
        }
    }
}
```

The trigger fires a *prompt*; the agent's *tools* are what make it exercise the
platform. That tool surface is the real work.

### 1b. The QA execution tool surface (the missing half)

A new capability bundle — call it `qa_tools` — granted ONLY to fleet agents
running under the autonomous harness, NEVER to a `localharness call` turn or an
`?rpc=1` callee. Each tool wraps a kitchen-sink command, captures structured
output, and never touches anything outside the sandbox subtree.

Native (the fleet runs as a `localharness probe` subcommand — see §6):

| Tool | Wraps | Captures |
|------|-------|----------|
| `qa_create(name)` | `registry::claim_and_maybe_set_main_sponsored` under a **throwaway** sandbox key | tx hash, owner-verify result, latency |
| `qa_compile(source)` | `rustlite::compile` | ok/err, byte size, error string, panic-or-clean |
| `qa_publish(name, source)` | compile + `submit_tempo_sponsored` (setMetadata) | tx hash, gas used vs. estimated, revert reason |
| `qa_call(target, msg)` | the existing headless `call` (proxy round-trip) | reply text, HTTP status, latency, hint class from `hint_for_call_error` |
| `qa_fetch(url)` | GET a public face (`<name>.localharness.xyz`, `llms.txt`, `skill.md`) | status, body hash, content-type, byte size |
| `qa_chain(method, params)` | a **read-only** JSON-RPC call (`owner_of_name`, `list_feedback`, loupe) | decoded result, RPC error |
| `qa_report(text)` | `submit_feedback_sponsored` under the **fleet's own** identity | tx hash |

The tools return JSON the agent can reason over (`{ ok, latency_ms, gas, error,
… }`) so the model does the *triage of its own run* — "the publish reverted with
OOG at 1.2M+8500/byte for a 9KB app" is a far better finding than "publish
failed". `qa_report` is the only on-chain *write to the feedback log*; it carries
a structured envelope (see §3) so the triage agent can parse, not just read prose.

The kitchen-sink "smoke suite" the interval prompt asks for is just the agent
calling these in sequence: create → compile a known-good cartridge → publish →
fetch the public face → call itself → report. Each specialist (below) narrows to
one column.

### 1c. The sandbox

Three nested fences, because granting *execution* is the whole risk:

1. **A throwaway identity per run.** `qa_create` mints names under keys written
   to a `./.qa-sandbox/<run-id>/` dir, never the operator's real
   `*.localharness.key`. Names are prefixed (`qa-<run>-<n>`) and **released**
   (`release_name_sponsored`) in a `finally` at run end — the fleet cleans up
   after itself. A leaked sandbox key controls only disposable junk names.
2. **A filesystem jail.** The native QA tools run with a `Filesystem` rooted at
   `./.qa-sandbox/<run-id>/` (the `Filesystem` trait already abstracts this —
   hand the agent a `NativeFilesystem` pointed at the jail, not `.`). This is
   why the conversational `call` path withholds tools entirely; the QA path
   *adds them back but behind the jail*.
3. **A spend ceiling + a kill switch.** Every sponsored tool checks a per-run
   budget (count of on-chain writes) and a wall-clock deadline before
   submitting; exceeding either aborts the run and files a `budget-exceeded`
   finding. `triggers.rs` shutdown / `TURN_CANCEL` already give cooperative
   cancellation; the autonomy dial (§4) is the master switch above all of it.

Net: the QA agent can *do real things*, but only to disposable identities, only
inside a jailed FS, only within a budget, and only when the dial is ON.

---

## 2. A fleet of specialists

One generalist smoke agent finds shallow bugs. A *fleet* — each agent a narrow
persona over a subset of `qa_tools` — finds deep ones. Each is just a persona +
a `qa_tools` subset + a trigger cadence. Personas are published on-chain
(`set_persona`) so the fleet is itself inspectable via `whoami`.

| Agent | Surface it probes | Tools | Example findings |
|-------|-------------------|-------|------------------|
| **`qa-security`** | trust boundaries | `qa_call`, `qa_chain`, `qa_fetch` | RPC accepts a spoofed `from`; an `?rpc=1` callee runs a destructive tool for a remote caller; markdown/HTML XSS in a face; sponsor key reachable |
| **`qa-fuzz`** | the rustlite compiler + wire decoders | `qa_compile`, `qa_publish` | source that panics the compiler instead of erroring; a cartridge that compiles but has no `frame`/`render` export; bytes that crash the wasm loader; SSE frame edge cases |
| **`qa-ux`** | the agent's own affordances | `qa_call`, `qa_fetch` | a tool that 405s where the help text implies HTTP; `(os error 2)` leaking to users; an empty-response turn with no explanation; confusing error copy |
| **`qa-perf`** | latency + gas | `qa_publish`, `qa_call`, `qa_chain` | publish gas estimate off by 6× (the exact bug CLAUDE.md records); proxy p95 latency regressions; a turn that loops to `MAX_AUTO_CONTINUATIONS` without progress |

Each specialist runs on its own `every(...)` cadence (security hourly, fuzz
continuous-with-backoff, perf daily). They share the sandbox machinery from §1c
and report through the same structured `qa_report` envelope, tagged with their
persona so triage can weight by source. Adding a fifth specialist is: write a
persona string, pick a tool subset, register a trigger — no new infra.

This is the "immune system" metaphor made literal: a standing population of
cheap, specialized probes, each watching one surface, continuously, reporting to
a shared log.

---

## 3. The triage agent

A distinct agent (`qa-triage`) whose only input is the on-chain feedback log and
whose only output is a ranked work-list. It does NOT exercise the platform; it
reads what the fleet (and humans, and other agents) reported.

```rust
// Inputs: the durable on-chain bus.
let entries = registry::list_feedback().await?;   // Vec<FeedbackEntry>
```

To make triage parseable rather than prose-grovelling, fleet findings use a
lightweight envelope inside the 2048-byte feedback `text`:

```
qa/v1 source=qa-fuzz sev=high surface=rustlite
compile panic on `enum E{}` with no variants — expected a clean Err.
repro: qa_compile("enum E{}")
```

The triage agent:

1. **Reads** the whole log (`list_feedback`), parses `qa/v1` envelopes, keeps
   free-form human entries as `sev=unknown`.
2. **Dedups + clusters** — N reports of the same `surface`+repro collapse into
   one item with a count (recurrence = signal).
3. **Ranks** by `severity × recurrence × surface-criticality` (security >
   on-chain-correctness > compiler > ux > perf, tunable in the persona).
4. **Emits** a ranked work-list: a markdown table, and (so the loop stays
   on-chain) a single `qa_report` with `source=qa-triage sev=meta` summarizing
   the top 3 — a durable, queryable "current state of the platform's health".

Because `list_feedback` reads contract *state* (not `cast logs`), triage has no
block-window limit and sees the full history every run. The triage agent's
output is what a human (or, at higher autonomy, a fix agent) picks up next.

---

## 4. The safety model

The dividing line: **probing is cheap and reversible; fixing and destroying are
not.** Autonomy is granted in exactly that order, and defaults OFF.

### Scoped keys

- **Fleet identity** — its own `*.localharness.key`, used to sign `qa_report`
  feedback and proxy auth. Spends only the fleet's own `$LH`. Cannot touch the
  operator's real names (different key, and `setMetadata`/`release` are
  owner-gated on-chain).
- **Per-run sandbox keys** — disposable, jailed, released at run end (§1c). The
  fleet's *destructive surface is confined to names it minted this run.*
- **The sponsor key stays the sponsor key.** The fleet pays fees through the
  same embedded AlphaUSD sponsor; it never gains the diamond-owner key (not in
  the repo) and so can never `diamondCut`, `adminResetAll`, or touch another
  holder's assets. The reset-proof boundary already in CLAUDE.md holds.

### The autonomy dial — defaulted OFF

A single setting (`./.qa-autonomy` / a `--autonomy` flag / an OPFS
`.lh_autonomy` file) with three rungs, each a strict superset of the last:

| Rung | Default | The fleet may… |
|------|---------|----------------|
| `observe` | **yes (this is OFF)** | run **read-only** tools (`qa_chain`, `qa_fetch`, `qa_compile`) and `qa_report`. No on-chain writes except feedback. |
| `exercise` | no | additionally run sandboxed *writes* — `qa_create`/`qa_publish`/`qa_call` against disposable names, within budget. |
| `propose` | no | additionally let a fix agent open a PR (§5). Never merges. |

`observe` IS off in the sense that matters: the platform's "OFF" state still lets
probes look and report, because looking is free and safe; it just can't *change
chain state* beyond appending a note. Flipping to `exercise`/`propose` is a
deliberate operator act, never inferred.

### What an autonomous agent must NEVER do unattended

Mirrors the existing typed-confirmation convention (`release_subdomain` requires
`confirmation == name`, never auto-filled; the system prompt forbids inventing
it). For the fleet:

- **Never release/burn a non-sandbox name.** `qa_*` tools refuse any name not
  matching the `qa-<run>-` prefix it minted this run. There is no autonomous
  path to `release_name` on a real identity — that still requires a human typing
  the exact name. The sandbox cleanup only releases its own prefix.
- **Never call `diamondCut`, `adminResetAll`, `adminBurnNames`, or any
  owner-gated admin.** The fleet doesn't hold the key; even if it did, these are
  on the permanent never-unattended list.
- **Never auto-merge a fix.** `propose` opens a PR; a human merges. The
  finding→fix arc (§5) stops at "proposed".
- **Never spend beyond the per-run budget**, and **never run with the dial at
  `observe` doing a write.** Both are hard refusals, not nudges.
- **Never exfiltrate** outside the jail: the QA `Filesystem` is rooted at the
  sandbox subtree, so a confused or adversarial probe can't read the operator's
  real keys or other origins' OPFS.

The rule from CLAUDE.md generalizes cleanly: *destructive / irreversible actions
require a typed confirmation that is never auto-filled.* The fleet's entire
design is to keep its autonomous actions **non-destructive and reversible**
(disposable identities, jailed FS, append-only reports) so the typed-confirmation
gate is never something it tries to pass — it operates entirely below that line.

---

## 5. From a finding to a code change (the feedback→fix arc)

The arc the human bridges today, decomposed so each leg can be automated
independently and the dangerous last step stays human:

```
finding (qa/v1 on-chain)
   │  list_feedback()
   ▼
triage  ──ranks──▶  work-item { surface, sev, repro, recurrence }
   │
   ▼  (dial=propose)
fix-agent
   ├─ reproduces the repro locally inside the jail (e.g. qa_compile(repro))
   ├─ reads the relevant source (grounded: it knows the repo tree from CLAUDE.md / llms.txt)
   ├─ writes a focused diff + a regression test that encodes the repro
   ├─ runs `cargo test` + the wasm guardrail check
   └─ opens a PR (gh) titled from the finding, body linking the on-chain entry
   ▼
human  ──reviews + merges──▶  (NEVER auto-merged)
```

Key properties:

- **The repro is the spec.** A `qa/v1` finding carries a machine-runnable repro
  (`qa_compile("enum E{}")`). The fix agent's first act is to *reproduce it* —
  no repro, no fix; it kicks the item back to the fleet for a better repro
  instead of guessing. This is the same "verify before explaining" discipline
  the project already insists on.
- **Every fix ships a test.** The regression test encodes the repro so the loop
  can't regress — mirrors the existing guards (`builtin_tool_schemas_have_no_union_types`,
  `cartridge_has_entry`). A fix with no test is incomplete.
- **The PR closes the on-chain loop.** Its body references the `FeedbackEntry`
  (sender + timestamp) so a merged fix is traceable back to the autonomous probe
  that found it. A future facet could even mark an entry resolved on-chain.
- **Merge stays human.** Per §4, `propose` is the ceiling. The agent does
  everything up to the irreversible act and stops — exactly the convention used
  for `release_subdomain`.

The first three legs (probe → triage → propose-with-test) are fully automatable
on the existing substrate. The loop runs unattended; the human's role shrinks
from *bridge* to *reviewer of proposed diffs*.

---

## Why this fits the existing substrate

- **No new server.** The on-chain feedback log is the bus; the proxy is the only
  off-chain piece and it already exists. The fleet is `triggers.rs` + tools +
  the same `registry`/`tempo_tx` paths the CLI uses.
- **No new trust assumption.** Sponsor key, owner-gated `setMetadata`, and the
  diamond-owner boundary are unchanged. The fleet's blast radius is disposable
  names inside a jail.
- **Reuses every primitive.** `every()`, `Trigger`, `CapabilitiesConfig`,
  `ClosureTool`, `submit_feedback_sponsored`, `list_feedback`,
  `claim_and_maybe_set_main_sponsored`, `release_name_sponsored`,
  `hint_for_call_error`, the `Filesystem` jail — all already shipped.
- **Honest about the gap.** The conversational `call`/`rpc` path has no tools by
  design; this design *adds a separate, jailed, dial-gated execution surface*
  rather than weakening that path. The two never overlap.
