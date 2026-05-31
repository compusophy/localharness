#!/usr/bin/env bash
# Build the localharness browser-app wasm bundle into web/pkg/ for
# Vercel. The app code lives inside the main `localharness` crate
# behind the `browser-app` feature; wasm-pack drives it as a cdylib.
#
# Usage:
#   ./scripts/build-web.sh
#
# After running, commit the updated web/pkg/* artefacts and push —
# Vercel serves the static `web/` directory verbatim. The build is local
# (Vercel itself does no Rust compilation).

set -euo pipefail

cd "$(dirname "$0")/.."

if ! command -v wasm-pack >/dev/null 2>&1; then
    echo "wasm-pack not on PATH. Install: cargo install wasm-pack" >&2
    exit 1
fi

# Stamp the crate version into web/llms.txt so the deployed bundle
# advertises its freshness (curl llms.txt | head). Keeps it from drifting
# from Cargo.toml without a manual bump step.
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
if [ -n "$VERSION" ]; then
    sed -i.bak -E "s/^\*\*version:\*\* .*/\*\*version:\*\* ${VERSION} (stamped from Cargo.toml by build-web; matches crates.io when the deployed bundle is current)/" web/llms.txt
    rm -f web/llms.txt.bak
    echo "→ stamped llms.txt version: ${VERSION}"
fi

echo "→ wasm-pack build (release, browser-app)..."
wasm-pack build . \
    --target web \
    --out-dir web/pkg \
    --release \
    --no-default-features \
    --features browser-app

echo "→ web/pkg/ updated. Commit the changes and push for Vercel to pick up."
ls -lh web/pkg
