# Upstream tracking — historical record

## tl;dr

`localharness` **was** a port of Google's [`google-antigravity`][upstream]
Python SDK. As of **2026-05-20** it is no longer a port: the 0.2.x line
replaces the Go `localharness` runtime binary with a Rust-native agent
loop that talks to the Gemini API directly. We diverge from upstream by
design.

See [`DESIGN.md`](DESIGN.md) for the Rust-native runtime plan.

## What we ported from (0.1.x)

| Field          | Value |
|----------------|-------|
| Repository     | `google-antigravity/antigravity-sdk-python` |
| Pinned commit  | `d6be9ca366ed9f8b58b0b21ab672053c847a0f4d` |
| Pinned date    | 2026-05-20 |
| Reviewed by    | initial port |

The 0.1.x Rust source mirrored that commit. We deliberately stopped
shipping the vendored Python tree in this repository because:

1. The 0.2.x runtime no longer depends on the Go harness, so parity
   with the Python client is no longer the goal.
2. The upstream lived in `google/` and made the repo look like a
   Python project to GitHub Linguist and to humans skimming the tree.
3. The Python source is still public at the upstream repository if
   anyone needs it for reference.

## When upstream changes

We may still glance at upstream for design ideas — naming, hook
ordering, edge cases the Go binary handles — but **we do not promise
behavioral parity**. The Rust crate's behavior is defined by its own
tests and by `DESIGN.md`.

To peek at upstream:

```sh
git clone --depth=1 https://github.com/google-antigravity/antigravity-sdk-python /tmp/antigravity-python
```

## License attribution

The `LICENSE` file at the repo root is Apache-2.0, identical to
upstream's. The 0.1.x Rust code was a derivative work; 0.2.x is
inspired-by rather than ported, but we keep the Apache-2.0 license for
continuity and because attribution costs nothing.

[upstream]: https://github.com/google-antigravity/antigravity-sdk-python
