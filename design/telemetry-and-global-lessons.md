# Off-chain telemetry, rich feedback & global lessons

The thesis: **on-chain stays small and public; rich data goes off-chain via
GitHub.** The `FeedbackFacet` is a great gas-filtered, public, durable *task
list* — but 2048 bytes can't hold a conversation, app state, or a stack trace,
and gas is the wrong tax for telemetry. So pair it with an **off-chain layer**
(the proxy + the colony GitHub bridge, the one server we already run) for the
big, texty, sometimes-private payloads.

```
ON-CHAIN  FeedbackFacet            short public feedback = the task list (stays)
          lessons slot (per agent) the agent's own folded-in lessons (stays)

OFF-CHAIN (proxy → GitHub bot)     ← NEW
  1. auto error reports   error + redacted context (convo, tool, state)
  2. rich feedback        submit_feedback + an off-chain attachment (full context)
  3. global lessons       harvest every agent's lessons → curate/filter → upstream
```

Reuses what exists: the **proxy** (`proxy/api/*`), the **colony bridge**
(`scripts/colony/sync-issues`, `issue-to-bounty`, `settle-on-merge`), the
**`compusophy-bot`** write collaborator, and **`GET /api/lessons`** (already a
public harvest of on-chain agent lessons). Likely a **separate repo**
(`localharness-telemetry`) so noise/PII stays out of the code repo's issues.

## Phase 1 — auto error reporting (the firmest ask)

When a turn hits a REAL, unexpected failure — a tool call that errors, a backend
4xx/5xx or empty/timeout response, a caught Worker panic (the brick watchdog) —
the app auto-submits ONE report to the proxy, which files it (deduped) to the
telemetry repo via the bot.

- **Captured:** error message + class, agent name/tokenId, model, the failing
  tool + args, the last N turns, route/app-state. NOT expected/ user-facing
  states (402 out-of-credits, a normal `finish`).
- **Redaction is non-negotiable:** strip the seed/`.lh_wallet`, api keys,
  `0x…`-private material before it leaves the device. Redact in the BROWSER, not
  the proxy.
- **Dedup + rate-limit:** hash the error signature (class + tool + top frame);
  report once per signature per session/day. Silent, best-effort — never blocks
  or annoys the user.
- **Consent:** on by default WITH redaction, with a clear admin toggle
  (`telemetry: on/off`). It's the platform learning from its own users' real
  failures — the highest-signal improvement loop we have.

This is distinct from `record_lesson` (the agent DELIBERATELY noting a lesson):
auto-error-reporting fires WITHOUT the model deciding, so we catch the failures
the model didn't even notice.

## Phase 2 — rich feedback

`submit_feedback` keeps writing the short public note on-chain (the task list),
but ALSO POSTs the full context (conversation + app state) off-chain to the
telemetry repo, linked by the on-chain index. The short version stays the
scannable queue; the rich version is one click away for triage.

## Phase 3 — global lessons (with filters)

Today lessons are per-agent (on-chain slot, folded into that agent's prompt).
To make the platform improve from EVERYONE's lessons:

1. **Harvest** all agents' lessons (`/api/lessons` already does the read).
2. **Tag + filter** — not every lesson is global. Bucket by scope:
   `global` (true platform facts/gotchas) vs `use-case` (cartridge-dev,
   trading, content, …) vs `agent-local` (don't upstream). Tagging can be
   AI-assisted (a periodic consolidation pass) with human review for the
   `global` bucket — the gate that keeps junk out of everyone's prompt.
3. **Upstream** the curated `global` set into the DEFAULT system prompt (a
   "lessons the whole platform learned" section), and expose per-use-case packs
   an agent can opt into. The colony flywheel already merges PRs; a curated
   `global-lessons.md` in the telemetry repo is just another reviewed artifact.

Net: a self-improving platform where one agent's hard-won lesson (and every
agent's silent failure) makes every future agent better — gated, filtered, and
off-chain where the rich data belongs.
