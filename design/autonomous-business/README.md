# Autonomous Business

An autonomous software-development **business composed of role-agents** — built on
and *for* localharness. Instead of one agent doing everything, distinct agents play
the roles a real company has: **coder, reviewer, PM, executive, accounting, HR, and
marketing**. The marketing function runs perpetually to grow brand awareness.

This directory is the operating workspace + persistent memory for a recurring
30-minute loop (`/loop`, session job). Each tick reads `LEDGER.md` + `BACKLOG.md`,
fans out parallel role-agents on the top priorities, commits to the
`autonomous-business` branch, and appends a ledger entry — so work **compounds**
instead of repeating.

## The thesis (dogfood, two loops)

1. **Be localharness's first serious customer.** Stand the business up as a real
   on-chain guild + treasury + role-agents, priming the platform's economy.
2. **Productize it.** "Spin up a company of agents" becomes a flagship feature
   (`found_company` — turns oggoel's 9 manual steps into one call).

See `STRATEGY.md` for the full role→primitive mapping and the thesis.

## Files

| File | What |
|------|------|
| `STRATEGY.md` | Org-of-agents architecture, role→facet mapping, the thesis, blockers |
| `BACKLOG.md` | Prioritized cross-role queue (the loop pulls from here) |
| `LEDGER.md` | Append-only progress log (one entry per tick) |
| `COMPANY-FEATURE.md` | Design for the `found_company` product feature + build plan |
| `roles/*.md` | 7 role-persona templates (usable as `set_persona` text) |
| `marketing/BRAND.md` | Positioning, voice, audiences, differentiators, taglines |
| `marketing/CONTENT.md` | Ready-to-post drafts (X, Reddit, HN, LinkedIn) + calendar |
| `marketing/GROWTH.md` | Channel playbook, `[AUTO]/[APPROVE]/[NEVER]` matrix, KPIs |
| `marketing/CREDENTIALS.template.md` | Exactly which creds to provide + how, securely |
| `RISKS.md` | ToS/safety guardrails — the loop's hard fences |

## Hard guardrails (from RISKS.md — non-negotiable)

- **Social posting is never a closed loop.** The loop drafts into a review queue;
  it holds **no live-post credentials** and **no merge/deploy/release/owner keys**.
- **No auto-merge to `main`, no deploy, no release, no facet cut.** Work lands on
  the `autonomous-business` branch only.
- **Typed-confirmation gate stays unweakened** for every `$LH`/value move.
- **No `git add -A`** — explicit paths only; never commit secrets.
- **Per-run + per-day budget ceilings**, idempotent ticks (a cron *will* double-fire).
- **Disclosure is law:** every draft post carries AI + material-connection
  disclosure and the platform's native AI label (FTC; EU AI Act Art. 50).

## To put marketing live (the human's part)

The agent can't sign up for accounts (CAPTCHA + phone verification + ToS — faking it
gets banned). The split is **you seed accounts once; the agent runs them forever via
official APIs.** When ready:

1. Create the accounts (Twitter/X, Reddit, + a dedicated marketing email to start;
   Instagram/TikTok later — they need multi-week API review).
2. Generate **scoped API tokens** (not passwords) per `marketing/CREDENTIALS.template.md`.
3. Drop real values in a **gitignored** `.env.marketing` (already covered by
   `.gitignore`'s `.env.*`) — never commit them.

Until then, the marketing role *prepares* assets so they're ready to fire.
