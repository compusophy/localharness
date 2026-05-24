# Releasing `localharness`

The release flow is **one command**. Read this once; then trust the script.

## Invariant

> A release is **one commit, one push, one publish, one GitHub release** —
> in that order, atomically. Never push the version bump as a separate
> commit from the change that justifies the version bump.

## What bumps where

| File | Changes on | Owner |
|------|-----------:|-------|
| `Cargo.toml` `version` | every release | release script |
| `Cargo.lock` | every release (auto) | cargo |
| `CHANGELOG.md` | every release | **you** (write the entry beforehand) |
| `src/app/templates.rs` `"web demo · X.Y.Z"` tag | every release | **you** (commit before running script) |
| `README.md` install line `localharness = "X.Y"` | breaking minor/major only | you |
| `LICENSE` | never | upstream |

The release script **only stages `Cargo.toml` + `Cargo.lock` +
`CHANGELOG.md`**. Anything else that needs to ship with the version
bump — code, templates, docs, RELEASING.md changes, web bundle —
must be committed *before* invoking the script. Mixing the two
breaks the "one commit per release" invariant.

## Pre-release checklist

1. Land all the feature work as normal commits on `main`. Bump the
   `templates.rs` version tag in the same commit as the feature work.
2. Confirm `main` is green: `cargo test && cargo clippy --all-targets`,
   plus `cargo check --no-default-features --features browser-app
   --target wasm32-unknown-unknown` if anything in `src/app/` changed.
3. Decide the next version per [SemVer](https://semver.org):
   - **patch** (`0.10.0 → 0.10.1`): bug fixes, internal refactors, docs,
     UX polish that doesn't move the public API.
   - **minor** (`0.10.x → 0.11.0`): backward-compatible additions.
   - **major** (`0.x.y → 1.0.0`): breaking changes. Before 1.0, breaking
     changes go in a minor bump per cargo convention.
4. Add a `CHANGELOG.md` entry under a new heading (no date — the script
   stamps today's date in):

   ```markdown
   ## [0.10.1]

   ### Added
   - …

   ### Changed
   - …

   ### Fixed
   - …
   ```

   The release script extracts this section verbatim into the GitHub
   release notes.

## Run the release

```sh
# Linux / macOS / git-bash
./scripts/release.sh 0.10.1

# Windows PowerShell
./scripts/release.ps1 -Version 0.10.1
```

The script performs:

1. **Pre-flight** — clean working tree, on `main`, `CHANGELOG.md` has a
   `## [<version>]` entry, `gh` + `cargo` are authenticated.
2. **Bump** — `Cargo.toml` `version = "<version>"`, refresh
   `Cargo.lock`.
3. **Verify** — `cargo test`, `cargo clippy --all-targets -D warnings`,
   `cargo package` dry-run.
4. **Commit** — `release v<version>` with the two version files.
5. **Tag** — annotated `v<version>`.
6. **Push** — `git push --atomic origin main v<version>` (main + tag in
   one push).
7. **Publish** — `cargo publish`.
8. **Release** — `gh release create v<version>` with notes lifted from
   `CHANGELOG.md`.

Each step exits non-zero on failure. The script never proceeds with a
half-finished release.

## Recovery

| Failure point | Recovery |
|---------------|----------|
| Pre-flight | Fix the issue and re-run. No state changed. |
| Bump / verify | `git checkout Cargo.toml Cargo.lock` and re-run. |
| Commit / tag | `git reset --hard HEAD~1 && git tag -d v<version>` then re-run. |
| Push | Tag may or may not be on the remote. `git fetch && git push --tags --force-with-lease` to reconcile. |
| `cargo publish` | Version is consumed permanently. Bump to `<version+1>` and re-run. Yank the old tag if needed: `gh release delete v<version>` + `git push --delete origin v<version>`. |
| `gh release` | Run `gh release create v<version> --notes-file …` manually. The crate is already live; only the GH release is missing. |

## Yanking a release

```sh
cargo yank --version <version>          # crates.io: hide from new deps
gh release delete v<version>            # remove GH release
git push --delete origin v<version>     # remove tag from origin
git tag -d v<version>                   # remove tag locally
```

A yanked crate is still downloadable for anyone with `<version>` in
their `Cargo.lock` — yanking is "discourage", not "delete".
