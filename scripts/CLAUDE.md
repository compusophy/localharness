# scripts — build / release / feedback tooling subsystem spec

> Module-owned context (auto-loaded when an agent works in `scripts/`). Operational
> tooling. The gotchas below have each cost a real incident.

## Release: `release.{sh,ps1}` is ATOMIC — don't hand-run the steps
Pre-flight → version bump → cargo verify → commit → tag → push → cargo publish → GH
release, in one shot. Pre-flight runs `gen-docs -- --check` so a version bump CAN'T
ship stale docs. On a mid-way failure consult `RELEASING.md`; don't hand-fix.
- **It commits ONLY `Cargo.toml`/`Cargo.lock`/`CHANGELOG.md`** — commit everything
  else FIRST or it's left out of the release.
- `release.ps1` is a THIN DELEGATING SHIM over `release.sh` (same pattern as
  `build-web.ps1`/`verify.ps1`) — release logic lives ONLY in release.sh.
- Per the maintainer: a commit is NOT a release (no auto version bump); the default
  is commit + push + `vercel` deploy. Run release scripts only when asked.

## `build-web.{sh,ps1}` — the web bundle
Regenerates `gen-docs` + `gen-feedback-resolutions` → `wasm-pack build` (release,
browser-app,mainnet) → STAMPS the `?v=` cache-buster into boot.js/index.html (see
`web/CLAUDE.md`). wasm-opt is DISABLED (bundled wasm-opt rejects post-MVP features).

## Feedback tooling — use the RIGHT chain
- `check-feedback.mjs` (node, view-function loop) reads on-chain feedback on BOTH
  chains. **MAINNET is live; the testnet 274 is STALE** (pre-migration). Use THIS.
- `harvest-feedback.{sh,ps1}` are THIN DELEGATING SHIMS over `check-feedback.mjs`
  (`--unresolved`/`-Unresolved` map to `--open`). They used to read the FeedbackFacet
  via `cast` pinned to the stale TESTNET diamond — don't reintroduce that.
- `gen-feedback-resolutions.mjs`: `docs/feedback-resolved-*.txt` → 
  `web/feedback-resolutions.json` (the resolved-bell feed). Mark an item resolved =
  add its index to the resolved file + regen + deploy.
- `check-meter.mjs` reads the per-request meter (the smoke-test for "can't send").

## Verify / parity gates (run for the matching change)
- `verify.sh` — the full proof suite (wasm builds, compose wiring, codegen).
- `test-compose-wiring.mjs` / `test-worker-host-parity.mjs` — assert
  `web/cartridge-worker.js` stays in PARITY with `src/compose.rs` + the rustlite host
  ABI. Run after touching either side (see `web/CLAUDE.md`).
- `smoke-cli.sh` (CLI), `test-fleet/` (12 QA personas), `audit-tech-debt.sh`
  (7-stage tech-debt gate).
- `smoke-money.sh` — OPT-IN live money-path smoke (`--as <name> [--spend]`):
  spends REAL $LH on mainnet (stage B ~1 $LH), asserts wei-exact balance
  conservation. NEVER wire into CI; run only with a quiet, funded identity.

## Doc integrity
`gen-docs` (cargo bin) fills the GEN blocks in `web/skill.md`/`llms.txt` from
`src/docs_manifest.rs`; `--check` is drift-only (release pre-flight + a cargo test
enforce it). NEVER hand-edit a GEN block — change the manifest + regen.
