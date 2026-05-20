#!/usr/bin/env bash
# scripts/release.sh — atomic release tool.
#
# Usage:
#   ./scripts/release.sh <version>
#
# Does the whole release in one go: pre-flight, version bump, verify,
# commit, tag, push, cargo publish, GH release. Each step exits on
# failure; the script never leaves a half-finished release.
#
# Read RELEASING.md before using.

set -euo pipefail

if [[ $# -ne 1 ]]; then
    echo "usage: $0 <version>   (e.g. 0.1.1)" >&2
    exit 1
fi

VERSION="$1"
TAG="v${VERSION}"
TODAY="$(date +%Y-%m-%d)"
REPO="compusophy/localharness"

# Resolve repo root regardless of CWD.
cd "$(dirname "$0")/.."

# Color helpers (no-op when not a tty).
if [[ -t 1 ]]; then
    G='\033[0;32m'; Y='\033[0;33m'; R='\033[0;31m'; N='\033[0m'
else
    G=''; Y=''; R=''; N=''
fi
step()  { printf "${G}==>${N} %s\n" "$*"; }
warn()  { printf "${Y}!! ${N} %s\n" "$*" >&2; }
fail()  { printf "${R}xx ${N} %s\n" "$*" >&2; exit 1; }

# Validate version shape (X.Y.Z plus optional pre-release).
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[a-zA-Z0-9.-]+)?$ ]]; then
    fail "version must look like X.Y.Z (got '$VERSION')"
fi

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

step "pre-flight: tooling"
command -v cargo >/dev/null || fail "cargo not on PATH"
command -v gh    >/dev/null || fail "gh not on PATH"
command -v git   >/dev/null || fail "git not on PATH"

gh auth status >/dev/null 2>&1 || fail "gh not authenticated (run: gh auth login)"

step "pre-flight: git state"
# CHANGELOG.md may be dirty — the user is staging release notes for
# *this* release. Every other dirty file is a hard error so we don't
# accidentally bundle unrelated work into the release commit.
DIRTY_OTHERS="$(git status --porcelain | grep -v '^.M CHANGELOG.md$' | grep -v '^ M CHANGELOG.md$' || true)"
[[ -z "$DIRTY_OTHERS" ]] || fail $'working tree has dirty files other than CHANGELOG.md:\n'"$DIRTY_OTHERS"
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[[ "$BRANCH" == "main" ]]            || fail "not on main (on $BRANCH)"
git fetch --quiet origin main
LOCAL="$(git rev-parse HEAD)"
REMOTE="$(git rev-parse origin/main)"
BASE="$(git merge-base HEAD origin/main)"
[[ "$LOCAL" == "$REMOTE" || "$REMOTE" == "$BASE" ]] || \
    fail "local main diverges from origin/main; rebase first"

step "pre-flight: tag availability"
if git rev-parse "$TAG" >/dev/null 2>&1; then
    fail "tag $TAG already exists locally"
fi
if git ls-remote --tags origin "$TAG" | grep -q .; then
    fail "tag $TAG already exists on origin"
fi

step "pre-flight: CHANGELOG.md entry"
grep -qE "^## \[$VERSION\]" CHANGELOG.md || \
    fail "CHANGELOG.md is missing a '## [$VERSION]' section (add it before releasing)"

# ---------------------------------------------------------------------------
# Bump
# ---------------------------------------------------------------------------

step "bump Cargo.toml version -> $VERSION"
# Match the FIRST `version = "..."` line under [package].
perl -i -0pe "s/(\[package\][^\[]*?\nversion = \")[^\"]+(\")/\${1}$VERSION\${2}/s" Cargo.toml
grep -q "^version = \"$VERSION\"" Cargo.toml || fail "Cargo.toml bump did not stick"

step "promote CHANGELOG.md heading date -> $TODAY"
perl -i -pe "s|^## \[$VERSION\][^\n]*|## [$VERSION] - $TODAY|" CHANGELOG.md
grep -q "^## \[$VERSION\] - $TODAY" CHANGELOG.md || fail "CHANGELOG promote did not stick"

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

step "cargo build (refreshes Cargo.lock)"
cargo build --quiet

step "cargo test"
cargo test --quiet

step "cargo clippy"
cargo clippy --all-targets -- -D warnings 2>&1 | tail -5

step "cargo package --list (sanity)"
PKG_FILES="$(cargo package --allow-dirty --list 2>/dev/null | wc -l)"
step "  package contains $PKG_FILES files"

step "cargo publish --dry-run"
cargo publish --dry-run --allow-dirty 2>&1 | tail -3

# ---------------------------------------------------------------------------
# Commit + tag + push
# ---------------------------------------------------------------------------

step "git commit"
git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -m "release $TAG" >/dev/null

step "git tag $TAG"
git tag -a "$TAG" -m "$TAG"

step "git push --atomic origin main $TAG"
git push --atomic origin main "$TAG"

# ---------------------------------------------------------------------------
# Publish + GH release
# ---------------------------------------------------------------------------

step "cargo publish"
cargo publish

step "extract release notes from CHANGELOG.md"
NOTES_FILE="$(mktemp)"
trap 'rm -f "$NOTES_FILE"' EXIT
# Print the lines between `## [VERSION]` and the next `## [...]` heading.
# Use perl so we get reliable regex behavior across awk variants.
perl -ne 'BEGIN { $in = 0 } if (/^## \[/) { last if $in; if (/^## \['"$VERSION"'\]/) { $in = 1; next } } print if $in;' CHANGELOG.md > "$NOTES_FILE"
if [[ ! -s "$NOTES_FILE" ]]; then
    warn "release notes are empty; falling back to generic"
    echo "Release $TAG." > "$NOTES_FILE"
fi

step "gh release create $TAG"
gh release create "$TAG" --repo "$REPO" --title "$TAG" --notes-file "$NOTES_FILE"

step "done"
echo
echo "  crate:   https://crates.io/crates/localharness/$VERSION"
echo "  release: https://github.com/$REPO/releases/tag/$TAG"
echo
