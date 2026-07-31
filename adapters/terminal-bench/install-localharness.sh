#!/usr/bin/env bash
# Install localharness inside a Terminal-Bench task container + provision the
# throwaway identity key the runner passed via LOCALHARNESS_KEY.
#
# FAST PATH: download the prebuilt static x86_64 linux binary from the
# `tbench-binary` release (the workflow's `binary` job builds it fresh). A
# cargo compile inside the container overran hello-world's 360s agent timeout,
# so we curl a ready binary (~seconds) and only fall back to compiling if the
# download fails.
set -euo pipefail

# Bare task images may lack curl/ca-certificates.
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update -qq
  apt-get install -y -qq curl ca-certificates
fi

URL="https://github.com/compusophy/localharness/releases/download/tbench-binary/localharness-x86_64-linux"
if curl --proto '=https' --tlsv1.2 -fsSL "$URL" -o /usr/local/bin/localharness; then
  chmod +x /usr/local/bin/localharness
else
  echo "prebuilt download failed — compiling from source (slow)"
  apt-get install -y -qq build-essential pkg-config || true
  if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  fi
  . "$HOME/.cargo/env"
  cargo install localharness --features wallet,native --locked
  ln -sf "$HOME/.cargo/bin/localharness" /usr/local/bin/localharness
fi

# Provision the identity: `work` loads ~/.localharness/keys/<name>.key.
mkdir -p "$HOME/.localharness/keys"
printf '%s\n' "$LOCALHARNESS_KEY" > "$HOME/.localharness/keys/tbench.localharness.key"
chmod 600 "$HOME/.localharness/keys/tbench.localharness.key"

localharness version >/dev/null 2>&1 || true
echo "localharness installed; identity 'tbench' provisioned."
