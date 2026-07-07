#!/bin/sh
# scripts/install.sh — one-line install of the prebuilt `localharness` CLI.
#
#   curl -fsSL https://raw.githubusercontent.com/compusophy/localharness/main/scripts/install.sh | sh
#
# Downloads the latest prebuilt binary for your platform from the GitHub release
# (built by .github/workflows/binaries.yml) and installs it to ~/.local/bin
# (override with LH_INSTALL_DIR). No Rust toolchain, no compile — the "under 60
# seconds" onboarding path (telemetry #62/#82). Prebuilt assets exist from the
# first release cut AFTER binaries.yml landed; before that, fall back to
# `cargo install --path .` or the browser at localharness.xyz.
set -eu

REPO="compusophy/localharness"
DIR="${LH_INSTALL_DIR:-$HOME/.local/bin}"
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)
    case "$arch" in
      x86_64|amd64) asset="localharness-linux-x86_64.tar.gz" ;;
      *) echo "no prebuilt Linux binary for $arch — build from source: cargo install --path ." >&2; exit 1 ;;
    esac ;;
  Darwin)
    case "$arch" in
      arm64|aarch64) asset="localharness-macos-arm64.tar.gz" ;;
      *) echo "no prebuilt macOS binary for $arch (Intel) — build from source: cargo install --path ." >&2; exit 1 ;;
    esac ;;
  *)
    echo "unsupported OS '$os' for this installer." >&2
    echo "On Windows: download the .zip from https://github.com/$REPO/releases/latest" >&2
    echo "Or build from source: cargo install --path ." >&2
    exit 1 ;;
esac

url="https://github.com/$REPO/releases/latest/download/$asset"
echo "downloading $asset …"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
if ! curl -fsSL "$url" -o "$tmp/lh.tar.gz"; then
  echo "download failed ($url)." >&2
  echo "no prebuilt release yet? build from source: cargo install --path ." >&2
  exit 1
fi
tar -C "$tmp" -xzf "$tmp/lh.tar.gz"
mkdir -p "$DIR"
install -m 0755 "$tmp/localharness" "$DIR/localharness"

echo "installed → $DIR/localharness"
case ":$PATH:" in
  *":$DIR:"*) : ;;
  *) echo "note: $DIR is not on your PATH — add it, e.g.  export PATH=\"$DIR:\$PATH\"" ;;
esac
echo "run:  localharness --help"
