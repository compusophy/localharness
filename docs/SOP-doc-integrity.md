# SOP — doc integrity (single source of truth + drift gate)

The drift-prone FACTS that used to be hand-copied across `web/skill.md`,
`web/llms.txt`, and `README.md` — chain addresses, the crate version, `$LH`
pricing, the agent tool list, the CLI command list — now live in ONE place and
are GENERATED into the docs. A `cargo test` gate and the release pre-flight
make stale docs impossible to ship.

## The pipeline

```
src/docs_manifest.rs          ← THE single source of truth (the facts)
        │  cargo run --bin gen-docs
        ▼
web/skill.md · web/llms.txt · README.md   ← facts live inside GEN marker pairs
        │  cargo test --lib --features wallet   (drift gate)
        │  scripts/release.{sh,ps1}             (pre-flight gen-docs --check)
        ▼
a version bump CANNOT ship with stale docs
```

1. **Facts live in `src/docs_manifest.rs`.** Chain facts are DERIVED from
   `registry::chain::{MAINNET, MODERATO}`; the version from
   `env!("CARGO_PKG_VERSION")`; pricing, the tool list, and the CLI list are the
   one canonical copy held there.
2. **`gen-docs` fills the GEN blocks** in the three managed docs from the
   manifest's `render(key)`.
3. **Never hand-edit text inside a GEN block** — the generator owns it and the
   drift gate rejects the edit. Change the FACT in `docs_manifest.rs`, then
   regenerate.
4. **The gates enforce sync** (below).

## The marker scheme

Each generated fact lives between an HTML-comment marker pair:

```
<!-- GEN:<key> -->
...generated content (owned by gen-docs)...
<!-- /GEN:<key> -->
```

HTML comments are inert in markdown (`skill.md`, `README.md`) and read as clear,
non-rendering delimiters in the plain-text `llms.txt`, so ONE marker style
covers all three docs. An unknown key inside a marker pair is left UNTOUCHED
(forward-compat). Do NOT write the literal opening token `<!-- GEN:` inside
prose/backticks — the generator would try to parse it; refer to "GEN marker
pairs" in prose instead.

**Keys:** `version`, `chain`, `pricing`, `tools`, `cli` (see
`docs_manifest::KEYS`). Each doc embeds whichever it needs; not every doc carries
every key.

## Commands

```sh
# Regenerate every managed doc in place (the normal edit-a-fact workflow):
cargo run --bin gen-docs --features wallet

# Check-only: render in-memory, diff vs the files, exit 1 if ANY block is stale
# (this is the gate the release scripts run, and what CI/`cargo test` mirrors):
cargo run --bin gen-docs --features wallet -- --check

# The drift unit-test (also runs under a normal `cargo test --lib --features wallet`):
cargo test --lib --features wallet docs_manifest::tests::no_doc_drift
```

`gen-docs` is IDEMPOTENT — running it twice is a no-op.

## The gates (the missing piece this system adds)

- **`cargo test` drift-test** (`docs_manifest::tests::no_doc_drift`): renders
  every GEN block in-memory and asserts it EQUALS the committed doc content.
  Fails with `doc drift: run \`cargo run --bin gen-docs\``. Runs under
  `cargo test --lib --features wallet` (the manifest is wallet-gated because it
  reads `registry::chain`).
- **`scripts/build-web.sh`**: runs `cargo run --bin gen-docs` (REGENERATE)
  BEFORE the wasm-pack build, so every deploy ships fresh docs.
- **`scripts/release.sh` + `scripts/release.ps1`**: a pre-flight step runs
  `gen-docs -- --check` and ABORTS the release on any drift, then (after the
  Cargo.toml version bump) reruns `gen-docs` to stamp the new version into every
  GEN:version block, and commits the regenerated docs. So a **version bump
  cannot ship with stale docs.**

## To change a documented fact

1. Edit the value in `src/docs_manifest.rs` (a chain const flows from
   `registry::chain`; pricing/tools/CLI are edited in the manifest's `const`
   tables).
2. Run `cargo run --bin gen-docs --features wallet`.
3. `git diff` to review the regenerated blocks; commit them.

## Notes

- **Why `wallet`-gated:** the manifest reads `registry::chain`, and the
  `registry` module is behind `feature = "wallet"`. `gen-docs` and the drift
  test run under `wallet`.
- **Pricing's runtime SoT is the proxy** (`proxy/api/_prices.ts`). The manifest
  `PRICING_SUMMARY` is the DOC source — keep the two in sync by hand (both are
  small, deliberately-readable tables; the manifest carries a `// SoT mirror`
  note).
- **Tool / CLI lists** are single-sourced as DATA in the manifest today. A
  future enhancement can derive them from the builtin/platform registries and
  the CLI dispatcher; for now single-sourcing the LIST is the win.
