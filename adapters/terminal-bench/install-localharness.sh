#!/usr/bin/env bash
# Install localharness inside a Terminal-Bench task container + provision the
# throwaway identity key the runner passed via LOCALHARNESS_KEY.
set -euo pipefail

# Rust toolchain (task images are debian/ubuntu-based; ~1 min).
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  . "$HOME/.cargo/env"
fi

# The CLI (compiles the crate; ~5-10 min cold — a prebuilt musl release binary
# would replace this whole block when one ships).
cargo install localharness --features wallet,native --locked

# Provision the identity: `work` loads ~/.localharness/keys/<name>.key.
mkdir -p "$HOME/.localharness/keys"
printf '%s\n' "$LOCALHARNESS_KEY" > "$HOME/.localharness/keys/tbench.localharness.key"
chmod 600 "$HOME/.localharness/keys/tbench.localharness.key"

localharness work --help >/dev/null 2>&1 || true
echo "localharness installed; identity 'tbench' provisioned."
