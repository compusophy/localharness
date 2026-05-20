#!/usr/bin/env bash
# Diff the upstream Python SDK against the commit we currently track.
#
# Usage:
#   ./scripts/sync-upstream.sh                 # diff vs. upstream HEAD
#   ./scripts/sync-upstream.sh <ref>           # diff vs. <ref>
#
# Does NOT modify the working tree. Prints:
#   * commit log between pinned and target
#   * --stat for google/antigravity/
#   * suggested next steps

set -euo pipefail

UPSTREAM_URL="https://github.com/google-antigravity/antigravity-sdk-python.git"
PINNED_COMMIT="$(grep -E '^\| Pinned commit' UPSTREAM.md | awk -F '`' '{print $2}')"
TARGET_REF="${1:-HEAD}"

if [[ -z "$PINNED_COMMIT" ]]; then
  echo "error: could not parse pinned commit from UPSTREAM.md" >&2
  exit 1
fi

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT

echo "==> cloning $UPSTREAM_URL"
git clone --quiet --filter=blob:none "$UPSTREAM_URL" "$SCRATCH/upstream"
cd "$SCRATCH/upstream"

git fetch --quiet origin "$PINNED_COMMIT" 2>/dev/null || true
TARGET_SHA="$(git rev-parse "$TARGET_REF")"

if [[ "$PINNED_COMMIT" == "$TARGET_SHA" ]]; then
  echo "==> upstream unchanged; pinned commit matches $TARGET_REF ($TARGET_SHA)"
  exit 0
fi

echo
echo "==> commits in upstream since pinned ($PINNED_COMMIT..$TARGET_SHA)"
git log --oneline "$PINNED_COMMIT..$TARGET_SHA" || true

echo
echo "==> files changed under google/antigravity/"
git diff --stat "$PINNED_COMMIT..$TARGET_SHA" -- google/antigravity/ || true

echo
echo "==> changed file list"
git diff --name-status "$PINNED_COMMIT..$TARGET_SHA" -- google/antigravity/ || true

cat <<EOF

==> next steps
  1. Review the diff above.
  2. Port the relevant changes into src/ of this repo.
  3. Update the 'Pinned commit' line in UPSTREAM.md to $TARGET_SHA.
  4. Refresh the vendored snapshot at google/antigravity/ to match.
  5. cargo test && cargo clippy --all-targets && cargo run --example smoke
  6. Commit: "sync upstream $(echo $TARGET_SHA | cut -c1-8)"
EOF
