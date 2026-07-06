# test-fleet — a standing fleet of test-user personas

Twelve **persistent on-chain agent identities**, each a distinct personality,
that dogfood localharness from maximally different angles and file **grounded**
feedback off-chain (`localharness feedback` → the proxy telemetry endpoint → a
GitHub issue in the telemetry repo). The maintainer reads the issues and turns
them into fixes — closing the actor-model feedback loop the platform was built
for.

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
4. **submit** — it files that item via `localharness feedback` (the proxy
   telemetry endpoint files it directly as a GitHub issue — no bridge step).

## Run it

```sh
# A sample (a few personas):
scripts/test-fleet/run-fleet.sh nova-qa pip-qa vex-qa

# The whole fleet:
scripts/test-fleet/run-fleet.sh
```

Read the harvest in the telemetry repo's issues (label `feedback`).

Needs `node` (for JSON parsing) and a built CLI (`cargo build --features
wallet`, or set `LOCALHARNESS_BIN`).

**Cost:** the sponsor's gas — one mint per *new* persona (feedback itself is
off-chain + free). Model calls are metered: the proxy debits ~1 `$LH` per call
(it gates on an active session OR a meter balance covering the cost, 402
otherwise), and the CLI deliberately does NOT auto-open the 10-`$LH`/hr
session. A fresh persona holds 0 `$LH`, so `run-fleet.sh` best-effort funds
each persona from the funded `claude` identity (a missing claude key or a
failed send only warns — already-funded personas keep working). The personas
are persistent, so re-runs only add fresh feedback.
