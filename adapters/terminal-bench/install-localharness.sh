#!/usr/bin/env bash
# Install localharness inside a Terminal-Bench task container + provision the
# throwaway identity key the runner passed via LOCALHARNESS_KEY.
set -euo pipefail

# Task images are minimal debian/ubuntu — they may lack curl AND a C linker
# (the first run failed with "curl: command not found" → no cargo → no binary).
# Install the toolchain deps FIRST; build-essential provides `cc` for the final
# link, pkg-config keeps -sys crates happy. rustls means no libssl needed.
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates build-essential pkg-config
fi

# Rust toolchain (~1 min).
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
fi
. "$HOME/.cargo/env"

# The CLI (compiles the crate; ~5-10 min cold — a prebuilt musl release binary
# would replace this whole block when one ships). Symlink onto PATH so the run
# command finds it even if ~/.cargo/bin isn't exported into the tmux pane.
cargo install localharness --features wallet,native --locked
ln -sf "$HOME/.cargo/bin/localharness" /usr/local/bin/localharness

# Provision the identity: `work` loads ~/.localharness/keys/<name>.key.
mkdir -p "$HOME/.localharness/keys"
printf '%s\n' "$LOCALHARNESS_KEY" > "$HOME/.localharness/keys/tbench.localharness.key"
chmod 600 "$HOME/.localharness/keys/tbench.localharness.key"

localharness work --help >/dev/null 2>&1 || true
echo "localharness installed; identity 'tbench' provisioned."
