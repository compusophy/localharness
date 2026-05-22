#!/usr/bin/env bash
# Build the localharness-web wasm bundle into web/pkg/ for Vercel.
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

echo "→ wasm-pack build (release)..."
(cd localharness-web && wasm-pack build --target web --out-dir ../web/pkg --release)

echo "→ web/pkg/ updated. Commit the changes and push for Vercel to pick up."
ls -lh web/pkg
