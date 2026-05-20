# Upstream tracking

This Rust port is a translation of the **Python** SDK published by Google
at:

> https://github.com/google-antigravity/antigravity-sdk-python

## Pinned upstream commit

| Field          | Value |
|----------------|-------|
| Repository     | `google-antigravity/antigravity-sdk-python` |
| Pinned commit  | `d6be9ca366ed9f8b58b0b21ab672053c847a0f4d` |
| Pinned date    | 2026-05-20 |
| Reviewed by    | initial port |

The Rust SDK in `src/` was ported against the Python source at that exact
commit. The vendored Python snapshot under `google/` matches this commit
verbatim and is kept in the tree as a reference.

## Checking for upstream changes

Run the sync script:

```sh
# Linux / macOS / git-bash
./scripts/sync-upstream.sh

# Windows PowerShell
./scripts/sync-upstream.ps1
```

The script clones upstream into a scratch directory, fetches the latest
commit on the default branch, and prints a diff summary against the pinned
commit. **It does not modify your working tree.** Use the output to scope
the porting work required to advance the pin.

## Promoting a new pin

1. Run the sync script and review the diff.
2. Decide whether to port the changes wholesale, partially, or wait.
3. Once the Rust source reflects the new upstream state, replace the
   `Pinned commit` and `Pinned date` in this file with the new SHA / date.
4. Update the vendored Python snapshot under `google/` to match.
5. Commit with a message like `sync upstream <short-sha>`.

## What we do *not* track

The following upstream directories are reference material and not part of
the published crate (see `Cargo.toml` `exclude`):

- `.kokoro/` — Google's release infrastructure.
- `pyproject.toml`, `skills/` — Python packaging.
- `examples/getting_started/`, `examples/deep_dives/`, `examples/resources/`
  — Python examples. Rust examples live in `examples/test_agent.rs`.

The `LICENSE` (Apache-2.0) is preserved at the root — required for
attribution since this is a derivative work.
