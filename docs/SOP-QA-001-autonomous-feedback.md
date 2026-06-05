# SOP-QA-001 — Autonomous QA Feedback Procedure

| Document control | |
|---|---|
| **Document ID** | SOP-QA-001 |
| **Title** | Autonomous QA Feedback Procedure (test-agent → on-chain feedback → fix) |
| **Revision** | 1.0 |
| **Effective date** | 2026-06-05 |
| **Process owner** | Maintainer (localharness) |
| **Conforms to** | ISO 9001:2015 §8.5 (production/service control), §9.1 (monitoring/measurement), §10.2 (nonconformity & corrective action) |
| **Supersedes** | — |

## 1. Purpose

To define a repeatable, auditable procedure by which an autonomous agent (the
**test-agent**) exercises localharness, detects nonconformities (defects, UX
friction, broken specs), and records them as **structured, on-chain feedback**
that drives corrective action. This closes the quality loop:
**observe → record → triage → correct → verify** — and makes the loop's evidence
permanent and tamper-evident (the on-chain `FeedbackFacet` log).

## 2. Scope

Applies to all autonomous QA passes run against:
- the `localharness` CLI and SDK (native),
- the browser agent platform (`<name>.localharness.xyz`),
- the on-chain registry surface (read paths; disposable-name write paths only).

**Out of scope:** destructive operations against production identities; any
write that touches a real user's seed, names, or balances. QA writes use
disposable `qa-*` names only (see SOP-QA-001-A, future).

## 3. Definitions

- **Nonconformity** — any behavior that violates the documented spec, leaks an
  internal error to a user, or imposes avoidable friction.
- **qa/v1 envelope** — the structured feedback record format the fleet emits,
  parsed by `triage`. Shape: `qa/v1 source=<id> sev=<n> …` followed by the note.
- **Worst-first** — defects ordered by user impact, highest first.
- **Verifier** — the proof-of-spec gate (`scripts/verify.sh`): native tests +
  wasm32 guardrail + real cartridge instantiate/render/compose. A correction is
  not "done" until the verifier passes.

## 4. Responsibilities

| Role | Responsibility |
|---|---|
| **test-agent** (autonomous) | Execute the QA pass; record nonconformities on-chain via its own signed identity. |
| **Maintainer** | Run `triage`; prioritize; apply corrections; gate every fix on the verifier; mark resolved. |
| **Verifier (`verify.sh`)** | Independent, mechanical confirmation that a correction behaves as specified before release. |

## 5. Procedure

### 5.1 Observe (exercise the system)
1. Act under the dedicated QA identity: `--as test-agent` (never a real user key).
2. Run the deterministic self-checks: `localharness probe --as test-agent`
   (compiles known-good/known-bad cartridges, allowlisted fetches, registry
   reads over provably-existing fns). On any failure the probe emits a `qa/v1`
   envelope.
3. Run the autonomous pass: `localharness probe --deep --as test-agent` — an LLM
   agent explores real CLI/SDK surfaces and reports findings it can substantiate
   from a transcript.

### 5.2 Record (make the nonconformity permanent)
4. Each distinct nonconformity is recorded **worst-first**, one actionable note,
   via `localharness feedback --as test-agent "<note>"` (writes to the on-chain
   `FeedbackFacet`, signed by the test-agent).
5. The note MUST: name the surface + version, state the observed vs expected
   behavior, and be specific enough to act on without re-deriving it. Cite the
   source (e.g. "Source: real CLI transcript, v0.22.0").
6. No defect is recorded twice in the same pass; recurrence across passes is a
   priority signal, not duplication.

### 5.3 Triage (rank and de-duplicate)
7. The Maintainer runs `localharness triage` — dedups and recurrence-ranks the
   on-chain log into a single prioritized worklist.

### 5.4 Correct (fix at the root)
8. For each ranked item, apply the **smallest correction at the root cause**
   (not the symptom). Destructive/irreversible changes require a typed,
   never-auto-filled confirmation.

### 5.5 Verify (independent confirmation) — mandatory gate
9. Run `scripts/verify.sh`. A correction that does not pass the verifier is **not
   released**. On-chain or identity-layer corrections additionally run the opt-in
   `scripts/verify-onchain.sh` (asserts the write actually landed — the silent-OOG
   guard).

### 5.6 Close
10. Release through `scripts/release.sh` (which re-runs the verifier). Record the
    fix in `CHANGELOG.md` referencing the nonconformity. The on-chain feedback
    entry stands as the permanent record that the loop ran.

## 6. Records / evidence

| Record | Location | Retention |
|---|---|---|
| Nonconformity log | On-chain `FeedbackFacet` (`feedbackRange` / `localharness feedback`) | Permanent (append-only) |
| Local mirror | `.lh_feedback.txt` (per origin) | Per device |
| Triage worklist | `localharness triage` output | Regenerated on demand |
| Verification evidence | `verify.sh` / `release.sh` console output | Per release |
| Corrective action | `CHANGELOG.md` + git history | Permanent |

## 7. References

- `src/bin/localharness.rs` — `probe`, `probe --deep`, `feedback`, `triage`.
- `scripts/verify.sh`, `scripts/verify-onchain.sh`, `scripts/release.sh`.
- `contracts/src/facets/FeedbackFacet.sol` — the append-only on-chain log.
- `scripts/harvest-feedback.{sh,ps1}` — off-chain read for triage.

## 8. Revision history

| Rev | Date | Change |
|---|---|---|
| 1.0 | 2026-06-05 | Initial issue. Formalizes the existing observe→record→triage→correct→verify loop. |
