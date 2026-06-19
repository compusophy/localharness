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

# The platform runs on Tempo MAINNET (chain 4217) — built with the `mainnet`
# feature. The bundle embeds NO sponsor key: on mainnet the `fee_payer` half is
# signed server-side by the rate-capped relay (`registry::sponsor_relay`,
# design/cli-mainnet-relay.md §2.2), so there is no LH_MAINNET_SPONSOR_KEY to set
# and no money-moving key to extract from the wasm.

# Regenerate the managed docs (web/skill.md, web/llms.txt, README.md) from the
# single source of truth (src/docs_manifest.rs) BEFORE the wasm build, so every
# deploy ships fresh chain/pricing/version/tool/CLI facts. This replaces the old
# manual llms.txt version stamp — the generator owns every `<!-- GEN:key -->`
# block, including the version line. Needs the `wallet` feature (the manifest
# reads registry::chain). See docs/SOP-doc-integrity.md.
echo "→ gen-docs (regenerate managed docs from src/docs_manifest.rs)..."
cargo run --quiet --bin gen-docs --features wallet

# PRIVACY: rustc embeds the BUILD MACHINE's absolute source paths into panic
# metadata inside the wasm (e.g. C:\Users\<name>\.cargo\registry\...) — the
# published binary leaked the builder's username (surfaced by the on-screen
# panic banner, but present in `strings` of every prior bundle). Remap every
# local prefix to a generic one. Both slash styles: rustc records Windows
# paths with backslashes for local crates and forward slashes elsewhere.
REPO_DIR="$(pwd)"
REPO_DIR_WIN="$(cygpath -w "$REPO_DIR" 2>/dev/null || echo "$REPO_DIR")"
CARGO_DIR="${CARGO_HOME:-$HOME/.cargo}"
CARGO_DIR_WIN="$(cygpath -w "$CARGO_DIR" 2>/dev/null || echo "$CARGO_DIR")"
HOME_WIN="$(cygpath -w "$HOME" 2>/dev/null || echo "$HOME")"
export RUSTFLAGS="${RUSTFLAGS:-} \
 --remap-path-prefix=${REPO_DIR_WIN}=/lh \
 --remap-path-prefix=${REPO_DIR}=/lh \
 --remap-path-prefix=${CARGO_DIR_WIN}=/cargo \
 --remap-path-prefix=${CARGO_DIR}=/cargo \
 --remap-path-prefix=${HOME_WIN}=/home \
 --remap-path-prefix=${HOME}=/home"

echo "→ wasm-pack build (release, browser-app, mainnet)..."
wasm-pack build . \
    --target web \
    --out-dir web/pkg \
    --release \
    --no-default-features \
    --features browser-app,mainnet

# Guard: the published wasm must not contain the builder's username.
USERPART="$(basename "$HOME_WIN" 2>/dev/null || basename "$HOME")"
if [ -n "$USERPART" ] && grep -aq "Users.$USERPART" web/pkg/localharness_bg.wasm; then
    echo "ERROR: build-machine path leaked into web/pkg/localharness_bg.wasm — remap failed" >&2
    exit 1
fi

# Cache-bust the bundle URLs (see web/boot.js): Chrome's wasm code cache serves
# a stale compiled module for the unchanged wasm URL despite must-revalidate, so
# a redeploy didn't reach returning visitors until a hard reload. Stamp the
# CONTENT HASH into boot.js's LH_BUILD + index.html's boot.js?v= — a new hash
# makes a fresh url = guaranteed cache miss. Hash the wasm AND boot.js (minus its
# own self-referential LH_BUILD line, to avoid a circular hash) so a JS-ONLY
# change to boot.js also busts — the buster used to key on the wasm alone, so a
# boot.js edit with unchanged wasm shipped stale to returning visitors.
HASH="$( { cat web/pkg/localharness_bg.wasm; grep -v 'const LH_BUILD =' web/boot.js; cat web/stripe-embed.js; } | (sha256sum 2>/dev/null || shasum -a 256) | cut -c1-12 )"
if [ -n "$HASH" ]; then
    sed -i.bak -E "s/const LH_BUILD = \"[^\"]*\"/const LH_BUILD = \"${HASH}\"/" web/boot.js && rm -f web/boot.js.bak
    sed -i.bak -E "s|(boot\.js\?v=)[A-Za-z0-9]*|\1${HASH}|" web/index.html && rm -f web/index.html.bak
    sed -i.bak -E "s|(stripe-embed\.js\?v=)[A-Za-z0-9]*|\1${HASH}|" web/index.html && rm -f web/index.html.bak
    echo "→ stamped bundle cache-buster: ${HASH}"
else
    echo "WARNING: could not hash wasm; bundle cache-buster NOT stamped" >&2
fi

echo "→ web/pkg/ updated. Commit the changes and push for Vercel to pick up."
ls -lh web/pkg
