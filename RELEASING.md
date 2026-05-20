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
| `UPSTREAM.md` | upstream sync only | you |
| `README.md` upstream badge | upstream sync only | you |
| `LICENSE` | never | upstream |
| `PYTHON_README.md` | upstream README change | you (rare) |

`README.md` install line says `localharness = "0.1"` — a caret range, so
patch bumps don't touch it. Only bump it for breaking minor/major
releases.

## Pre-release checklist

1. Confirm `main` is green: `cargo test && cargo clippy --all-targets`.
2. Decide the next version per [SemVer](https://semver.org):
   - **patch** (`0.1.0 → 0.1.1`): bug fixes, internal refactors, docs.
   - **minor** (`0.1.x → 0.2.0`): backward-compatible additions.
   - **major** (`0.x.y → 1.0.0`): breaking changes. Before 1.0, breaking
     changes go in a minor bump per cargo convention.
3. Add a `CHANGELOG.md` entry under a new heading:

   ```markdown
   ## [0.1.1] - 2026-05-20

   ### Added
   - …

   ### Changed
   - …

   ### Fixed
   - …
   ```

   The release script extracts this section verbatim into the GitHub
   release notes.

4. (Upstream sync only) Update `UPSTREAM.md` `Pinned commit`/`Pinned
   date` and the `upstream-XXXXXXX` badge in `README.md`.

## Run the release

```sh
# Linux / macOS / git-bash
./scripts/release.sh 0.1.1

# Windows PowerShell
./scripts/release.ps1 -Version 0.1.1
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

## Upstream sync release flow

When promoting the pinned upstream commit:

1. Run `./scripts/sync-upstream.sh` and review the diff.
2. Port the relevant Rust changes.
3. Update `UPSTREAM.md` `Pinned commit` + `Pinned date`.
4. Update the `upstream-XXXXXXX` badge in `README.md`.
5. Add a `## [<version>]` entry to `CHANGELOG.md` noting the sync.
6. Run the release script as normal.

The version bump for an upstream sync is usually **minor** (0.x → 0.x+1)
because new harness behavior almost always means new SDK surface.
