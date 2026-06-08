# test-fleet — a standing fleet of test-user personas

Twelve **persistent on-chain agent identities**, each a distinct personality,
that dogfood localharness from maximally different angles and file **grounded**
feedback on-chain (the `FeedbackFacet`). The maintainer reads the harvest and
turns it into fixes — closing the actor-model feedback loop the platform was
built for.

## The fleet

| name | tests the platform as… |
|------|------------------------|
| `nova-qa` | impatient 10x power-user — speed, friction, perf |
| `pip-qa`  | confused first-timer — onboarding clarity |
| `vex-qa`  | security-paranoid adversary — auth, trust, edge cases |
| `iris-qa` | visual artist — aesthetics, layout, craft |
| `dex-qa`  | SDK developer — API ergonomics, docs, examples |
| `sol-qa`  | hype early-adopter — ambitious feature requests |
| `mara-qa` | skeptic — value proposition, "why this over X?" |
| `kit-qa`  | mobile-only user — mobile / partition-class friction |
| `ada-qa`  | accessibility — keyboard, screen reader, contrast |
| `rho-qa`  | verbose reporter — detailed, structured items |
| `zed-qa`  | terse minimalist — one-line items, sparse input |
| `juno-qa` | chaos tester — weird input, robustness gaps |

Each persona (its on-chain system prompt) + the real task it runs lives in
[`personas.json`](personas.json).

## How a run works (per persona)

`run-fleet.sh` drives each selected persona through, reusing the existing CLI —
**no new server, no new infra**:

1. **create** it on-chain with its persona (`localharness create … --persona`;
   idempotent — an existing persona is reused, not re-minted).
2. **probe** — it sends its task to a live agent (`localharness call`): a *real*
   interaction with a real response and real latency.
3. **reflect** — it reasons *in persona* about that **actual** experience and
   writes exactly one `[BUG]` / `[FEATURE]` / `[FEEDBACK]` item. Feedback is
   anchored to the real probe + reply — never hallucinated.
4. **submit** — it files that item on-chain (`localharness feedback`).

## Run it

```sh
# A sample (a few personas):
scripts/test-fleet/run-fleet.sh nova-qa pip-qa vex-qa

# The whole fleet:
scripts/test-fleet/run-fleet.sh

# Read the harvest:
scripts/harvest-feedback.sh        # or:  localharness feedback
```

Needs `node` (for JSON parsing) and a built CLI (`cargo build --features
wallet`, or set `LOCALHARNESS_BIN`).

## Bridge the feedback to GitHub issues

The first rung of *agents filing their own issues*: surface the on-chain feedback
as GitHub issues on the repo so it's tracked + actionable.

```sh
node scripts/test-fleet/feedback-to-issues.mjs           # DRY RUN — prints what it'd file
node scripts/test-fleet/feedback-to-issues.mjs --create  # actually file them (needs `gh` authed)
```

It reads `localharness feedback --json`, skips entries already filed (dedup
ledger `docs/feedback-bridged.txt`, keyed on `<timestamp>:<sender>`), classifies
each (`[BUG]`→`bug`, `[FEATURE]`→`enhancement`, `[FEEDBACK]`→`feedback`, all
`from-fleet`), and opens an issue carrying the full text + on-chain submitter +
timestamp. **Dry-run by default; `--create` is opt-in** — creating public issues
is outward-facing. Idempotent, so it's safe to wire into a cron/CI later. **Cost:** the sponsor's AlphaUSD gas — one mint + one
feedback write per *new* persona (reused personas pay only the feedback write).
Model calls are free in the beta (a `$LH` session opens automatically for any
identity). The personas are persistent, so re-runs only add fresh feedback.
