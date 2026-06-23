# scripts — build / release / feedback tooling subsystem spec

> Module-owned context (auto-loaded when an agent works in `scripts/`). Operational
> tooling. The gotchas below have each cost a real incident.

## Release: `release.{sh,ps1}` is ATOMIC — don't hand-run the steps
Pre-flight → version bump → cargo verify → commit → tag → push → cargo publish → GH
release, in one shot. Pre-flight runs `gen-docs -- --check` so a version bump CAN'T
ship stale docs. On a mid-way failure consult `RELEASING.md`; don't hand-fix.
- **It commits ONLY `Cargo.toml`/`Cargo.lock`/`CHANGELOG.md`** — commit everything
  else FIRST or it's left out of the release.
- ⛔ **PS5.1 stderr trap**: `release.ps1` wraps native cmds in `Invoke-Native`
  (PowerShell 5.1 turns cargo/git/gh stderr into a TERMINATING error) — never call
  `cargo`/`git`/`gh` directly in it. And a `"` inside a here-string commit message
  shreds PS5 native-arg quoting into pathspecs — keep `"` OUT of messages.
- Per the maintainer: a commit is NOT a release (no auto version bump); the default
  is commit + push + `vercel` deploy. Run release scripts only when asked.

## `build-web.{sh,ps1}` — the web bundle
Regenerates `gen-docs` + `gen-feedback-resolutions` → `wasm-pack build` (release,
browser-app,mainnet) → STAMPS the `?v=` cache-buster into boot.js/index.html (see
`web/CLAUDE.md`). wasm-opt is DISABLED (bundled wasm-opt rejects post-MVP features).

## Feedback tooling — use the RIGHT chain
- `check-feedback.mjs` (node, view-function loop) reads on-chain feedback on BOTH
  chains. **MAINNET is live; the testnet 274 is STALE** (pre-migration). Use THIS.
- ⛔ `harvest-feedback.{sh,ps1}` point at the WRONG (testnet) chain + need cast —
  DON'T use them; `check-feedback.mjs` is the replacement.
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

## Doc integrity
`gen-docs` (cargo bin) fills the GEN blocks in `web/skill.md`/`llms.txt` from
`src/docs_manifest.rs`; `--check` is drift-only (release pre-flight + a cargo test
enforce it). NEVER hand-edit a GEN block — change the manifest + regen.
