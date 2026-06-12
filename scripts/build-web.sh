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

# Cache-bust the bundle URLs (see web/boot.js): Chrome's wasm code cache serves
# a stale compiled module for the unchanged wasm URL despite must-revalidate, so
# a redeploy didn't reach returning visitors until a hard reload. Stamp the wasm
# CONTENT HASH into boot.js's LH_BUILD + index.html's boot.js?v= — a new hash
# (only when the wasm actually changes) makes a fresh url = guaranteed cache
# miss. Re-stampable: matches the current value, not a one-shot placeholder.
HASH="$( (sha256sum web/pkg/localharness_bg.wasm 2>/dev/null || shasum -a 256 web/pkg/localharness_bg.wasm) | cut -c1-12 )"
if [ -n "$HASH" ]; then
    sed -i.bak -E "s/const LH_BUILD = \"[^\"]*\"/const LH_BUILD = \"${HASH}\"/" web/boot.js && rm -f web/boot.js.bak
    sed -i.bak -E "s|(boot\.js\?v=)[A-Za-z0-9]*|\1${HASH}|" web/index.html && rm -f web/index.html.bak
    echo "→ stamped bundle cache-buster: ${HASH}"
else
    echo "WARNING: could not hash wasm; bundle cache-buster NOT stamped" >&2
fi

echo "→ web/pkg/ updated. Commit the changes and push for Vercel to pick up."
ls -lh web/pkg
